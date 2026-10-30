//! The event loop: messages in, effects out.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::app::{App, Effect, Message};
use crate::frontend::Frontend;
use crate::source::Source;
use crate::store::Store;
use crate::thumbnails::Fetcher;

/// What effects need to do their work.
pub struct Services {
    pub source: Arc<dyn Source>,
    pub store: Store,
    /// `None` when thumbnails are off.
    pub thumbnails: Option<Fetcher>,
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
    frontend.start_input(tx.clone());
    watch_signals(tx.clone());
    frontend.draw(app)?;

    while let Some(msg) = rx.recv().await {
        let mut next = Some(msg);
        while let Some(msg) = next {
            for effect in app.update(msg) {
                if let ControlFlow::Break(result) = handle(effect, &tx, &source, thumbnails.as_ref(), &writer) {
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

/// Carries out one effect. Slow work runs in a task that sends a `Message`
/// when it is done. `Break` ends the loop with its result.
fn handle(
    effect: Effect,
    tx: &UnboundedSender<Message>,
    source: &Arc<dyn Source>,
    thumbnails: Option<&Fetcher>,
    writer: &Writer,
) -> ControlFlow<Result<()>> {
    match effect {
        Effect::Quit => return ControlFlow::Break(Ok(())),
        Effect::Fail(why) => return ControlFlow::Break(Err(anyhow!(why))),
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
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.channel(&id).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::ChannelLoaded { request, result });
            });
        }
        Effect::LoadPlaylist { request, id } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.playlist(&id).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::PlaylistLoaded { request, result });
            });
        }
        Effect::LoadMore { request, continuation } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.browse_more(&continuation).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::MoreLoaded { request, result });
            });
        }
        Effect::LoadVideo { request, id } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.video(&id).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::VideoLoaded { request, result });
            });
        }
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
    }
    ControlFlow::Continue(())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
