//! Video playback: libmpv decodes and renders, and the frames go to the
//! terminal through the Kitty graphics protocol.
//!
//! A `Player` plays one video. It has two threads:
//! - The render thread waits for mpv's frames, renders them into a buffer,
//!   and sends them to the `VideoSink`. mpv's render call waits until each
//!   frame is due, so this thread sets the pace.
//! - The event thread turns mpv events into `Message::Player`.
//!
//! The main thread only sends commands, which never wait. `stop` closes the
//! sink at once, so no frame comes after it, and leaves the rest of the
//! shutdown to a thread of its own.

mod ffi;
pub mod mpv;

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::app::Message;
use crate::app::player::{Geometry, PlayerCommand, PlayerEvent};
use crate::source::streams::Streams;
use crate::terminal::kitty::{Presented, VideoSink};
use mpv::{EndReason, Event, Events, Format, Frame, Mpv, Renderer, Signal, Value};

/// The oldest libmpv client API that reeltty supports (mpv 0.37).
pub const MIN_API: (u32, u32) = (2, 2);
/// How long the render thread waits for a frame before it checks for stop
/// and size changes.
const RENDER_POLL: Duration = Duration::from_millis(100);
/// How long a stopped player gives the terminal to read the last frames.
const REAP_WAIT: Duration = Duration::from_millis(200);
const STATS_EVERY: Duration = Duration::from_secs(5);

/// mpv options. mpv must not read the user's mpv configuration, take keys,
/// draw its on-screen display into the frames, or run yt-dlp itself.
const OPTIONS: &[(&str, &str)] = &[
    ("vo", "libmpv"),
    ("hwdec", "auto-copy-safe"),
    ("sws-fast", "yes"),
    ("ytdl", "no"),
    ("terminal", "no"),
    ("config", "no"),
    ("load-scripts", "no"),
    ("input-default-bindings", "no"),
    ("input-vo-keyboard", "no"),
    ("osd-level", "0"),
    ("osd-bar", "no"),
    ("idle", "yes"),
    ("keep-open", "no"),
    ("cache", "yes"),
    ("demuxer-max-bytes", "64MiB"),
    ("volume-max", "100"),
    ("audio-client-name", "reeltty"),
];

/// The properties that the event thread observes, by ID.
const PROPERTIES: &[(u64, &str, Format)] = &[
    (1, "time-pos", Format::Double),
    (2, "duration", Format::Double),
    (3, "pause", Format::Flag),
    (4, "paused-for-cache", Format::Flag),
    (5, "volume", Format::Double),
    (6, "mute", Format::Flag),
];

/// The libmpv client API version as (major, minor).
pub fn api_version() -> (u32, u32) {
    let v = ffi::mpv_client_api_version();
    (u32::try_from(v >> 16).unwrap_or(u32::MAX), u32::try_from(v & 0xffff).unwrap_or(u32::MAX))
}

/// Where a player sends its events.
pub type EventSender = Box<dyn Fn(PlayerEvent) + Send + Sync>;

/// An `EventSender` for the app's message channel.
pub fn to_app(tx: UnboundedSender<Message>, session: u64) -> EventSender {
    Box::new(move |event| {
        let _ = tx.send(Message::Player { session, event });
    })
}

/// What the render thread reads from the main thread.
struct Shared {
    quit: AtomicBool,
    /// Changes when the geometry changes.
    generation: AtomicU64,
    geometry: Mutex<Geometry>,
    signal: Arc<Signal>,
}

pub struct Player {
    session: u64,
    mpv: Mpv,
    sink: Arc<VideoSink>,
    shared: Arc<Shared>,
    events_quit: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player").field("session", &self.session).finish_non_exhaustive()
    }
}

impl Player {
    /// Starts mpv on `streams`. Frames go to `sink`, and events to `send`.
    pub fn start(
        session: u64,
        streams: &Streams,
        geometry: Geometry,
        sink: Arc<VideoSink>,
        send: EventSender,
        extra_options: &[(&str, &str)],
    ) -> Result<Self, String> {
        let api = api_version();
        if api.0 != MIN_API.0 || api.1 < MIN_API.1 {
            return Err(format!("libmpv client API {}.{} is not supported. Install mpv 0.37 or later", api.0, api.1));
        }
        let started = Instant::now();
        let user_agent = streams.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("user-agent"));
        let mut options: Vec<(&str, &str)> = OPTIONS.iter().chain(extra_options).copied().collect();
        if let Some((_, ua)) = user_agent {
            options.push(("user-agent", ua));
        }
        let (mpv, events) = Mpv::new(&options).map_err(|e| e.to_string())?;
        let setup = || -> mpv::Result<Renderer> {
            mpv.request_log_messages("warn")?;
            for (id, name, format) in PROPERTIES {
                mpv.observe(*id, name, *format)?;
            }
            // List options split on commas, and `Accept` has commas. Add one at a time.
            for (k, v) in &streams.headers {
                if !k.eq_ignore_ascii_case("user-agent") {
                    mpv.command(&["change-list", "http-header-fields", "append", &format!("{k}: {v}")])?;
                }
            }
            // A plain list would split the URL at its colons.
            if let Some(audio) = &streams.audio_url {
                mpv.command(&["change-list", "audio-files", "append", audio])?;
            }
            // The renderer must exist before the file loads. The load runs in
            // mpv, so it is safe to start it before the threads: its events wait
            // in the queue. If it fails, nothing has started yet.
            let renderer = Renderer::new(&mpv)?;
            mpv.command_async(0, &["loadfile", &streams.video_url])?;
            Ok(renderer)
        };
        let renderer = setup().map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared {
            quit: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            geometry: Mutex::new(geometry),
            signal: renderer.signal(),
        });
        let send = Arc::new(send);
        sink.open(session);
        let render = {
            let (shared, sink, send) = (Arc::clone(&shared), Arc::clone(&sink), Arc::clone(&send));
            spawn("mpv-render", Arc::clone(&send), move || render_loop(session, renderer, &shared, &sink, &send))
        };
        let events_quit = Arc::new(AtomicBool::new(false));
        let events = {
            let quit = Arc::clone(&events_quit);
            spawn("mpv-events", Arc::clone(&send), move || event_loop(events, &quit, &send))
        };
        tracing::debug!("mpv started in {:?}", started.elapsed());
        Ok(Self { session, mpv, sink, shared, events_quit, threads: vec![render, events] })
    }

    /// Runs a control command. Commands never wait for mpv.
    pub fn command(&self, command: &PlayerCommand) {
        let result = match command {
            PlayerCommand::TogglePause => self.mpv.command_async(0, &["cycle", "pause"]),
            PlayerCommand::Seek { secs } => self.mpv.command_async(0, &["seek", &secs.to_string(), "relative"]),
            PlayerCommand::Volume { delta } => self.mpv.command_async(0, &["add", "volume", &delta.to_string()]),
            PlayerCommand::ToggleMute => self.mpv.command_async(0, &["cycle", "mute"]),
            PlayerCommand::Resize(g) => {
                *self.shared.geometry.lock().unwrap_or_else(PoisonError::into_inner) = *g;
                self.shared.generation.fetch_add(1, Ordering::Relaxed);
                self.shared.signal.notify();
                Ok(())
            }
            PlayerCommand::Start { .. } | PlayerCommand::Stop => Ok(()),
        };
        if let Err(e) = result {
            tracing::warn!("player command {command:?}: {e}");
        }
    }

    /// Stops playback. The video image is gone when this returns. The
    /// threads and mpv stop on the returned thread.
    pub fn stop(self) -> JoinHandle<()> {
        self.sink.close();
        let Self { session, mpv, sink, shared, events_quit, threads } = self;
        std::thread::Builder::new()
            .name("player-reaper".into())
            .spawn(move || {
                // The render thread goes first: it frees the render context
                // while the core is still alive.
                shared.quit.store(true, Ordering::Relaxed);
                shared.signal.notify();
                events_quit.store(true, Ordering::Relaxed);
                let mut threads = threads.into_iter();
                if let Some(render) = threads.next() {
                    let _ = render.join();
                }
                mpv.wakeup();
                for t in threads {
                    let _ = t.join();
                }
                // The last reference: mpv stops here.
                drop(mpv);
                sink.reap(session, REAP_WAIT);
            })
            .expect("start the player-reaper thread")
    }
}

/// Starts a thread that reports a panic as `PlayerEvent::Crashed`.
fn spawn(name: &str, send: Arc<EventSender>, body: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    let thread = name.to_string();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(body)) {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(ToString::to_string)
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                send(PlayerEvent::Crashed(format!("{thread}: {why}")));
            }
        })
        .unwrap_or_else(|e| panic!("start the {name} thread: {e}"))
}

fn render_loop(session: u64, mut mpv_renderer: Renderer, shared: &Shared, sink: &VideoSink, send: &EventSender) {
    let mut geometry = *shared.geometry.lock().unwrap_or_else(PoisonError::into_inner);
    let mut generation = shared.generation.load(Ordering::Relaxed);
    let mut frame = Frame::new(geometry.width, geometry.height);
    let (mut rendered, mut shown, mut skipped, mut write_failed) = (false, 0u32, 0u32, false);
    // The last frame was dropped by backpressure. Without new frames (when
    // paused), it must be sent again, or a resize or a seek never shows.
    let mut redraw = false;
    let mut stats = Instant::now();
    while !shared.quit.load(Ordering::Relaxed) {
        let woke = shared.signal.wait(RENDER_POLL);
        let resized = shared.generation.load(Ordering::Relaxed) != generation;
        if resized {
            generation = shared.generation.load(Ordering::Relaxed);
            geometry = *shared.geometry.lock().unwrap_or_else(PoisonError::into_inner);
            frame = Frame::new(geometry.width, geometry.height);
        }
        let new_frame = woke && mpv_renderer.has_frame();
        // After a resize, render the last frame again, so a paused video is
        // shown at the new size.
        if !(new_frame || (resized || redraw) && rendered) {
            continue;
        }
        if let Err(e) = mpv_renderer.render(&mut frame) {
            send(PlayerEvent::Failed(e.to_string()));
            return;
        }
        rendered = true;
        frame.set_opaque();
        let size = (frame.width(), frame.height());
        match sink.present(session, frame.bytes(), size, (geometry.cols, geometry.rows)) {
            Ok(Presented::Shown) => {
                shown += 1;
                redraw = false;
            }
            Ok(Presented::Skipped) => {
                skipped += 1;
                redraw = true;
            }
            Ok(Presented::Closed) => redraw = false,
            Err(e) => {
                if !write_failed {
                    tracing::warn!("cannot send a video frame: {e:#}");
                }
                write_failed = true;
            }
        }
        mpv_renderer.report_swap();
        if stats.elapsed() >= STATS_EVERY {
            let secs = stats.elapsed().as_secs_f64();
            tracing::debug!("video: {:.1} fps, {skipped} frames skipped", f64::from(shown) / secs);
            (shown, skipped, stats) = (0, 0, Instant::now());
        }
    }
}

/// Error lines from mpv's log, for the message when a file fails.
#[derive(Default)]
struct Errors(Vec<String>);

impl Errors {
    const KEEP: usize = 5;

    fn push(&mut self, line: String) {
        if self.0.len() == Self::KEEP {
            self.0.remove(0);
        }
        self.0.push(line);
    }

    /// The most useful line: an HTTP error if there is one, else the last.
    fn best(&self) -> Option<&String> {
        self.0.iter().rev().find(|l| l.contains("HTTP error")).or_else(|| self.0.last())
    }
}

fn event_loop(mut events: Events, quit: &AtomicBool, send: &EventSender) {
    let mut errors = Errors::default();
    let mut second = None;
    while !quit.load(Ordering::Relaxed) {
        let Some(event) = events.wait(Duration::from_millis(500)) else { continue };
        match event {
            Event::Shutdown => return,
            Event::Log { level, prefix, text } => {
                tracing::debug!("mpv {level} [{prefix}] {text}");
                if level == "error" || level == "fatal" {
                    errors.push(format!("{prefix}: {text}"));
                }
            }
            Event::CommandReply { error: Some(e), .. } => tracing::debug!("mpv command failed: {e}"),
            Event::FileLoaded => send(PlayerEvent::FileLoaded),
            Event::EndFile { reason: EndReason::Eof, .. } => send(PlayerEvent::Ended),
            Event::EndFile { reason: EndReason::Error, error } => {
                let why = errors.best().cloned().or(error).unwrap_or_else(|| "playback failed".into());
                send(PlayerEvent::Failed(why));
            }
            // Show the new position at once after a seek.
            Event::Seek | Event::PlaybackRestart => second = None,
            Event::Property { id, value } => {
                if let Some(event) = property_event(id, value, &mut second) {
                    send(event);
                }
            }
            Event::EndFile { .. } | Event::CommandReply { .. } | Event::Other(_) => {}
        }
    }
}

/// The app event for an observed property. The position is sent once a
/// second, not on every frame.
fn property_event(id: u64, value: Value, second: &mut Option<u64>) -> Option<PlayerEvent> {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "rounded and clamped")]
    let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
    match (id, value) {
        (1, Value::Double(t)) => {
            let ms = ms(t);
            if *second == Some(ms / 1000) {
                return None;
            }
            *second = Some(ms / 1000);
            Some(PlayerEvent::Position(ms))
        }
        (2, Value::Double(d)) => Some(PlayerEvent::Duration(ms(d))),
        (3, Value::Flag(b)) => Some(PlayerEvent::Paused(b)),
        (4, Value::Flag(b)) => Some(PlayerEvent::Buffering(b)),
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..=100")]
        (5, Value::Double(v)) => Some(PlayerEvent::Volume(v.clamp(0.0, 100.0).round() as u8)),
        (6, Value::Flag(b)) => Some(PlayerEvent::Muted(b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_come_once_a_second() {
        let mut second = None;
        let mut position = |t| property_event(1, Value::Double(t), &mut second);
        assert_eq!(position(0.0), Some(PlayerEvent::Position(0)));
        assert_eq!(position(0.5), None);
        assert_eq!(position(1.02), Some(PlayerEvent::Position(1020)));
        assert_eq!(property_event(5, Value::Double(42.4), &mut None), Some(PlayerEvent::Volume(42)));
        assert_eq!(property_event(2, Value::None, &mut None), None);
    }

    #[test]
    fn errors_prefer_http_lines() {
        let mut e = Errors::default();
        assert_eq!(e.best(), None);
        e.push("ffmpeg: https: HTTP error 403 Forbidden".into());
        e.push("cplayer: Failed to open".into());
        assert_eq!(e.best().unwrap(), "ffmpeg: https: HTTP error 403 Forbidden");
        for i in 0..Errors::KEEP {
            e.push(format!("line {i}"));
        }
        assert_eq!(e.best().unwrap(), "line 4");
    }

    #[test]
    fn supports_the_linked_libmpv() {
        let (major, minor) = api_version();
        assert!(major == MIN_API.0 && minor >= MIN_API.1);
    }
}
