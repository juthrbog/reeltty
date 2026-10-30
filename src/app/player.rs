//! The player screen: its state and how it changes.
//!
//! Playing a video goes through these phases:
//! 1. `Resolving`: yt-dlp finds the stream URLs.
//! 2. `Waiting`: YouTube makes guests wait as long as the ads would play.
//! 3. `Loading`: mpv opens the streams.
//! 4. `Playing`, then `Ended`. Or `Failed` at any point.
//!
//! The app never reads the clock. The runtime puts the time in the messages.

use super::{App, Effect};
use crate::keymap::{KeyPress, PlayerAction};
use crate::source::streams::{Streams, rung};
use crate::terminal::kitty::MAX_CELLS;

/// Rows under the video for the progress bar and the status line.
pub const STATUS_ROWS: u16 = 2;
/// Videos are rendered at most this large (DESIGN.md Q13).
const MAX_RENDER: (u32, u32) = (1280, 720);
/// Most terminals report no pixel size in tests and some setups.
const DEFAULT_CELL: (u16, u16) = (10, 20);
/// The aspect ratio until the stream gives the real one.
const DEFAULT_ASPECT: (u32, u32) = (16, 9);
const SEEK_SECS: i32 = 5;
const SEEK_LONG_SECS: i32 = 60;
const VOLUME_STEP: i8 = 2;

/// Where the video goes on screen, in cells, and the pixel size that mpv
/// renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Geometry {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    /// A multiple of 16.
    pub width: u16,
    pub height: u16,
}

impl Geometry {
    /// Fits a video with the `aspect` ratio into a screen of `cols` x `rows`
    /// cells of `cell` pixels, above the status rows, and centers it.
    pub fn fit(cols: u16, rows: u16, cell: (u16, u16), aspect: (u32, u32)) -> Self {
        let (cw, ch) = if cell.0 == 0 || cell.1 == 0 { DEFAULT_CELL } else { cell };
        let (cw, ch) = (u32::from(cw), u32::from(ch));
        let area_cols = u32::from(cols).clamp(1, u32::from(MAX_CELLS));
        let area_rows = u32::from(rows.saturating_sub(STATUS_ROWS)).clamp(1, u32::from(MAX_CELLS));
        let (aw, ah) = if aspect.0 == 0 || aspect.1 == 0 { DEFAULT_ASPECT } else { aspect };
        let (pw, ph) = (area_cols * cw, area_rows * ch);
        // The largest picture with the right shape in the area, in pixels.
        let (iw, ih) = if u64::from(pw) * u64::from(ah) <= u64::from(ph) * u64::from(aw) {
            (pw, pw * ah / aw)
        } else {
            (ph * aw / ah, ph)
        };
        let gc = ((iw + cw / 2) / cw).clamp(1, area_cols);
        let gr = ((ih + ch / 2) / ch).clamp(1, area_rows);
        // Render the size of the cells, but no larger than 720p.
        let (mut w, mut h) = (gc * cw, gr * ch);
        let scale =
            f64::min(1.0, f64::min(f64::from(MAX_RENDER.0) / f64::from(w), f64::from(MAX_RENDER.1) / f64::from(h)));
        if scale < 1.0 {
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "at most 1280 x 720")]
            {
                w = (f64::from(w) * scale) as u32;
                h = (f64::from(h) * scale) as u32;
            }
        }
        let small = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
        // Center in the whole screen, which can be larger than the most cells a picture can have.
        let screen_cols = u32::from(cols).max(1);
        let screen_rows = u32::from(rows.saturating_sub(STATUS_ROWS)).max(1);
        Self {
            x: small((screen_cols - gc) / 2),
            y: small((screen_rows - gr) / 2),
            cols: small(gc),
            rows: small(gr),
            width: small((w / 16).max(1) * 16),
            height: small((h / 2).max(1) * 2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Resolving,
    /// The ads wait. Times are Unix seconds.
    Waiting {
        until: u64,
        now: u64,
    },
    Loading,
    Playing,
    Ended,
    Failed(String),
}

/// What mpv reports about playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    FileLoaded,
    /// Milliseconds.
    Position(u64),
    /// Milliseconds.
    Duration(u64),
    Paused(bool),
    /// mpv waits for the network.
    Buffering(bool),
    Volume(u8),
    Muted(bool),
    Ended,
    Failed(String),
    /// A player thread stopped by a bug. The app quits.
    Crashed(String),
}

/// What the runtime does with the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerCommand {
    Start {
        session: u64,
        streams: Box<Streams>,
        geometry: Geometry,
    },
    TogglePause,
    Seek {
        secs: i32,
    },
    Volume {
        delta: i8,
    },
    ToggleMute,
    Resize(Geometry),
    /// Stops playback, and the stream search or the wait before it.
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "independent flags, most of them from mpv")]
pub struct PlayerState {
    pub session: u64,
    pub id: String,
    pub title: String,
    pub phase: Phase,
    pub paused: bool,
    pub buffering: bool,
    pub volume: u8,
    pub muted: bool,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub geometry: Geometry,
    /// The width and height of the video, for its shape on screen.
    aspect: (u32, u32),
    /// Kept for the end of the wait.
    streams: Option<Box<Streams>>,
    /// The streams were found again once after an HTTP 403 error.
    retried: bool,
}

impl PlayerState {
    /// Whether mpv runs, so that the controls work.
    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Loading | Phase::Playing | Phase::Ended)
    }

    /// Seconds left to wait for the ads.
    pub fn wait_left(&self) -> Option<u64> {
        match self.phase {
            Phase::Waiting { until, now } => Some(until.saturating_sub(now)),
            _ => None,
        }
    }
}

impl App {
    /// Plays a video, or says why it cannot.
    pub fn play(&mut self, id: String, title: String) -> Vec<Effect> {
        if let Some(why) = self.graphics.message() {
            self.status = Some(why);
            return Vec::new();
        }
        let mut effects = Vec::new();
        if self.player.is_some() {
            effects.push(Effect::Player(PlayerCommand::Stop));
        }
        self.session += 1;
        let geometry = Geometry::fit(self.width, self.height, self.cell, DEFAULT_ASPECT);
        let state = PlayerState {
            session: self.session,
            id,
            title,
            phase: Phase::Resolving,
            paused: false,
            buffering: false,
            volume: 100,
            muted: false,
            position_ms: 0,
            duration_ms: None,
            geometry,
            aspect: DEFAULT_ASPECT,
            streams: None,
            retried: false,
        };
        effects.push(self.resolve(&state));
        self.player = Some(state);
        effects
    }

    fn resolve(&self, p: &PlayerState) -> Effect {
        let (_, ch) = if self.cell.0 == 0 || self.cell.1 == 0 { DEFAULT_CELL } else { self.cell };
        let pane = u32::from(self.height.saturating_sub(STATUS_ROWS)) * u32::from(ch);
        Effect::ResolveStreams { session: p.session, id: p.id.clone(), max_height: rung(pane.min(MAX_RENDER.1)) }
    }

    /// The player for messages of `session`, if it is still the current one.
    fn player_of(&mut self, session: u64) -> Option<&mut PlayerState> {
        self.player.as_mut().filter(|p| p.session == session)
    }

    pub(super) fn on_streams(&mut self, session: u64, now: u64, result: Result<Box<Streams>, String>) -> Vec<Effect> {
        let (width, height, cell) = (self.width, self.height, self.cell);
        let Some(p) = self.player_of(session) else { return Vec::new() };
        if p.phase != Phase::Resolving {
            return Vec::new();
        }
        let streams = match result {
            Ok(s) => s,
            Err(e) => {
                p.phase = Phase::Failed(e);
                return Vec::new();
            }
        };
        if let (Some(w), Some(h)) = (streams.format.width, streams.format.height) {
            p.aspect = (w, h);
            p.geometry = Geometry::fit(width, height, cell, p.aspect);
        }
        match streams.available_at {
            Some(until) if until > now => {
                p.phase = Phase::Waiting { until, now };
                p.streams = Some(streams);
                vec![Effect::Countdown { session, until }]
            }
            _ => start(p, streams),
        }
    }

    pub(super) fn on_tick(&mut self, session: u64, now: u64) -> Vec<Effect> {
        let Some(p) = self.player_of(session) else { return Vec::new() };
        let Phase::Waiting { until, .. } = p.phase else { return Vec::new() };
        if now < until {
            p.phase = Phase::Waiting { until, now };
            return Vec::new();
        }
        match p.streams.take() {
            Some(streams) => start(p, streams),
            None => Vec::new(),
        }
    }

    pub(super) fn on_player_event(&mut self, session: u64, event: PlayerEvent) -> Vec<Effect> {
        if let PlayerEvent::Crashed(why) = event {
            return vec![Effect::Fail(format!("the player stopped: {why}"))];
        }
        let Some(p) = self.player_of(session) else { return Vec::new() };
        match event {
            PlayerEvent::FileLoaded if p.phase == Phase::Loading => p.phase = Phase::Playing,
            PlayerEvent::Position(ms) => p.position_ms = ms,
            PlayerEvent::Duration(ms) => p.duration_ms = Some(ms),
            PlayerEvent::Paused(b) => p.paused = b,
            PlayerEvent::Buffering(b) => p.buffering = b,
            PlayerEvent::Volume(v) => p.volume = v,
            PlayerEvent::Muted(b) => p.muted = b,
            PlayerEvent::Ended if p.is_running() => p.phase = Phase::Ended,
            PlayerEvent::Failed(e) if p.is_running() => {
                // The URLs can expire, or the ads wait was too short. Find them once more.
                if e.contains("403") && !p.retried {
                    p.retried = true;
                    p.phase = Phase::Resolving;
                    p.position_ms = 0;
                    let resolve = self.player.as_ref().map(|p| self.resolve(p));
                    return [Some(Effect::Player(PlayerCommand::Stop)), resolve].into_iter().flatten().collect();
                }
                p.phase = Phase::Failed(e);
                return vec![Effect::Player(PlayerCommand::Stop)];
            }
            _ => {}
        }
        Vec::new()
    }

    pub(super) fn on_player_key(&mut self, key: KeyPress) -> Vec<Effect> {
        let action = self.keymap.player.action(key);
        if action == Some(PlayerAction::Back) {
            self.player = None;
            return vec![Effect::Player(PlayerCommand::Stop)];
        }
        let Some(p) = &self.player else { return Vec::new() };
        if !p.is_running() {
            return Vec::new();
        }
        let command = match action {
            Some(PlayerAction::TogglePause) => PlayerCommand::TogglePause,
            Some(PlayerAction::SeekForward) => PlayerCommand::Seek { secs: SEEK_SECS },
            Some(PlayerAction::SeekBack) => PlayerCommand::Seek { secs: -SEEK_SECS },
            Some(PlayerAction::SeekForwardLong) => PlayerCommand::Seek { secs: SEEK_LONG_SECS },
            Some(PlayerAction::SeekBackLong) => PlayerCommand::Seek { secs: -SEEK_LONG_SECS },
            Some(PlayerAction::VolumeUp) => PlayerCommand::Volume { delta: VOLUME_STEP },
            Some(PlayerAction::VolumeDown) => PlayerCommand::Volume { delta: -VOLUME_STEP },
            Some(PlayerAction::Mute) => PlayerCommand::ToggleMute,
            Some(PlayerAction::Back) | None => return Vec::new(),
        };
        vec![Effect::Player(command)]
    }

    /// Fits the video to a new screen or cell size.
    pub(super) fn refit_player(&mut self) -> Vec<Effect> {
        let (width, height, cell) = (self.width, self.height, self.cell);
        let Some(p) = &mut self.player else { return Vec::new() };
        let geometry = Geometry::fit(width, height, cell, p.aspect);
        if geometry == p.geometry {
            return Vec::new();
        }
        p.geometry = geometry;
        if p.is_running() { vec![Effect::Player(PlayerCommand::Resize(geometry))] } else { Vec::new() }
    }
}

fn start(p: &mut PlayerState, streams: Box<Streams>) -> Vec<Effect> {
    p.phase = Phase::Loading;
    vec![Effect::Player(PlayerCommand::Start { session: p.session, streams, geometry: p.geometry })]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Input, Message};
    use crate::keymap::KeyMap;
    use crate::source::streams::FormatInfo;
    use crate::terminal::detect::Graphics;

    fn app() -> App {
        let mut a = App::new(KeyMap::default(), Graphics::KittyShm);
        a.update(Message::Input(Input::Resize { width: 80, height: 24 }));
        a.update(Message::Input(Input::CellSize { width: 10, height: 20 }));
        a
    }

    fn key(a: &mut App, s: &str) -> Vec<Effect> {
        a.update(Message::Input(Input::Key(s.parse().unwrap())))
    }

    fn streams(available_at: Option<u64>) -> Box<Streams> {
        Box::new(Streams {
            video_url: "https://v".into(),
            audio_url: Some("https://a".into()),
            headers: Vec::new(),
            available_at,
            expires_at: None,
            format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
        })
    }

    fn phase(a: &App) -> Phase {
        a.player.as_ref().unwrap().phase.clone()
    }

    #[test]
    fn fits_16_9_into_a_screen() {
        // 80 x 22 cells of 10 x 20 px is 800 x 440 px. 16:9 at 800 px wide is 450 px high, too high.
        let g = Geometry::fit(80, 24, (10, 20), (16, 9));
        assert_eq!((g.cols, g.rows), (78, 22));
        assert_eq!((g.x, g.y), (1, 0));
        assert_eq!((g.width, g.height), (768, 440));
        // A wide screen: the height limits it, and it is centered.
        let g = Geometry::fit(200, 30, (10, 20), (16, 9));
        assert_eq!((g.cols, g.rows, g.x, g.y), (100, 28, 50, 0));
        // A portrait video.
        let g = Geometry::fit(80, 24, (10, 20), (9, 16));
        assert_eq!((g.cols, g.rows), (25, 22));
        assert_eq!(g.x, 27);
    }

    #[test]
    fn renders_at_most_720p_in_whole_blocks() {
        let g = Geometry::fit(250, 70, (10, 20), (16, 9));
        assert!(g.width <= 1280 && g.height <= 720, "{g:?}");
        assert_eq!(g.width % 16, 0);
        assert_eq!(g.height % 2, 0);
        // Unknown cell size and tiny screens still give a picture.
        let g = Geometry::fit(1, 1, (0, 0), (0, 0));
        assert_eq!((g.cols, g.rows, g.width, g.height), (1, 1, 16, 20));
        // Kitty placeholders cannot address more cells than this, and the
        // picture is centered in the whole screen.
        let g = Geometry::fit(1000, 400, (2, 4), (16, 9));
        assert!(g.cols <= MAX_CELLS && g.rows <= MAX_CELLS, "{g:?}");
        assert_eq!(g.x, (1000 - g.cols) / 2);
        assert_eq!(g.y, (398 - g.rows) / 2);
    }

    #[test]
    fn plays_after_the_streams_and_the_ads_wait() {
        let mut a = app();
        let effects = a.play("v1".into(), "Title".into());
        // 22 rows of 20 px is 440 px, so 480p.
        assert_eq!(effects, [Effect::ResolveStreams { session: 1, id: "v1".into(), max_height: 480 }]);
        assert_eq!(phase(&a), Phase::Resolving);

        let effects = a.update(Message::StreamsResolved { session: 1, now: 100, result: Ok(streams(Some(115))) });
        assert_eq!(effects, [Effect::Countdown { session: 1, until: 115 }]);
        assert_eq!(a.player.as_ref().unwrap().wait_left(), Some(15));
        assert!(a.update(Message::Tick { session: 1, now: 110 }).is_empty());
        assert_eq!(a.player.as_ref().unwrap().wait_left(), Some(5));
        // A tick for an old session does nothing.
        assert!(a.update(Message::Tick { session: 0, now: 200 }).is_empty());

        let effects = a.update(Message::Tick { session: 1, now: 115 });
        let geometry = a.player.as_ref().unwrap().geometry;
        assert_eq!(
            effects,
            [Effect::Player(PlayerCommand::Start { session: 1, streams: streams(Some(115)), geometry })]
        );
        assert_eq!(phase(&a), Phase::Loading);
        a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded });
        assert_eq!(phase(&a), Phase::Playing);
        a.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        assert_eq!(phase(&a), Phase::Ended);
    }

    #[test]
    fn plays_at_once_without_ads() {
        let mut a = app();
        a.play("v1".into(), "Title".into());
        let effects = a.update(Message::StreamsResolved { session: 1, now: 100, result: Ok(streams(Some(100))) });
        assert!(matches!(effects.as_slice(), [Effect::Player(PlayerCommand::Start { session: 1, .. })]));
    }

    #[test]
    fn errors_stop_the_player() {
        let mut a = app();
        a.play("v1".into(), "Title".into());
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Err("yt-dlp failed: private video".into()) });
        assert_eq!(phase(&a), Phase::Failed("yt-dlp failed: private video".into()));
        // Keys other than back do nothing now.
        assert!(key(&mut a, "space").is_empty());

        a.play("v2".into(), "Title".into());
        a.update(Message::StreamsResolved { session: 2, now: 0, result: Ok(streams(None)) });
        let effects = a.update(Message::Player { session: 2, event: PlayerEvent::Failed("HTTP error 403".into()) });
        // One more try with new URLs.
        assert_eq!(
            effects,
            [
                Effect::Player(PlayerCommand::Stop),
                Effect::ResolveStreams { session: 2, id: "v2".into(), max_height: 480 }
            ]
        );
        a.update(Message::StreamsResolved { session: 2, now: 0, result: Ok(streams(None)) });
        let effects = a.update(Message::Player { session: 2, event: PlayerEvent::Failed("HTTP error 403".into()) });
        assert_eq!(effects, [Effect::Player(PlayerCommand::Stop)]);
        assert_eq!(phase(&a), Phase::Failed("HTTP error 403".into()));

        let effects = a.update(Message::Player { session: 2, event: PlayerEvent::Crashed("render".into()) });
        assert!(matches!(effects.as_slice(), [Effect::Fail(_)]));
    }

    #[test]
    fn keys_control_the_player() {
        let mut a = app();
        a.play("v1".into(), "Title".into());
        // No controls before mpv runs.
        assert!(key(&mut a, "space").is_empty());
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(streams(None)) });
        let cases = [
            ("space", PlayerCommand::TogglePause),
            ("right", PlayerCommand::Seek { secs: 5 }),
            ("left", PlayerCommand::Seek { secs: -5 }),
            ("up", PlayerCommand::Seek { secs: 60 }),
            ("down", PlayerCommand::Seek { secs: -60 }),
            ("0", PlayerCommand::Volume { delta: 2 }),
            ("9", PlayerCommand::Volume { delta: -2 }),
            ("m", PlayerCommand::ToggleMute),
        ];
        for (k, command) in cases {
            assert_eq!(key(&mut a, k), [Effect::Player(command)], "{k}");
        }
        assert!(key(&mut a, "j").is_empty());
        assert_eq!(key(&mut a, "ctrl-c"), [Effect::Quit]);
        assert_eq!(key(&mut a, "q"), [Effect::Player(PlayerCommand::Stop)]);
        assert_eq!(a.player, None);
    }

    #[test]
    fn resizes_while_running() {
        let mut a = app();
        a.play("v1".into(), "Title".into());
        // Not running yet: only the state changes.
        assert!(a.update(Message::Input(Input::Resize { width: 100, height: 30 })).is_empty());
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(streams(None)) });
        let effects = a.update(Message::Input(Input::Resize { width: 120, height: 40 }));
        let g = a.player.as_ref().unwrap().geometry;
        assert_eq!(effects, [Effect::Player(PlayerCommand::Resize(g))]);
        assert!(a.update(Message::Input(Input::Resize { width: 120, height: 40 })).is_empty());
        let effects = a.update(Message::Input(Input::CellSize { width: 8, height: 16 }));
        assert!(matches!(effects.as_slice(), [Effect::Player(PlayerCommand::Resize(_))]));
    }

    #[test]
    fn no_video_without_graphics() {
        let mut a = App::new(KeyMap::default(), Graphics::Unavailable("tmux".into()));
        assert!(a.play("v1".into(), "Title".into()).is_empty());
        assert_eq!(a.player, None);
        assert!(a.status.unwrap().contains("tmux"));
    }
}
