//! The event loop: messages in, effects out.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::AbortHandle;

use crate::app::comments::CommentsFrom;
use crate::app::player::{PlayerCommand, PlayerEvent};
use crate::app::{App, Effect, Message};
use crate::frontend::Frontend;
use crate::player::{self, Player};
use crate::source::Source;
use crate::source::model::Item;
use crate::source::ytdlp::YtDlp;
use crate::sponsorblock;
use crate::store::Store;
use crate::terminal::kitty::VideoSink;
use crate::thumbnails::Fetcher;

/// What effects need to do their work.
pub struct Services {
    pub source: Arc<dyn Source>,
    pub store: Store,
    /// `None` when thumbnails are off.
    pub thumbnails: Option<Fetcher>,
    /// Finds stream URLs.
    pub ytdlp: YtDlp,
    /// `None` when SponsorBlock is off.
    pub sponsorblock: Option<sponsorblock::Client>,
}

/// The video that plays, and the work before it.
struct Playback {
    ytdlp: YtDlp,
    sponsorblock: Option<sponsorblock::Client>,
    sink: Option<Arc<VideoSink>>,
    player: Option<Player>,
    /// The stream search or the ads wait.
    pending: Option<AbortHandle>,
    /// Stopped players that are still shutting down.
    reapers: Vec<JoinHandle<()>>,
}

impl Playback {
    fn stop(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.abort();
        }
        if let Some(player) = self.player.take() {
            self.reapers.push(player.stop());
        }
        self.reapers.retain(|r| !r.is_finished());
    }

    /// Starts `task` in place of the pending one.
    fn set_pending(&mut self, task: impl Future<Output = ()> + Send + 'static) {
        if let Some(pending) = self.pending.take() {
            pending.abort();
        }
        self.pending = Some(tokio::spawn(task).abort_handle());
    }

    fn handle(&mut self, command: PlayerCommand, tx: &UnboundedSender<Message>) {
        match command {
            PlayerCommand::Start { session, streams, geometry, start_secs } => {
                self.stop();
                let send = player::to_app(tx.clone(), session);
                let Some(sink) = self.sink.clone() else {
                    send(PlayerEvent::Failed("this terminal cannot show video".into()));
                    return;
                };
                match Player::start(session, &streams, geometry, sink, send, start_secs, &[]) {
                    Ok(p) => self.player = Some(p),
                    Err(e) => {
                        tracing::error!("cannot start the player: {e}");
                        let _ = tx.send(Message::Player { session, event: PlayerEvent::Failed(e) });
                    }
                }
            }
            PlayerCommand::Stop => self.stop(),
            command => {
                if let Some(p) = &self.player {
                    p.command(&command);
                }
            }
        }
    }
}

impl Drop for Playback {
    /// Stops the video before the terminal is restored, and waits for mpv to stop.
    fn drop(&mut self) {
        self.stop();
        for reaper in self.reapers.drain(..) {
            let _ = reaper.join();
        }
    }
}

type Write = Box<dyn FnOnce(&Store) -> Result<()> + Send>;

/// Database writes run on one thread, in the order the effects came, so a
/// quick subscribe and unsubscribe can never swap.
/// Dropping it waits for the queued writes, so quitting right after a
/// subscribe does not lose it.
struct Writer {
    jobs: Option<std_mpsc::Sender<Write>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Writer {
    fn start(store: Store, tx: UnboundedSender<Message>) -> Self {
        let (jobs, rx) = std_mpsc::channel::<Write>();
        let thread = std::thread::Builder::new()
            .name("store".into())
            .spawn(move || {
                for job in rx {
                    if let Err(e) = job(&store) {
                        tracing::error!("store write failed: {e:#}");
                        let _ = tx.send(Message::StoreFailed(format!("{e:#}")));
                    }
                }
            })
            .expect("start the store thread");
        Self { jobs: Some(jobs), thread: Some(thread) }
    }

    fn write(&self, job: impl FnOnce(&Store) -> Result<()> + Send + 'static) {
        if self.jobs.as_ref().is_none_or(|j| j.send(Box::new(job)).is_err()) {
            tracing::error!("the store thread stopped");
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop after the last queued job.
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Runs until an `Effect::Quit`. Messages that arrive together are applied
/// before the next draw, so a burst of input gives one frame.
pub async fn run(app: &mut App, frontend: &mut impl Frontend, services: Services) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let writer = Writer::start(services.store, tx.clone());
    let (source, thumbnails) = (services.source, services.thumbnails);
    let mut playback = Playback {
        ytdlp: services.ytdlp,
        sponsorblock: services.sponsorblock,
        sink: frontend.video_sink(),
        player: None,
        pending: None,
        reapers: Vec::new(),
    };
    frontend.start_input(tx.clone());
    watch_signals(tx.clone());
    for effect in app.start() {
        let services = (&source, thumbnails.as_ref(), &writer, &mut playback);
        if let ControlFlow::Break(result) = handle(effect, &tx, services) {
            return result;
        }
    }
    frontend.draw(app)?;

    while let Some(msg) = rx.recv().await {
        let mut next = Some(msg);
        while let Some(msg) = next {
            for effect in app.update(msg) {
                let services = (&source, thumbnails.as_ref(), &writer, &mut playback);
                if let ControlFlow::Break(result) = handle(effect, &tx, services) {
                    return result;
                }
            }
            next = rx.try_recv().ok();
        }
        frontend.draw(app)?;
    }
    Err(anyhow!("all message senders stopped"))
}

/// Turns SIGTERM and SIGHUP into `Message::Terminate`, so the terminal is
/// restored on the normal exit path.
fn watch_signals(tx: UnboundedSender<Message>) {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async move {
        let (Ok(mut term), Ok(mut hup)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup())) else {
            tracing::warn!("cannot watch for signals");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
        let _ = tx.send(Message::Terminate);
    });
}

/// Runs `load` in a task, and sends the message that `reply` makes of its result.
fn fetch<T>(
    tx: &UnboundedSender<Message>,
    load: impl Future<Output = crate::source::Result<T>> + Send + 'static,
    reply: impl FnOnce(std::result::Result<T, String>) -> Message + Send + 'static,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let _ = tx.send(reply(load.await.map_err(|e| e.to_string())));
    });
}

/// Carries out an effect that loads from the source.
fn load(effect: Effect, tx: &UnboundedSender<Message>, source: &Arc<dyn Source>) {
    match effect {
        Effect::Search { generation, query } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.search(&query).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::SearchResult { generation, append: false, result });
            });
        }
        Effect::SearchMore { generation, continuation } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.search_more(&continuation).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::SearchResult { generation, append: true, result });
            });
        }
        Effect::LoadChannel { request, id } => {
            let source = Arc::clone(source);
            fetch(tx, async move { source.channel(&id).await }, move |result| Message::ChannelLoaded {
                request,
                result,
            });
        }
        Effect::LoadPlaylist { request, id } => {
            let source = Arc::clone(source);
            fetch(tx, async move { source.playlist(&id).await }, move |result| Message::PlaylistLoaded {
                request,
                result,
            });
        }
        Effect::LoadMore { request, continuation } => {
            let source = Arc::clone(source);
            fetch(tx, async move { source.browse_more(&continuation).await }, move |result| Message::MoreLoaded {
                request,
                result,
            });
        }
        Effect::LoadComments { request, generation, video_id, slot, from } => {
            let source = Arc::clone(source);
            let load = async move {
                match from {
                    CommentsFrom::Video(id) => source.comments(&id).await,
                    CommentsFrom::Token(token) => source.comments_more(&video_id, &token).await,
                }
            };
            fetch(tx, load, move |result| Message::CommentsLoaded { request, generation, slot, result });
        }
        Effect::LoadVideo { request, id } => {
            let source = Arc::clone(source);
            fetch(tx, async move { source.video(&id).await }, move |result| Message::VideoLoaded { request, result });
        }
        _ => unreachable!("not a source load: {effect:?}"),
    }
}

/// Carries out one effect. Slow work runs in a task that sends a `Message`
/// when it is done. `Break` ends the loop with its result.
fn handle(
    effect: Effect,
    tx: &UnboundedSender<Message>,
    (source, thumbnails, writer, playback): (&Arc<dyn Source>, Option<&Fetcher>, &Writer, &mut Playback),
) -> ControlFlow<Result<()>> {
    match effect {
        Effect::Quit => return ControlFlow::Break(Ok(())),
        Effect::Fail(why) => return ControlFlow::Break(Err(anyhow!(why))),
        effect @ (Effect::Search { .. }
        | Effect::SearchMore { .. }
        | Effect::LoadChannel { .. }
        | Effect::LoadPlaylist { .. }
        | Effect::LoadMore { .. }
        | Effect::LoadComments { .. }
        | Effect::LoadVideo { .. }) => load(effect, tx, source),
        Effect::Subscribe(mut s) => {
            s.subscribed_at = now();
            writer.write(move |store| store.subscribe(&s));
        }
        Effect::Unsubscribe { channel_id } => writer.write(move |store| store.unsubscribe(&channel_id)),
        Effect::SavePlaylist(mut p) => {
            p.saved_at = now();
            writer.write(move |store| store.save_playlist(&p));
        }
        Effect::UnsavePlaylist { playlist_id } => writer.write(move |store| store.unsave_playlist(&playlist_id)),
        Effect::SaveWatch(e) => writer.write(move |store| store.record_watch(&e)),
        Effect::LoadFeed { channels, force } => load_feed(tx.clone(), Arc::clone(source), &channels, force),
        Effect::SaveFeed { channel_id, fetched_at, videos } => {
            writer.write(move |store| store.save_channel_feed(&channel_id, fetched_at, &videos));
        }
        Effect::ForgetFeed { channel_id } => writer.write(move |store| store.forget_channel_feed(&channel_id)),
        Effect::ForgetWatch { video_id } => writer.write(move |store| store.forget_watch(&video_id)),
        Effect::LoadThumbnail { url, slot } => {
            let tx = tx.clone();
            let Some(fetcher) = thumbnails.cloned() else {
                let _ = tx.send(Message::ThumbnailLoaded { url, slot, result: Err("thumbnails are off".into()) });
                return ControlFlow::Continue(());
            };
            tokio::spawn(async move {
                let result = fetcher.load(&url, slot).await.map(Arc::new).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Message::ThumbnailLoaded { url, slot, result });
            });
        }
        Effect::ResolveStreams { session, id, max_height } => {
            let (tx, ytdlp) = (tx.clone(), playback.ytdlp.clone());
            playback.set_pending(async move {
                let result = ytdlp.streams(&id, max_height).await.map(Box::new).map_err(|e| e.to_string());
                let _ = tx.send(Message::StreamsResolved { session, now: now_unix(), result });
            });
        }
        Effect::Countdown { session, until } => playback.set_pending(countdown(tx.clone(), session, until)),
        Effect::Ticks { session, count } => playback.set_pending(ticks(tx.clone(), session, count)),
        Effect::LoadRelated { session, id } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.video(&id).await.map(|d| d.related).map_err(|e| e.to_string());
                let _ = tx.send(Message::RelatedLoaded { session, result });
            });
        }
        // Not pending work: it must not stop the stream search.
        Effect::LoadSegments { session, id } => {
            let Some(client) = playback.sponsorblock.clone() else { return ControlFlow::Continue(()) };
            let tx = tx.clone();
            tokio::spawn(async move {
                let result = client.segments(&id).await.map_err(|e| format!("{e:#}"));
                let _ = tx.send(Message::SegmentsLoaded { session, result });
            });
        }
        Effect::Player(command) => playback.handle(command, tx),
    }
    ControlFlow::Continue(())
}

/// How many channels the feed loads at the same time (DESIGN.md Q15).
const FEED_REQUESTS: usize = 4;

/// Loads the videos tab of each stale channel, a few at a time, and sends
/// each one as `Message::FeedChannelLoaded`.
fn load_feed(tx: UnboundedSender<Message>, source: Arc<dyn Source>, channels: &[(String, Option<i64>)], force: bool) {
    use futures::StreamExt as _;

    let stale = crate::app::feed::stale(channels, now(), force);
    let _ = tx.send(Message::FeedLoading { count: stale.len() });
    tokio::spawn(async move {
        let loads = futures::stream::iter(stale).map(|channel_id| {
            let source = Arc::clone(&source);
            async move {
                let result = source.channel(&channel_id).await.map_err(|e| e.to_string()).map(|page| {
                    page.videos
                        .items
                        .into_iter()
                        .filter_map(|item| match item {
                            Item::Video(v) => Some(v),
                            _ => None,
                        })
                        .collect()
                });
                Message::FeedChannelLoaded { channel_id, fetched_at: now(), result }
            }
        });
        let mut loads = loads.buffer_unordered(FEED_REQUESTS);
        while let Some(msg) = loads.next().await {
            if tx.send(msg).is_err() {
                return;
            }
        }
    });
}

/// Sends `Message::Tick` once a second, from now until the Unix time `until`.
async fn countdown(tx: UnboundedSender<Message>, session: u64, until: u64) {
    let mut every = tokio::time::interval(Duration::from_secs(1));
    loop {
        every.tick().await;
        let now = now_unix();
        if tx.send(Message::Tick { session, now }).is_err() || now >= until {
            return;
        }
    }
}

/// Sends `Message::Tick` once a second, `count` times, from a second from now.
async fn ticks(tx: UnboundedSender<Message>, session: u64, count: u8) {
    let mut every = tokio::time::interval(Duration::from_secs(1));
    // The first tick comes at once.
    every.tick().await;
    for _ in 0..count {
        every.tick().await;
        if tx.send(Message::Tick { session, now: now_unix() }).is_err() {
            return;
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
