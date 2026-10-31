//! The player screen: its state and how it changes.
//!
//! Playing a video goes through these phases:
//! 1. `Resolving`: yt-dlp finds the stream URLs.
//! 2. `Waiting`: YouTube makes guests wait as long as the ads would play.
//! 3. `Loading`: mpv opens the streams.
//! 4. `Playing`, then `Ended`. Or `Failed` at any point.
//!
//! The app never reads the clock. The runtime puts the time in the messages.

use super::{App, Effect, Tab};
use crate::keymap::{Action, KeyPress, PlayerAction};
use crate::source::model::{ChannelRef, Chapter, Video, VideoDetails, VideoKind};
use crate::source::streams::{Caption, Quality, Streams, pick_caption, rung};
use crate::sponsorblock::{self, Category, Segment};
use crate::store::HistoryEntry;
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
/// Seconds before autoplay starts the next video.
pub const AUTOPLAY_SECS: u8 = 5;
/// The most videos in the up next list.
const UP_NEXT_MAX: usize = 10;
/// The history saves the position this often while a video plays.
const SAVE_EVERY_SECS: u32 = 30;
/// Resume only from this far into a video, and this far before its end.
const RESUME_MARGIN_SECS: u32 = 30;
/// The most videos that "previous video" can go back.
const PLAYED_MAX: usize = 50;
/// Going to the previous chapter in the first seconds of a chapter goes to
/// the one before it, as in mpv.
const CHAPTER_GRACE_MS: u64 = 2000;
/// Rows under the mini-player's video for the title, the progress bar, and
/// the state.
pub const MINI_INFO_ROWS: u16 = 3;
/// The mini-player takes a third of the screen width, within these limits.
const MINI_COLS: (u16, u16) = (24, 64);
/// A smaller screen has no room for the mini-player. Playback goes on, and
/// the status bar shows the video.
const MINI_MIN_SCREEN: (u16, u16) = (72, 14);
/// The video size when the mini-player does not show, in cells.
const HIDDEN_CELLS: (u16, u16) = (16, 4);

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
        Self::fit_area(cols, rows.saturating_sub(STATUS_ROWS), cell, aspect)
    }

    /// Fits a video with the `aspect` ratio into an area of `cols` x `rows`
    /// cells of `cell` pixels, and centers it. `x` and `y` are from the top
    /// left of the area.
    pub fn fit_area(cols: u16, rows: u16, cell: (u16, u16), aspect: (u32, u32)) -> Self {
        let (cw, ch) = if cell.0 == 0 || cell.1 == 0 { DEFAULT_CELL } else { cell };
        let (cw, ch) = (u32::from(cw), u32::from(ch));
        let area_cols = u32::from(cols).clamp(1, u32::from(MAX_CELLS));
        let area_rows = u32::from(rows).clamp(1, u32::from(MAX_CELLS));
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
        let screen_rows = u32::from(rows).max(1);
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

/// The column on the right of the browse screen that holds the mini-player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dock {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

/// The dock on a screen of `width` x `height` cells: the right part of the
/// browse body, under the tab row and above the status bar. `None` when the
/// screen is too small.
pub fn dock(width: u16, height: u16) -> Option<Dock> {
    if width < MINI_MIN_SCREEN.0 || height < MINI_MIN_SCREEN.1 {
        return None;
    }
    let cols = (width / 3).clamp(MINI_COLS.0, MINI_COLS.1);
    Some(Dock { x: width - cols, y: 1, cols, rows: height - 2 })
}

/// Where the mini-player goes on a screen of `width` x `height` cells: the
/// dock, and the video in a box at the bottom of the dock, above the info
/// rows. The geometry is from the top left of the screen. `None` when the
/// screen is too small.
pub fn mini_layout(width: u16, height: u16, cell: (u16, u16), aspect: (u32, u32)) -> Option<(Dock, Geometry)> {
    let dock = dock(width, height)?;
    let cols = dock.cols;
    // A tall video takes at most half of the dock.
    let room = (dock.rows - MINI_INFO_ROWS - 2).min(dock.rows / 2);
    let mut g = Geometry::fit_area(cols - 2, room, cell, aspect);
    g.x += dock.x + 1;
    g.y = dock.y + dock.rows - MINI_INFO_ROWS - 1 - g.rows;
    Some((dock, g))
}

/// The video geometry of the full player, or of the mini-player.
fn player_geometry(mini: bool, width: u16, height: u16, cell: (u16, u16), aspect: (u32, u32)) -> Geometry {
    if !mini {
        return Geometry::fit(width, height, cell, aspect);
    }
    match mini_layout(width, height, cell, aspect) {
        Some((_, g)) => g,
        // Nothing shows it, so mpv renders a small picture.
        None => Geometry::fit_area(HIDDEN_CELLS.0, HIDDEN_CELLS.1, cell, aspect),
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
    /// Percent: 100 is normal speed.
    Speed(u16),
    /// mpv could not load the caption track.
    CaptionsFailed(String),
    /// The player skipped a SponsorBlock segment.
    Skipped(Category),
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
        /// Where to start, in seconds. Not 0 after an HTTP 403 retry.
        start_secs: u32,
    },
    TogglePause,
    Seek {
        secs: i32,
    },
    Volume {
        delta: i8,
    },
    ToggleMute,
    /// Faster, slower, or normal speed.
    Speed(SpeedStep),
    /// Seconds from the start.
    SeekTo {
        secs: u32,
    },
    /// Loads a caption track and shows it.
    AddCaptions {
        url: String,
        lang: String,
        name: String,
    },
    ShowCaptions(bool),
    /// Plays another size of the video stream from `secs`.
    SwitchVideo {
        url: String,
        secs: u32,
    },
    Resize(Geometry),
    /// The SponsorBlock segments to skip. `continues`: the player goes on
    /// where another one stopped, so it does not skip where it starts.
    Segments {
        segments: Vec<Segment>,
        continues: bool,
    },
    /// Stops playback, and the stream search or the wait before it.
    Stop,
}

/// mpv's speed keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedStep {
    Up,
    Down,
    Reset,
}

/// What played before a quality change, to go back to if the change fails.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Switch {
    url: String,
    quality: Option<u32>,
}

/// What can play after a video ends: the queue, then related videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpNext {
    pub videos: Vec<Video>,
    pub selection: usize,
    /// Seconds until the selected video plays. `None` when autoplay is off
    /// or the user moved in the list.
    pub countdown: Option<u8>,
}

/// The menu of video sizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityMenu {
    pub selection: usize,
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
    /// Percent: 100 is normal speed.
    pub speed: u16,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub geometry: Geometry,
    /// The video plays in the mini-player, and the browse screen shows.
    pub mini: bool,
    pub chapters: Vec<Chapter>,
    pub channel: Option<ChannelRef>,
    /// From yt-dlp, for the history.
    duration_secs: Option<u32>,
    /// A live stream: the history keeps no position for it.
    live: bool,
    /// The video is in the history. It goes in when mpv first loads it.
    recorded: bool,
    /// The position in the history, in seconds.
    saved_secs: u32,
    /// Unix time when the streams were found, for the history.
    watched_at: i64,
    pub qualities: Vec<Quality>,
    /// The size that plays now, in pixels on the short side.
    pub quality: Option<u32>,
    pub quality_menu: Option<QualityMenu>,
    /// The URL of the video stream that plays.
    video_url: String,
    /// A quality change that has not loaded yet, with what played before it.
    switch: Option<Switch>,
    captions: Vec<Caption>,
    /// The caption track that is shown, by name.
    pub caption: Option<String>,
    /// Videos that YouTube shows next to this one.
    related: Vec<Video>,
    /// Shown when the video has ended.
    pub up_next: Option<UpNext>,
    /// mpv has the caption track of `caption` for this file.
    caption_loaded: bool,
    /// The width and height of the video, for its shape on screen.
    aspect: (u32, u32),
    /// Kept for the end of the wait.
    streams: Option<Box<Streams>>,
    /// The streams were found again once after an HTTP 403 error.
    retried: bool,
    /// From SponsorBlock, as they came.
    segments: Vec<Segment>,
    /// The player has segments, so a list that is empty now must go to it too.
    segments_sent: bool,
}

impl PlayerState {
    /// Whether mpv runs, so that the video shows.
    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Loading | Phase::Playing | Phase::Ended)
    }

    /// Whether mpv has a file, so that the controls work. After the end, mpv
    /// has none.
    pub fn has_file(&self) -> bool {
        matches!(self.phase, Phase::Loading | Phase::Playing)
    }

    /// Changes the phase. The quality menu only stays while there is a file.
    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        if !self.has_file() {
            self.quality_menu = None;
        }
    }

    /// Seconds left to wait for the ads.
    pub fn wait_left(&self) -> Option<u64> {
        match self.phase {
            Phase::Waiting { until, now } => Some(until.saturating_sub(now)),
            _ => None,
        }
    }

    /// The chapter that plays now.
    pub fn chapter(&self) -> Option<&Chapter> {
        self.chapters.iter().rev().find(|c| u64::from(c.start_secs) * 1000 <= self.position_ms)
    }

    /// The start of the next chapter, or of the previous one, in seconds.
    fn chapter_seek(&self, forward: bool) -> Option<u32> {
        let starts = self.chapters.iter().map(|c| c.start_secs);
        let ms = |s: u32| u64::from(s) * 1000;
        if forward {
            starts.into_iter().find(|s| ms(*s) > self.position_ms)
        } else {
            // Before the first chapter, or in its first seconds, go to the start.
            let before = self.position_ms.saturating_sub(CHAPTER_GRACE_MS);
            if self.chapters.is_empty() {
                return None;
            }
            Some(starts.rev().find(|s| ms(*s) < before).unwrap_or(0))
        }
    }

    /// The SponsorBlock segments to skip in this video. mpv's length is
    /// exact. yt-dlp's comes first, before the file loads.
    pub fn skip_segments(&self) -> Vec<Segment> {
        if self.live {
            return Vec::new();
        }
        let duration = self.duration_ms.or(self.duration_secs.map(|s| u64::from(s) * 1000));
        sponsorblock::usable(&self.segments, duration)
    }

    /// The command that gives the player the segments to skip. mpv must
    /// have the file. The player ignores the same segments again.
    fn segments_command(&mut self) -> Option<Effect> {
        let segments = self.skip_segments();
        if !self.has_file() || (segments.is_empty() && !self.segments_sent) {
            return None;
        }
        self.segments_sent = true;
        // After an HTTP 403 error, a new player goes on from the old one.
        Some(Effect::Player(PlayerCommand::Segments { segments, continues: self.retried }))
    }

    /// The commands to show the caption track again after a file loads.
    fn reload_captions(&mut self) -> Vec<Effect> {
        self.caption_loaded = false;
        let Some(name) = &self.caption else { return Vec::new() };
        match self.captions.iter().find(|c| c.name == *name) {
            Some(c) => {
                self.caption_loaded = true;
                vec![Effect::Player(PlayerCommand::AddCaptions {
                    url: c.url.clone(),
                    lang: c.lang.clone(),
                    name: c.name.clone(),
                })]
            }
            None => Vec::new(),
        }
    }
}

impl App {
    /// Plays a video full size, or says why it cannot. The video that plays
    /// in the mini-player goes back to full size. After its end or a
    /// failure, it starts again.
    pub fn play(&mut self, id: String, title: String) -> Vec<Effect> {
        let alive = |p: &PlayerState| !matches!(p.phase, Phase::Ended | Phase::Failed(_));
        if self.player.as_ref().is_some_and(|p| p.mini && p.id == id && alive(p)) {
            return self.restore_player();
        }
        self.play_video(id, title, true, false)
    }

    /// Plays a video from a link, from `start_secs` if it is known. A link to
    /// the video that plays seeks in it, in the same size.
    pub(super) fn play_link(&mut self, id: String, title: String, start_secs: Option<u32>) -> Vec<Effect> {
        let secs = start_secs.unwrap_or(0);
        let same = self.player.as_mut().filter(|p| p.id == id);
        match same.as_ref().map(|p| &p.phase) {
            Some(Phase::Loading | Phase::Playing) => {
                self.status = Some(format!("Playing from {}.", clock(secs)));
                return vec![Effect::Player(PlayerCommand::SeekTo { secs })];
            }
            // mpv has no file yet: it starts there.
            Some(Phase::Resolving | Phase::Waiting { .. }) => {
                if let Some(p) = same {
                    p.position_ms = u64::from(secs) * 1000;
                }
                self.status = Some(format!("Playing from {}.", clock(secs)));
                return Vec::new();
            }
            _ => {}
        }
        // After its end or a failure, the same video starts again in the same
        // size, and does not become its own "previous video".
        let again = same.map(|p| p.mini);
        let effects = self.play_video(id, title, again.is_none(), again.unwrap_or(false));
        if let (Some(secs), Some(p)) = (start_secs, self.player.as_mut()) {
            p.position_ms = u64::from(secs) * 1000;
            self.status = Some(format!("Playing from {}.", clock(secs)));
        }
        effects
    }

    /// Plays a video. With `remember`, the video that plays now can come back with "previous video".
    fn play_video(&mut self, id: String, title: String, remember: bool, mini: bool) -> Vec<Effect> {
        if let Some(why) = self.graphics.message() {
            self.status = Some(why);
            return Vec::new();
        }
        let mut effects: Vec<Effect> = self.save_watch(false).into_iter().collect();
        if let Some(p) = &self.player {
            if remember {
                self.played.push(plain_video(&p.id, &p.title));
                if self.played.len() > PLAYED_MAX {
                    self.played.remove(0);
                }
            }
            effects.push(Effect::Player(PlayerCommand::Stop));
        } else {
            // A new run of the player: the history of the last one does not count.
            self.played.clear();
            self.forward.clear();
        }
        // A video in the queue that plays now leaves the queue, however it started.
        self.queue.retain(|v| v.id != id);
        self.forward.retain(|v| v.id != id);
        self.queue_selection.clamp(self.queue.len(), self.plain_rows());
        self.session += 1;
        let geometry = player_geometry(mini, self.width, self.height, self.cell, DEFAULT_ASPECT);
        let state = PlayerState {
            session: self.session,
            id,
            title,
            phase: Phase::Resolving,
            paused: false,
            buffering: false,
            volume: 100,
            muted: false,
            speed: 100,
            position_ms: 0,
            duration_ms: None,
            geometry,
            mini,
            chapters: Vec::new(),
            channel: None,
            duration_secs: None,
            live: false,
            recorded: false,
            saved_secs: 0,
            watched_at: 0,
            qualities: Vec::new(),
            quality: None,
            quality_menu: None,
            video_url: String::new(),
            switch: None,
            captions: Vec::new(),
            caption: None,
            related: Vec::new(),
            up_next: None,
            caption_loaded: false,
            aspect: DEFAULT_ASPECT,
            streams: None,
            retried: false,
            segments: Vec::new(),
            segments_sent: false,
        };
        let mut state = state;
        if let Some(secs) = self.resume_at(&state.id) {
            state.position_ms = u64::from(secs) * 1000;
            self.status = Some(format!("Resuming at {}.", clock(secs)));
        }
        effects.push(self.resolve(&state));
        effects.push(Effect::LoadRelated { session: state.session, id: state.id.clone() });
        if self.sponsorblock {
            effects.push(Effect::LoadSegments { session: state.session, id: state.id.clone() });
        }
        self.player = Some(state);
        effects
    }

    /// Where to resume a video from the history, if the user stopped in the
    /// middle. Only videos with a known length resume, so live streams start
    /// at the live edge.
    fn resume_at(&self, id: &str) -> Option<u32> {
        let e = self.history.iter().find(|e| e.video_id == id)?;
        let before_end = e.position_secs + RESUME_MARGIN_SECS < e.duration_secs?;
        (!e.finished && e.position_secs >= RESUME_MARGIN_SECS && before_end).then_some(e.position_secs)
    }

    /// Saves the video that plays in the history, if mpv has loaded it. After
    /// the end, it is saved as finished, and later saves do not undo that.
    pub(super) fn save_watch(&mut self, finished: bool) -> Option<Effect> {
        let p = self.player.as_mut().filter(|p| p.recorded)?;
        if !finished && p.phase == Phase::Ended {
            return None;
        }
        let position = if finished || p.live { 0 } else { u32::try_from(p.position_ms / 1000).unwrap_or(u32::MAX) };
        p.saved_secs = position;
        let entry = HistoryEntry {
            video_id: p.id.clone(),
            title: p.title.clone(),
            channel_id: p.channel.as_ref().and_then(|c| c.id.clone()),
            channel_name: p.channel.as_ref().map(|c| c.name.clone()),
            duration_secs: p.duration_secs.or_else(|| p.duration_ms.and_then(|ms| u32::try_from(ms / 1000).ok())),
            position_secs: position,
            finished,
            watched_at: p.watched_at,
        };
        self.remember_watch(&entry);
        Some(Effect::SaveWatch(entry))
    }

    /// The real title of the video. A video from a link starts with the link
    /// text or "Video <id>". The history gets the title again if it has the
    /// video already.
    pub(super) fn on_video_title(&mut self, session: u64, title: String) -> Vec<Effect> {
        let Some(p) = self.player_of(session) else { return Vec::new() };
        if title.is_empty() || p.title == title {
            return Vec::new();
        }
        p.title = title;
        // After the end, the history has the video as finished already.
        let finished = p.phase == Phase::Ended;
        self.save_watch(finished).into_iter().collect()
    }

    pub(super) fn on_related(&mut self, session: u64, result: Result<Vec<Video>, String>) -> Vec<Effect> {
        let Some(p) = self.player_of(session) else { return Vec::new() };
        match result {
            Ok(related) => p.related = related,
            Err(e) => tracing::debug!("no related videos: {e}"),
        }
        // They came after the end: fill in the up next list.
        if p.phase == Phase::Ended {
            return self.show_up_next(session);
        }
        Vec::new()
    }

    pub(super) fn on_segments(&mut self, session: u64, result: Result<Vec<Segment>, String>) -> Vec<Effect> {
        let Some(p) = self.player_of(session) else { return Vec::new() };
        match result {
            Ok(segments) => p.segments = segments,
            // The video plays anyway.
            Err(e) => tracing::warn!("no SponsorBlock segments: {e}"),
        }
        p.segments_command().into_iter().collect()
    }

    /// The videos that can play next: the ones that "previous video" went
    /// back from, the queue, then related videos that did not play yet. Live
    /// streams never play by themselves.
    fn next_videos(&self, p: &PlayerState) -> Vec<Video> {
        let listed = |id: &str| {
            id == p.id
                || self.played.iter().any(|v| v.id == id)
                || self.forward.iter().any(|v| v.id == id)
                || self.queue.iter().any(|v| v.id == id)
        };
        let related = p.related.iter().filter(|v| v.kind != VideoKind::Live && !listed(&v.id));
        // The mini-player lets the user queue the video that plays.
        let queue = self.queue.iter().filter(|v| v.id != p.id);
        self.forward.iter().rev().chain(queue).chain(related).take(UP_NEXT_MAX).cloned().collect()
    }

    /// Builds the up next list again after the queue changed. The browse
    /// screen can change the queue while the mini-player shows the end.
    pub(super) fn refresh_up_next(&mut self) {
        let Some(p) = self.player.as_ref().filter(|p| p.phase == Phase::Ended && p.up_next.is_some()) else {
            return;
        };
        let videos = self.next_videos(p);
        let Some(p) = &mut self.player else { return };
        if videos.is_empty() {
            p.up_next = None;
        } else if let Some(up) = &mut p.up_next {
            up.selection = up.selection.min(videos.len() - 1);
            up.videos = videos;
        }
    }

    /// Plays a video from the up next list, in the same size.
    fn play_next(&mut self, v: Video) -> Vec<Effect> {
        let mini = self.player.as_ref().is_some_and(|p| p.mini);
        self.play_video(v.id, v.title, true, mini)
    }

    fn on_ended(&mut self, session: u64) -> Vec<Effect> {
        let Some(p) = self.player_of(session).filter(|p| p.is_running()) else { return Vec::new() };
        p.set_phase(Phase::Ended);
        let mut effects: Vec<Effect> = self.save_watch(true).into_iter().collect();
        effects.extend(self.show_up_next(session));
        effects
    }

    /// Fills in the up next list after the end, and starts the countdown if
    /// it is new. A list that is there already keeps its selection.
    fn show_up_next(&mut self, session: u64) -> Vec<Effect> {
        let Some(p) = self.player.as_ref().filter(|p| p.session == session) else { return Vec::new() };
        let videos = self.next_videos(p);
        let autoplay = self.autoplay;
        let Some(p) = self.player_of(session) else { return Vec::new() };
        if videos.is_empty() {
            return Vec::new();
        }
        if let Some(up) = &mut p.up_next {
            up.selection = up.selection.min(videos.len() - 1);
            up.videos = videos;
            return Vec::new();
        }
        let countdown = autoplay.then_some(AUTOPLAY_SECS);
        p.up_next = Some(UpNext { videos, selection: 0, countdown });
        match countdown {
            Some(count) => vec![Effect::Ticks { session, count }],
            None => Vec::new(),
        }
    }

    /// Keys in the up next list. It moves with the browse keys.
    fn on_up_next_key(&mut self, key: KeyPress, action: Option<PlayerAction>) -> Vec<Effect> {
        let browse = self.keymap.action(key);
        let Some(up) = self.player.as_mut().and_then(|p| p.up_next.as_mut()) else { return Vec::new() };
        let last = up.videos.len().saturating_sub(1);
        let selection = match browse {
            Some(Action::Up) => up.selection.saturating_sub(1),
            Some(Action::Down) => (up.selection + 1).min(last),
            Some(Action::Top) => 0,
            Some(Action::Bottom) => last,
            _ => {
                if browse == Some(Action::Open) || action == Some(PlayerAction::NextVideo) {
                    let v = up.videos[up.selection].clone();
                    return self.play_next(v);
                }
                return Vec::new();
            }
        };
        up.selection = selection;
        // Choosing stops the countdown.
        up.countdown = None;
        Vec::new()
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
        let (language, captions_on) = (self.captions_language.clone(), self.captions_on);
        let Some(p) = self.player_of(session) else { return Vec::new() };
        if p.phase != Phase::Resolving {
            return Vec::new();
        }
        let streams = match result {
            Ok(s) => s,
            Err(e) => {
                p.set_phase(Phase::Failed(e));
                return Vec::new();
            }
        };
        p.video_url.clone_from(&streams.video_url);
        p.switch = None;
        if let (Some(w), Some(h)) = (streams.format.width, streams.format.height) {
            p.aspect = (w, h);
            p.geometry = player_geometry(p.mini, width, height, cell, p.aspect);
            p.quality = Some(w.min(h));
        }
        p.chapters.clone_from(&streams.chapters);
        p.channel.clone_from(&streams.channel);
        p.duration_secs = streams.duration_secs;
        p.live = streams.live;
        if p.watched_at == 0 {
            p.watched_at = i64::try_from(now).unwrap_or(i64::MAX);
        }
        p.qualities.clone_from(&streams.qualities);
        p.captions.clone_from(&streams.captions);
        // The configuration turns captions on once. After a 403 retry, they
        // stay as the user left them.
        if !p.retried && p.caption.is_none() && captions_on {
            p.caption = pick_caption(&p.captions, &language).map(|c| c.name.clone());
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
        if let Some(up) = p.up_next.as_mut().filter(|_| p.phase == Phase::Ended) {
            let Some(left) = up.countdown else { return Vec::new() };
            if left > 1 {
                up.countdown = Some(left - 1);
                return Vec::new();
            }
            let v = up.videos[up.selection].clone();
            return self.play_next(v);
        }
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
        if event == PlayerEvent::Ended {
            return self.on_ended(session);
        }
        let Some(p) = self.player_of(session) else { return Vec::new() };
        match event {
            PlayerEvent::FileLoaded => {
                if p.phase == Phase::Loading {
                    p.phase = Phase::Playing;
                }
                p.switch = None;
                // Caption tracks belong to one file, and a quality change loads a new one.
                let mut effects = p.reload_captions();
                // A new player after an HTTP 403 error has no segments yet.
                effects.extend(p.segments_command());
                if !p.recorded {
                    p.recorded = true;
                    effects.extend(self.save_watch(false));
                }
                return effects;
            }
            PlayerEvent::Position(ms) => {
                p.position_ms = ms;
                let secs = u32::try_from(ms / 1000).unwrap_or(u32::MAX);
                if p.recorded && secs.abs_diff(p.saved_secs) >= SAVE_EVERY_SECS {
                    return self.save_watch(false).into_iter().collect();
                }
            }
            PlayerEvent::Duration(ms) => {
                p.duration_ms = Some(ms);
                // The exact length can keep or drop other segments.
                return p.segments_command().into_iter().collect();
            }
            PlayerEvent::Paused(b) => p.paused = b,
            PlayerEvent::Buffering(b) => p.buffering = b,
            PlayerEvent::Volume(v) => p.volume = v,
            PlayerEvent::Muted(b) => p.muted = b,
            PlayerEvent::Speed(s) => p.speed = s,
            PlayerEvent::Skipped(category) => self.status = Some(format!("Skipped {}.", category.label())),
            PlayerEvent::CaptionsFailed(e) => {
                p.caption = None;
                p.caption_loaded = false;
                self.status = Some(format!("Cannot load the captions: {e}"));
            }
            PlayerEvent::Failed(e) if p.is_running() => {
                // A new size that does not load: go back to the one that played.
                if let Some(back) = p.switch.take() {
                    p.quality = back.quality;
                    p.video_url.clone_from(&back.url);
                    let secs = u32::try_from(p.position_ms / 1000).unwrap_or(u32::MAX);
                    self.status = Some(format!("Cannot play that size: {e}"));
                    return vec![Effect::Player(PlayerCommand::SwitchVideo { url: back.url, secs })];
                }
                // The URLs can expire, or the ads wait was too short. Find
                // them once more, and go on from the same place.
                if e.contains("403") && !p.retried {
                    p.retried = true;
                    p.set_phase(Phase::Resolving);
                    let resolve = self.player.as_ref().map(|p| self.resolve(p));
                    return [Some(Effect::Player(PlayerCommand::Stop)), resolve].into_iter().flatten().collect();
                }
                p.set_phase(Phase::Failed(e));
                return vec![Effect::Player(PlayerCommand::Stop)];
            }
            _ => {}
        }
        Vec::new()
    }

    pub(super) fn on_player_key(&mut self, key: KeyPress) -> Vec<Effect> {
        let action = self.keymap.player.action(key);
        if self.player.as_ref().is_some_and(|p| p.quality_menu.is_some() && p.has_file()) {
            return self.on_quality_menu_key(key, action);
        }
        match action {
            Some(PlayerAction::Back) => return self.stop_player(),
            Some(PlayerAction::Minimize) => return self.minimize(),
            _ => {}
        }
        if action != Some(PlayerAction::PrevVideo)
            && self.player.as_ref().is_some_and(|p| p.phase == Phase::Ended && p.up_next.is_some())
        {
            return self.on_up_next_key(key, action);
        }
        self.player_action(action)
    }

    /// Stops the player and goes back to browsing.
    pub(super) fn stop_player(&mut self) -> Vec<Effect> {
        let save = self.save_watch(false);
        if self.player.take().is_none() {
            return Vec::new();
        }
        save.into_iter().chain([Effect::Player(PlayerCommand::Stop)]).collect()
    }

    /// Shrinks the player to the mini-player. Playback goes on.
    pub(super) fn minimize(&mut self) -> Vec<Effect> {
        let Some(p) = &mut self.player else { return Vec::new() };
        p.mini = true;
        p.quality_menu = None;
        self.refit_player()
    }

    /// Shows the mini-player full size again.
    pub(super) fn restore_player(&mut self) -> Vec<Effect> {
        let Some(p) = &mut self.player else { return Vec::new() };
        p.mini = false;
        self.refit_player()
    }

    /// Keys while the mini-player plays and the browse screen shows. `None`
    /// when the key is for the browse screen.
    ///
    /// Typing and the help come first. Then the browse keys, and then the
    /// player keys that browse does not use.
    pub(super) fn on_mini_player_key(&mut self, key: KeyPress) -> Option<Vec<Effect>> {
        let typing = self.tab == Tab::Search && self.screens.is_empty() && self.search.captures_keys();
        if typing || self.show_help {
            return None;
        }
        match self.keymap.action(key) {
            Some(Action::ShowPlayer) => Some(self.restore_player()),
            Some(Action::ClosePlayer) => Some(self.stop_player()),
            Some(_) => None,
            None => match self.keymap.player.action(key)? {
                // These only make sense full size.
                PlayerAction::Back | PlayerAction::Minimize | PlayerAction::Quality => None,
                // The up next list is not on screen, so its first video plays.
                PlayerAction::NextVideo
                    if self.player.as_ref().is_some_and(|p| p.phase == Phase::Ended && p.up_next.is_some()) =>
                {
                    Some(self.on_up_next_key(key, Some(PlayerAction::NextVideo)))
                }
                action => Some(self.player_action(Some(action))),
            },
        }
    }

    /// Runs a player key that is not about the screen.
    fn player_action(&mut self, action: Option<PlayerAction>) -> Vec<Effect> {
        match action {
            Some(PlayerAction::NextVideo) => {
                let next = self.player.as_ref().map(|p| self.next_videos(p)).unwrap_or_default();
                if let Some(v) = next.into_iter().next() {
                    return self.play_next(v);
                }
                self.status = Some("Nothing to play next. Add videos to the queue with a.".into());
                return Vec::new();
            }
            Some(PlayerAction::PrevVideo) => {
                let Some(prev) = self.played.pop() else {
                    self.status = Some("No video played before this one.".into());
                    return Vec::new();
                };
                // The video that plays now comes next again, before the queue.
                if let Some(p) = &self.player {
                    let current = plain_video(&p.id, &p.title);
                    self.forward.push(current);
                }
                let mini = self.player.as_ref().is_some_and(|p| p.mini);
                return self.play_video(prev.id, prev.title, false, mini);
            }
            _ => {}
        }
        let language = self.captions_language.clone();
        let Some(p) = &mut self.player else { return Vec::new() };
        if !p.has_file() {
            return Vec::new();
        }
        let command = match action {
            Some(PlayerAction::SpeedUp) => PlayerCommand::Speed(SpeedStep::Up),
            Some(PlayerAction::SpeedDown) => PlayerCommand::Speed(SpeedStep::Down),
            Some(PlayerAction::SpeedReset) => PlayerCommand::Speed(SpeedStep::Reset),
            Some(a @ (PlayerAction::NextChapter | PlayerAction::PrevChapter)) => {
                let Some(secs) = p.chapter_seek(a == PlayerAction::NextChapter) else {
                    if p.chapters.is_empty() {
                        self.status = Some("This video has no chapters.".into());
                    }
                    return Vec::new();
                };
                PlayerCommand::SeekTo { secs }
            }
            Some(PlayerAction::Captions) => {
                if p.caption.take().is_some() {
                    return vec![Effect::Player(PlayerCommand::ShowCaptions(false))];
                }
                let Some(track) = pick_caption(&p.captions, &language) else {
                    self.status = Some(format!("This video has no captions in \"{language}\"."));
                    return Vec::new();
                };
                p.caption = Some(track.name.clone());
                if p.caption_loaded {
                    return vec![Effect::Player(PlayerCommand::ShowCaptions(true))];
                }
                return p.reload_captions();
            }
            Some(PlayerAction::Quality) => {
                if p.qualities.is_empty() {
                    self.status = Some("This video has no other sizes.".into());
                    return Vec::new();
                }
                let selection = p.qualities.iter().position(|q| Some(q.res) == p.quality).unwrap_or(0);
                p.quality_menu = Some(QualityMenu { selection });
                return Vec::new();
            }
            Some(PlayerAction::TogglePause) => PlayerCommand::TogglePause,
            Some(PlayerAction::SeekForward) => PlayerCommand::Seek { secs: SEEK_SECS },
            Some(PlayerAction::SeekBack) => PlayerCommand::Seek { secs: -SEEK_SECS },
            Some(PlayerAction::SeekForwardLong) => PlayerCommand::Seek { secs: SEEK_LONG_SECS },
            Some(PlayerAction::SeekBackLong) => PlayerCommand::Seek { secs: -SEEK_LONG_SECS },
            Some(PlayerAction::VolumeUp) => PlayerCommand::Volume { delta: VOLUME_STEP },
            Some(PlayerAction::VolumeDown) => PlayerCommand::Volume { delta: -VOLUME_STEP },
            Some(PlayerAction::Mute) => PlayerCommand::ToggleMute,
            Some(PlayerAction::Back | PlayerAction::Minimize | PlayerAction::NextVideo | PlayerAction::PrevVideo)
            | None => return Vec::new(),
        };
        vec![Effect::Player(command)]
    }

    /// Keys while the quality menu is open. It moves with the browse keys.
    fn on_quality_menu_key(&mut self, key: KeyPress, action: Option<PlayerAction>) -> Vec<Effect> {
        let browse = self.keymap.action(key);
        let Some(p) = &mut self.player else { return Vec::new() };
        let Some(menu) = &mut p.quality_menu else { return Vec::new() };
        let last = p.qualities.len().saturating_sub(1);
        match (browse, action) {
            (Some(Action::Up), _) => menu.selection = menu.selection.saturating_sub(1),
            (Some(Action::Down), _) => menu.selection = (menu.selection + 1).min(last),
            (Some(Action::Top), _) => menu.selection = 0,
            (Some(Action::Bottom), _) => menu.selection = last,
            (Some(Action::Open), _) => {
                let chosen = p.qualities.get(menu.selection).cloned();
                p.quality_menu = None;
                let Some(q) = chosen.filter(|q| Some(q.res) != p.quality) else { return Vec::new() };
                let previous = Switch { url: std::mem::replace(&mut p.video_url, q.url.clone()), quality: p.quality };
                // After a failed change, it goes back to the first size, not to the failed one.
                p.switch.get_or_insert(previous);
                p.quality = Some(q.res);
                let secs = u32::try_from(p.position_ms / 1000).unwrap_or(u32::MAX);
                return vec![Effect::Player(PlayerCommand::SwitchVideo { url: q.url, secs })];
            }
            (Some(Action::Back | Action::Quit), _)
            | (_, Some(PlayerAction::Back | PlayerAction::Minimize | PlayerAction::Quality)) => {
                p.quality_menu = None;
            }
            _ => {}
        }
        Vec::new()
    }

    /// Fits the video to a new screen or cell size, or to the other player size.
    pub(super) fn refit_player(&mut self) -> Vec<Effect> {
        let (width, height, cell) = (self.width, self.height, self.cell);
        let Some(p) = &mut self.player else { return Vec::new() };
        let geometry = player_geometry(p.mini, width, height, cell, p.aspect);
        if geometry == p.geometry {
            return Vec::new();
        }
        p.geometry = geometry;
        if p.is_running() { vec![Effect::Player(PlayerCommand::Resize(geometry))] } else { Vec::new() }
    }
}

/// `1:05`, or `1:02:05` from one hour.
pub fn clock(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// The video of a history entry, for the queue and the details screen.
pub(super) fn history_video(e: &HistoryEntry) -> Video {
    let mut v = plain_video(&e.video_id, &e.title);
    if let Some(name) = &e.channel_name {
        v.channels.push(ChannelRef { id: e.channel_id.clone(), name: name.clone(), handle: None });
    }
    v.duration_secs = e.duration_secs;
    v.duration_text = e.duration_secs.map(clock);
    v
}

/// The video of a details page, for the queue.
pub(super) fn details_video(d: &VideoDetails) -> Video {
    let mut v = plain_video(&d.id, &d.title);
    v.channels.extend(d.channel.clone());
    v.duration_secs = d.duration_secs;
    v.duration_text = d.duration_secs.map(clock);
    v.views_text.clone_from(&d.views_text);
    v.thumbnails.clone_from(&d.thumbnails);
    if d.live {
        v.kind = VideoKind::Live;
    }
    v
}

/// A video known only by its ID and title.
pub(super) fn plain_video(id: &str, title: &str) -> Video {
    Video {
        id: id.into(),
        title: title.into(),
        kind: VideoKind::Normal,
        channels: Vec::new(),
        duration_text: None,
        duration_secs: None,
        views_text: None,
        published_text: None,
        thumbnails: Vec::new(),
        members_only: false,
        badges: Vec::new(),
        description_snippet: None,
    }
}

fn start(p: &mut PlayerState, streams: Box<Streams>) -> Vec<Effect> {
    p.caption_loaded = false;
    p.phase = Phase::Loading;
    let start_secs = u32::try_from(p.position_ms / 1000).unwrap_or(0);
    vec![Effect::Player(PlayerCommand::Start { session: p.session, streams, geometry: p.geometry, start_secs })]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Input, Message};
    use crate::keymap::KeyMap;
    use crate::source::model::Item;
    use crate::source::streams::{Caption, FormatInfo, Quality};
    use crate::terminal::detect::Graphics;

    fn app() -> App {
        let mut a = App::new(KeyMap::default(), Graphics::KittyShm);
        a.update(Message::Input(Input::Resize { width: 80, height: 24 }));
        a.update(Message::Input(Input::CellSize { width: 10, height: 20 }));
        a
    }

    /// The effects without history saves, which have their own tests.
    fn no_saves(effects: Vec<Effect>) -> Vec<Effect> {
        effects.into_iter().filter(|e| !matches!(e, Effect::SaveWatch(_))).collect()
    }

    fn key(a: &mut App, s: &str) -> Vec<Effect> {
        no_saves(a.update(Message::Input(Input::Key(s.parse().unwrap()))))
    }

    fn streams(available_at: Option<u64>) -> Box<Streams> {
        Box::new(Streams {
            video_url: "https://v".into(),
            audio_url: Some("https://a".into()),
            headers: Vec::new(),
            available_at,
            expires_at: None,
            format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
            chapters: Vec::new(),
            channel: None,
            duration_secs: None,
            live: false,
            qualities: Vec::new(),
            captions: Vec::new(),
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
        assert_eq!(
            effects,
            [
                Effect::ResolveStreams { session: 1, id: "v1".into(), max_height: 480 },
                Effect::LoadRelated { session: 1, id: "v1".into() }
            ]
        );
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
            [Effect::Player(PlayerCommand::Start { session: 1, streams: streams(Some(115)), geometry, start_secs: 0 })]
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

    /// A playing video with chapters at 0, 60 and 120 s, three sizes, and English captions.
    fn playing(captions_on: bool) -> App {
        let mut a = app();
        a.captions_on = captions_on;
        a.play("v1".into(), "Title".into());
        let mut s = streams(None);
        s.chapters = [(0, "Intro"), (60, "Middle"), (120, "End")]
            .into_iter()
            .map(|(start_secs, title)| Chapter { title: title.into(), start_secs })
            .collect();
        s.qualities = [360, 480, 720].into_iter().map(|res| Quality { res, url: format!("https://v/{res}") }).collect();
        s.captions = vec![
            Caption { lang: "de".into(), name: "German".into(), url: "https://c/de".into(), auto: false },
            Caption {
                lang: "en-orig".into(),
                name: "English (Original)".into(),
                url: "https://c/en".into(),
                auto: true,
            },
        ];
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(s) });
        a
    }

    fn event(a: &mut App, event: PlayerEvent) -> Vec<Effect> {
        no_saves(a.update(Message::Player { session: 1, event }))
    }

    #[test]
    fn speed_keys() {
        let mut a = playing(false);
        assert_eq!(key(&mut a, "]"), [Effect::Player(PlayerCommand::Speed(SpeedStep::Up))]);
        assert_eq!(key(&mut a, "["), [Effect::Player(PlayerCommand::Speed(SpeedStep::Down))]);
        assert_eq!(key(&mut a, "backspace"), [Effect::Player(PlayerCommand::Speed(SpeedStep::Reset))]);
        event(&mut a, PlayerEvent::Speed(121));
        assert_eq!(a.player.as_ref().unwrap().speed, 121);
    }

    #[test]
    fn chapter_keys() {
        let mut a = playing(false);
        let seek = |secs| [Effect::Player(PlayerCommand::SeekTo { secs })];
        event(&mut a, PlayerEvent::Position(70_000));
        assert_eq!(a.player.as_ref().unwrap().chapter().unwrap().title, "Middle");
        assert_eq!(key(&mut a, "pgdn"), seek(120));
        assert_eq!(key(&mut a, "pgup"), seek(60));
        // In the first seconds of a chapter, back goes to the one before.
        event(&mut a, PlayerEvent::Position(61_000));
        assert_eq!(key(&mut a, "pgup"), seek(0));
        event(&mut a, PlayerEvent::Position(130_000));
        assert!(key(&mut a, "pgdn").is_empty());
        event(&mut a, PlayerEvent::Position(500));
        assert_eq!(key(&mut a, "pgup"), seek(0));

        let mut a = app();
        a.play("v2".into(), "Title".into());
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(streams(None)) });
        assert!(key(&mut a, "pgdn").is_empty());
        assert_eq!(a.status.as_deref(), Some("This video has no chapters."));
    }

    #[test]
    fn captions_toggle_and_reload() {
        let mut a = playing(false);
        // Nothing to load until the key.
        assert!(event(&mut a, PlayerEvent::FileLoaded).is_empty());
        let add = Effect::Player(PlayerCommand::AddCaptions {
            url: "https://c/en".into(),
            lang: "en-orig".into(),
            name: "English (Original)".into(),
        });
        assert_eq!(key(&mut a, "c"), std::slice::from_ref(&add));
        assert_eq!(a.player.as_ref().unwrap().caption.as_deref(), Some("English (Original)"));
        assert_eq!(key(&mut a, "c"), [Effect::Player(PlayerCommand::ShowCaptions(false))]);
        assert_eq!(key(&mut a, "c"), [Effect::Player(PlayerCommand::ShowCaptions(true))]);
        // A new file (a quality change) needs the track again.
        assert_eq!(event(&mut a, PlayerEvent::FileLoaded), std::slice::from_ref(&add));

        // On from the start, from the configuration file.
        let mut a = playing(true);
        assert_eq!(event(&mut a, PlayerEvent::FileLoaded), [add]);

        a.captions_language = "fr".into();
        key(&mut a, "c");
        assert!(key(&mut a, "c").is_empty());
        assert_eq!(a.status.as_deref(), Some("This video has no captions in \"fr\"."));
    }

    #[test]
    fn quality_menu() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::Position(42_500));
        assert!(key(&mut a, "v").is_empty());
        // The menu starts on the size that plays.
        assert_eq!(a.player.as_ref().unwrap().quality_menu, Some(QualityMenu { selection: 2 }));
        // The arrows move in the menu, and do not seek.
        assert!(key(&mut a, "up").is_empty());
        assert!(key(&mut a, "k").is_empty());
        assert!(key(&mut a, "k").is_empty());
        assert_eq!(a.player.as_ref().unwrap().quality_menu, Some(QualityMenu { selection: 0 }));
        let switch = key(&mut a, "enter");
        assert_eq!(switch, [Effect::Player(PlayerCommand::SwitchVideo { url: "https://v/360".into(), secs: 42 })]);
        let p = a.player.as_ref().unwrap();
        assert_eq!((p.quality, p.quality_menu.clone()), (Some(360), None));
        // The same size again does nothing, and q closes the menu but not the player.
        key(&mut a, "v");
        assert!(key(&mut a, "enter").is_empty());
        key(&mut a, "v");
        assert!(key(&mut a, "q").is_empty());
        assert!(a.player.as_ref().unwrap().quality_menu.is_none());
        assert!(a.player.is_some());

        let mut a = app();
        a.play("v2".into(), "Title".into());
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(streams(None)) });
        key(&mut a, "v");
        assert_eq!(a.player.as_ref().unwrap().quality_menu, None);
        assert_eq!(a.status.as_deref(), Some("This video has no other sizes."));
    }

    #[test]
    fn a_failed_caption_load_can_be_tried_again() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(matches!(key(&mut a, "c").as_slice(), [Effect::Player(PlayerCommand::AddCaptions { .. })]));
        event(&mut a, PlayerEvent::CaptionsFailed("HTTP error 429".into()));
        assert_eq!(a.player.as_ref().unwrap().caption, None);
        assert_eq!(a.status.as_deref(), Some("Cannot load the captions: HTTP error 429"));
        // The next key loads the track again, not only shows it.
        assert!(matches!(key(&mut a, "c").as_slice(), [Effect::Player(PlayerCommand::AddCaptions { .. })]));
    }

    #[test]
    fn a_failed_quality_change_goes_back() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        event(&mut a, PlayerEvent::Position(42_000));
        key(&mut a, "v");
        key(&mut a, "k");
        assert!(matches!(key(&mut a, "enter").as_slice(), [Effect::Player(PlayerCommand::SwitchVideo { .. })]));
        assert_eq!(a.player.as_ref().unwrap().quality, Some(480));
        // The new size fails: back to the stream that played, at the same place.
        let effects = event(&mut a, PlayerEvent::Failed("HTTP error 403".into()));
        assert_eq!(effects, [Effect::Player(PlayerCommand::SwitchVideo { url: "https://v".into(), secs: 42 })]);
        let p = a.player.as_ref().unwrap();
        assert_eq!((p.quality, &p.phase), (Some(720), &Phase::Playing));
        assert!(a.status.as_deref().unwrap().starts_with("Cannot play that size"));
        // A change that loads is not undone by a later failure.
        key(&mut a, "v");
        key(&mut a, "k");
        key(&mut a, "enter");
        event(&mut a, PlayerEvent::FileLoaded);
        let effects = event(&mut a, PlayerEvent::Failed("HTTP error 403".into()));
        assert!(matches!(effects.as_slice(), [Effect::Player(PlayerCommand::Stop), Effect::ResolveStreams { .. }]));
    }

    #[test]
    fn a_403_retry_goes_on_from_the_same_place_and_keeps_captions_off() {
        let mut a = playing(true);
        event(&mut a, PlayerEvent::FileLoaded);
        // The user turns off the captions that the configuration turned on.
        assert_eq!(key(&mut a, "c"), [Effect::Player(PlayerCommand::ShowCaptions(false))]);
        event(&mut a, PlayerEvent::Position(90_500));
        event(&mut a, PlayerEvent::Failed("HTTP error 403".into()));
        let effects = a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(streams(None)) });
        assert!(matches!(effects.as_slice(), [Effect::Player(PlayerCommand::Start { start_secs: 90, .. })]));
        assert_eq!(a.player.as_ref().unwrap().caption, None);
        assert!(event(&mut a, PlayerEvent::FileLoaded).is_empty());
    }

    #[test]
    fn no_controls_after_the_end_or_a_failure() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        key(&mut a, "v");
        event(&mut a, PlayerEvent::Ended);
        // The menu closed with the file, so q leaves the player at once.
        assert_eq!(a.player.as_ref().unwrap().quality_menu, None);
        for k in ["v", "c", "pgup", "space", "right", "]"] {
            assert!(key(&mut a, k).is_empty(), "{k}");
        }
        assert_eq!(key(&mut a, "q"), [Effect::Player(PlayerCommand::Stop)]);

        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        key(&mut a, "v");
        event(&mut a, PlayerEvent::Failed("demuxer error".into()));
        assert_eq!(key(&mut a, "q"), [Effect::Player(PlayerCommand::Stop)]);
    }

    #[test]
    fn previous_chapter_never_goes_forward() {
        let mut a = playing(false);
        a.player.as_mut().unwrap().chapters = [(10, "First"), (60, "Second")]
            .into_iter()
            .map(|(start_secs, title)| Chapter { title: title.into(), start_secs })
            .collect();
        event(&mut a, PlayerEvent::Position(5_000));
        assert_eq!(key(&mut a, "pgup"), [Effect::Player(PlayerCommand::SeekTo { secs: 0 })]);
        event(&mut a, PlayerEvent::Position(11_000));
        assert_eq!(key(&mut a, "pgup"), [Effect::Player(PlayerCommand::SeekTo { secs: 0 })]);
        event(&mut a, PlayerEvent::Position(30_000));
        assert_eq!(key(&mut a, "pgup"), [Effect::Player(PlayerCommand::SeekTo { secs: 10 })]);
    }

    fn related(ids: &[&str]) -> Vec<Video> {
        ids.iter().map(|id| plain_video(id, &format!("Video {id}"))).collect()
    }

    fn up_next_ids(a: &App) -> Vec<String> {
        a.player.as_ref().unwrap().up_next.as_ref().unwrap().videos.iter().map(|v| v.id.clone()).collect()
    }

    fn resolves(effects: &[Effect], want: &str) -> bool {
        let effects = no_saves(effects.to_vec());
        matches!(effects.as_slice(), [Effect::Player(PlayerCommand::Stop), Effect::ResolveStreams { id, .. }, Effect::LoadRelated { .. }] if id == want)
    }

    #[test]
    fn autoplay_counts_down_and_plays_the_queue_first() {
        let mut a = playing(false);
        a.queue = related(&["q1"]);
        let mut live = plain_video("live", "Radio");
        live.kind = VideoKind::Live;
        let mut list = related(&["v1", "r1"]);
        list.insert(0, live);
        a.update(Message::RelatedLoaded { session: 1, result: Ok(list) });
        event(&mut a, PlayerEvent::FileLoaded);
        let effects = event(&mut a, PlayerEvent::Ended);
        assert_eq!(effects, [Effect::Ticks { session: 1, count: AUTOPLAY_SECS }]);
        // The queue first. Not the video that played, and no live streams.
        assert_eq!(up_next_ids(&a), ["q1", "r1"]);
        for left in (1..AUTOPLAY_SECS).rev() {
            assert!(a.update(Message::Tick { session: 1, now: 0 }).is_empty());
            assert_eq!(a.player.as_ref().unwrap().up_next.as_ref().unwrap().countdown, Some(left));
        }
        let effects = a.update(Message::Tick { session: 1, now: 0 });
        assert!(resolves(&effects, "q1"), "{effects:?}");
        assert!(a.queue.is_empty(), "a played video leaves the queue");
    }

    #[test]
    fn choosing_in_up_next_stops_the_countdown() {
        let mut a = playing(false);
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1", "r2"])) });
        event(&mut a, PlayerEvent::FileLoaded);
        event(&mut a, PlayerEvent::Ended);
        assert!(key(&mut a, "j").is_empty());
        let up = a.player.as_ref().unwrap().up_next.clone().unwrap();
        assert_eq!((up.selection, up.countdown), (1, None));
        assert!(a.update(Message::Tick { session: 1, now: 0 }).is_empty());
        assert!(resolves(&key(&mut a, "enter"), "r2"));
    }

    #[test]
    fn no_autoplay_when_it_is_off_or_nothing_is_next() {
        let mut a = playing(false);
        a.autoplay = false;
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1"])) });
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(event(&mut a, PlayerEvent::Ended).is_empty());
        assert_eq!(a.player.as_ref().unwrap().up_next.as_ref().unwrap().countdown, None);

        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(event(&mut a, PlayerEvent::Ended).is_empty());
        assert_eq!(a.player.as_ref().unwrap().up_next, None);
        assert_eq!(phase(&a), Phase::Ended);
    }

    #[test]
    fn next_and_previous_video() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(key(&mut a, ">").is_empty());
        assert!(a.status.as_deref().unwrap().starts_with("Nothing to play next"));
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1"])) });
        assert!(resolves(&key(&mut a, ">"), "r1"));
        // Back to v1. r1 comes next again, and the user's queue stays as it was.
        assert!(resolves(&key(&mut a, "<"), "v1"));
        assert!(a.queue.is_empty());
        assert!(key(&mut a, "<").is_empty());
        assert_eq!(a.status.as_deref(), Some("No video played before this one."));
        assert!(resolves(&key(&mut a, ">"), "r1"));
    }

    #[test]
    fn a_queued_video_that_plays_leaves_the_queue() {
        let mut a = app();
        a.queue = related(&["v1", "q2"]);
        a.play("v1".into(), "Video v1".into());
        assert_eq!(a.queue.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["q2"]);
    }

    #[test]
    fn related_videos_of_an_old_session_are_dropped() {
        let mut a = playing(false);
        a.update(Message::RelatedLoaded { session: 0, result: Ok(related(&["r1"])) });
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(event(&mut a, PlayerEvent::Ended).is_empty());
    }

    #[test]
    fn the_history_lasts_until_the_player_closes() {
        let mut a = playing(false);
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1"])) });
        key(&mut a, ">");
        key(&mut a, "q");
        a.play("x".into(), "Video x".into());
        assert!(key(&mut a, "<").is_empty());
        assert_eq!(a.status.as_deref(), Some("No video played before this one."));
    }

    #[test]
    fn previous_works_in_up_next() {
        let mut a = playing(false);
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1", "r2"])) });
        key(&mut a, ">");
        let session = a.player.as_ref().unwrap().session;
        a.update(Message::RelatedLoaded { session, result: Ok(related(&["r2"])) });
        a.update(Message::StreamsResolved { session, now: 0, result: Ok(streams(None)) });
        a.update(Message::Player { session, event: PlayerEvent::FileLoaded });
        a.update(Message::Player { session, event: PlayerEvent::Ended });
        assert!(a.player.as_ref().unwrap().up_next.is_some());
        assert!(resolves(&key(&mut a, "<"), "v1"));
        // r1 comes first in the up next list of v1, before the related videos.
        assert!(resolves(&key(&mut a, ">"), "r1"));
    }

    #[test]
    fn related_videos_after_the_end_make_the_up_next_list() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(event(&mut a, PlayerEvent::Ended).is_empty());
        let effects = a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1"])) });
        assert_eq!(effects, [Effect::Ticks { session: 1, count: AUTOPLAY_SECS }]);
        assert_eq!(up_next_ids(&a), ["r1"]);
    }

    fn saves(effects: &[Effect]) -> Vec<HistoryEntry> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::SaveWatch(h) => Some(h.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_history_records_and_keeps_the_place() {
        let mut a = playing(false);
        // Not in the history until mpv loads the video.
        assert!(saves(&a.update(Message::Player { session: 1, event: PlayerEvent::Position(5_000) })).is_empty());
        a.update(Message::Player { session: 1, event: PlayerEvent::Duration(600_000) });
        let s = saves(&a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded }));
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].video_id.as_str(), s[0].position_secs, s[0].finished), ("v1", 5, false));
        assert_eq!(a.history.len(), 1);
        // Every 30 s of play, not every second.
        assert!(saves(&a.update(Message::Player { session: 1, event: PlayerEvent::Position(20_000) })).is_empty());
        let s = saves(&a.update(Message::Player { session: 1, event: PlayerEvent::Position(36_000) }));
        assert_eq!(s[0].position_secs, 36);
        // Back saves where the user stopped.
        a.update(Message::Player { session: 1, event: PlayerEvent::Position(90_000) });
        let s = saves(&a.update(Message::Input(Input::Key("q".parse().unwrap()))));
        assert_eq!(s[0].position_secs, 90);
        assert_eq!(a.history[0].position_secs, 90);

        // Playing it again resumes there.
        let effects = a.play("v1".into(), "Title".into());
        assert_eq!(a.status.as_deref(), Some("Resuming at 1:30."));
        assert_eq!(a.player.as_ref().unwrap().position_ms, 90_000);
        assert!(saves(&effects).is_empty());
        let start = a.update(Message::StreamsResolved { session: 2, now: 0, result: Ok(streams(None)) });
        assert!(matches!(start.as_slice(), [Effect::Player(PlayerCommand::Start { start_secs: 90, .. })]));
    }

    #[test]
    fn a_finished_video_starts_from_the_beginning() {
        let mut a = playing(false);
        a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded });
        a.update(Message::Player { session: 1, event: PlayerEvent::Position(600_000) });
        let s = saves(&a.update(Message::Player { session: 1, event: PlayerEvent::Ended }));
        assert_eq!((s[0].position_secs, s[0].finished), (0, true));
        // Back after the end does not save the video as unfinished.
        assert!(saves(&a.update(Message::Input(Input::Key("q".parse().unwrap())))).is_empty());
        a.play("v1".into(), "Title".into());
        assert_eq!(a.player.as_ref().unwrap().position_ms, 0);
        assert_eq!(a.status, None);
    }

    #[test]
    fn no_resume_near_the_start_or_the_end() {
        let mut a = app();
        let entry = |position_secs| HistoryEntry {
            video_id: "v1".into(),
            title: "T".into(),
            channel_id: None,
            channel_name: None,
            duration_secs: Some(600),
            position_secs,
            finished: false,
            watched_at: 1,
        };
        for (position, resume) in [(10, None), (30, Some(30)), (569, Some(569)), (570, None)] {
            a.history = vec![entry(position)];
            assert_eq!(a.resume_at("v1"), resume, "{position}");
        }
        // No length, as for a live stream: no resume.
        a.history = vec![HistoryEntry { duration_secs: None, ..entry(300) }];
        assert_eq!(a.resume_at("v1"), None);
    }

    #[test]
    fn quitting_and_another_video_save_the_place() {
        let mut a = playing(false);
        a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded });
        a.update(Message::Player { session: 1, event: PlayerEvent::Position(45_000) });
        let effects = a.update(Message::Input(Input::Key("ctrl-c".parse().unwrap())));
        assert!(matches!(effects.as_slice(), [Effect::SaveWatch(h), Effect::Quit] if h.position_secs == 45));
        let effects = a.update(Message::Terminate);
        assert!(matches!(effects.as_slice(), [Effect::SaveWatch(_), Effect::Quit]));
        let effects = a.play("v2".into(), "Other".into());
        assert!(matches!(effects.first(), Some(Effect::SaveWatch(h)) if h.video_id == "v1"));
    }

    #[test]
    fn a_finished_video_stays_finished() {
        let ended = || {
            let mut a = playing(false);
            a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1"])) });
            a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded });
            a.update(Message::Player { session: 1, event: PlayerEvent::Position(599_000) });
            a.update(Message::Player { session: 1, event: PlayerEvent::Ended });
            a
        };
        // Autoplay, quit, and a late position after the end save nothing more.
        let mut a = ended();
        let mut effects = Vec::new();
        for _ in 0..AUTOPLAY_SECS {
            effects.extend(a.update(Message::Tick { session: 1, now: 0 }));
        }
        assert!(saves(&effects).is_empty(), "{effects:?}");
        assert!(a.history.iter().any(|h| h.video_id == "v1" && h.finished));
        let mut a = ended();
        assert!(saves(&a.update(Message::Input(Input::Key("ctrl-c".parse().unwrap())))).is_empty());
        let mut a = ended();
        assert!(saves(&a.update(Message::Player { session: 1, event: PlayerEvent::Position(600_000) })).is_empty());
    }

    #[test]
    fn a_crash_or_lost_input_saves_the_place() {
        let mut a = playing(false);
        a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded });
        a.update(Message::Player { session: 1, event: PlayerEvent::Position(45_000) });
        let effects = a.update(Message::InputClosed("EIO".into()));
        assert!(matches!(effects.as_slice(), [Effect::SaveWatch(h), Effect::Fail(_)] if h.position_secs == 45));
        let effects = a.update(Message::Player { session: 1, event: PlayerEvent::Crashed("render".into()) });
        assert!(matches!(effects.as_slice(), [Effect::SaveWatch(_), Effect::Fail(_)]));
    }

    #[test]
    fn live_streams_keep_no_position() {
        let mut a = app();
        a.play("live".into(), "Radio".into());
        let mut s = streams(None);
        s.live = true;
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(s) });
        a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded });
        a.update(Message::Player { session: 1, event: PlayerEvent::Position(300_000) });
        let s = saves(&a.update(Message::Input(Input::Key("q".parse().unwrap()))));
        assert_eq!(s[0].position_secs, 0);
    }

    fn sponsor(start_ms: u64, end_ms: u64) -> Segment {
        Segment { category: Category::Sponsor, start_ms, end_ms, video_duration_ms: None }
    }

    fn segments_loaded(a: &mut App, session: u64, result: Result<Vec<Segment>, String>) -> Vec<Effect> {
        a.update(Message::SegmentsLoaded { session, result })
    }

    #[test]
    fn segments_load_only_when_sponsorblock_is_on() {
        let mut a = app();
        assert!(!a.play("v1".into(), "T".into()).iter().any(|e| matches!(e, Effect::LoadSegments { .. })));
        a.sponsorblock = true;
        let effects = a.play("v2".into(), "T".into());
        assert!(effects.contains(&Effect::LoadSegments { session: 2, id: "v2".into() }), "{effects:?}");
    }

    #[test]
    fn segments_go_to_the_player_once_it_has_the_file() {
        let mut a = app();
        a.sponsorblock = true;
        a.play("v1".into(), "T".into());
        // They come while yt-dlp still looks for the streams.
        assert!(segments_loaded(&mut a, 1, Ok(vec![sponsor(10_000, 20_000)])).is_empty());
        let mut s = streams(None);
        s.duration_secs = Some(100);
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(s) });
        let send =
            Effect::Player(PlayerCommand::Segments { segments: vec![sponsor(10_000, 20_000)], continues: false });
        assert!(event(&mut a, PlayerEvent::FileLoaded).contains(&send));
        // They come after the file loaded.
        assert_eq!(segments_loaded(&mut a, 1, Ok(vec![sponsor(10_000, 20_000)])), [send]);
        // mpv's exact length: a segment that ends 0.5 s before the end ends there.
        assert_eq!(
            segments_loaded(&mut a, 1, Ok(vec![sponsor(90_000, 99_700)])),
            [Effect::Player(PlayerCommand::Segments { segments: vec![sponsor(90_000, 100_000)], continues: false })]
        );
        assert_eq!(
            event(&mut a, PlayerEvent::Duration(100_200)),
            [Effect::Player(PlayerCommand::Segments { segments: vec![sponsor(90_000, 100_200)], continues: false })]
        );
    }

    #[test]
    fn segments_of_an_old_session_or_errors_change_nothing() {
        let mut a = playing(false);
        a.sponsorblock = true;
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(segments_loaded(&mut a, 0, Ok(vec![sponsor(1000, 2000)])).is_empty());
        assert!(a.player.as_ref().unwrap().skip_segments().is_empty());
        assert!(segments_loaded(&mut a, 1, Err("timed out".into())).is_empty());
        assert_eq!(a.status, None);
        assert_eq!(phase(&a), Phase::Playing);
    }

    #[test]
    fn an_empty_list_reaches_the_player_that_has_segments() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        // An upload of another length: yt-dlp has no length, mpv's drops it.
        let old = Segment { video_duration_ms: Some(110_000), ..sponsor(10_000, 20_000) };
        assert_eq!(segments_loaded(&mut a, 1, Ok(vec![old.clone()])).len(), 1);
        assert_eq!(
            event(&mut a, PlayerEvent::Duration(100_000)),
            [Effect::Player(PlayerCommand::Segments { segments: Vec::new(), continues: false })]
        );
        // A player that never had segments gets no empty list.
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        a.update(Message::Player { session: 1, event: PlayerEvent::Duration(100_000) });
        assert!(segments_loaded(&mut a, 1, Ok(vec![old])).is_empty());
    }

    #[test]
    fn live_streams_skip_nothing() {
        let mut a = app();
        a.sponsorblock = true;
        a.play("live".into(), "Radio".into());
        let mut s = streams(None);
        s.live = true;
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(s) });
        event(&mut a, PlayerEvent::FileLoaded);
        assert!(segments_loaded(&mut a, 1, Ok(vec![sponsor(1000, 2000)])).is_empty());
    }

    #[test]
    fn a_skip_shows_in_the_status_and_a_new_player_gets_the_segments() {
        let mut a = playing(false);
        event(&mut a, PlayerEvent::FileLoaded);
        segments_loaded(&mut a, 1, Ok(vec![sponsor(10_000, 20_000)]));
        event(&mut a, PlayerEvent::Skipped(Category::Sponsor));
        assert_eq!(a.status.as_deref(), Some("Skipped sponsor."));
        // After an HTTP 403 error, the new player needs them again.
        event(&mut a, PlayerEvent::Failed("HTTP error 403".into()));
        a.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(streams(None)) });
        let send = Effect::Player(PlayerCommand::Segments { segments: vec![sponsor(10_000, 20_000)], continues: true });
        assert_eq!(event(&mut a, PlayerEvent::FileLoaded), [send]);
    }

    #[test]
    fn fit_area_is_fit_without_the_status_rows() {
        assert_eq!(Geometry::fit_area(80, 22, (10, 20), (16, 9)), Geometry::fit(80, 24, (10, 20), (16, 9)));
    }

    #[test]
    fn the_mini_player_docks_at_the_bottom_right() {
        // 120 x 40: the dock is a third of the width, under the tab row and above the status bar.
        let (dock, g) = mini_layout(120, 40, (10, 20), (16, 9)).unwrap();
        assert_eq!(dock, Dock { x: 80, y: 1, cols: 40, rows: 38 });
        // 38 cells of 10 px is 380 px wide, so 214 px high: 11 rows.
        assert_eq!((g.cols, g.rows), (38, 11));
        assert_eq!(g.x, 81);
        // The box ends above the 3 info rows, on the last row of the dock.
        assert_eq!(g.y + g.rows + 1 + MINI_INFO_ROWS, dock.y + dock.rows);
        // A portrait video takes at most half of the dock.
        let (dock, g) = mini_layout(120, 40, (10, 20), (9, 16)).unwrap();
        assert!(g.rows <= dock.rows / 2, "{g:?}");
        assert!(g.x > dock.x && g.x + g.cols < dock.x + dock.cols, "{g:?}");
        // The widest dock, and the smallest screen with one.
        assert_eq!(mini_layout(300, 60, (10, 20), (16, 9)).unwrap().0.cols, 64);
        let (dock, g) = mini_layout(72, 14, (10, 20), (16, 9)).unwrap();
        assert_eq!(dock.cols, 24);
        assert!(g.y > dock.y, "{g:?}");
        assert_eq!(mini_layout(71, 40, (10, 20), (16, 9)), None);
        assert_eq!(mini_layout(120, 13, (10, 20), (16, 9)), None);
    }

    /// A playing video, shrunk to the mini-player on a 120 x 40 screen.
    fn mini() -> App {
        let mut a = playing(false);
        a.update(Message::Input(Input::Resize { width: 120, height: 40 }));
        event(&mut a, PlayerEvent::FileLoaded);
        let (_, g) = mini_layout(120, 40, (10, 20), (16, 9)).unwrap();
        assert_eq!(key(&mut a, "esc"), [Effect::Player(PlayerCommand::Resize(g))]);
        let p = a.player.as_ref().unwrap();
        assert!(p.mini);
        assert_eq!(p.geometry, g);
        a
    }

    #[test]
    fn esc_shrinks_the_player_and_playback_goes_on() {
        let mut a = playing(false);
        key(&mut a, "v");
        assert!(a.player.as_ref().unwrap().quality_menu.is_some());
        // In the quality menu, esc only closes the menu.
        assert!(key(&mut a, "esc").is_empty());
        assert!(!a.player.as_ref().unwrap().mini);
        let a = mini();
        assert_eq!(a.player.as_ref().unwrap().phase, Phase::Playing);
        // Before mpv runs, only the state changes.
        let mut a = app();
        a.play("v1".into(), "Title".into());
        assert!(key(&mut a, "esc").is_empty());
        assert!(a.player.as_ref().unwrap().mini);
    }

    #[test]
    fn mini_player_keys_come_after_the_browse_keys() {
        let mut a = mini();
        a.search.results = related(&["r1", "r2"]).into_iter().map(Item::Video).collect();
        // Browse keys move in the list.
        assert!(key(&mut a, "j").is_empty());
        assert_eq!(a.search.selection.index, 1);
        // Player keys that browse does not use control the video.
        assert_eq!(key(&mut a, "space"), [Effect::Player(PlayerCommand::TogglePause)]);
        assert_eq!(key(&mut a, "right"), [Effect::Player(PlayerCommand::Seek { secs: 5 })]);
        assert_eq!(key(&mut a, "m"), [Effect::Player(PlayerCommand::ToggleMute)]);
        // The quality menu needs the full player.
        assert!(key(&mut a, "v").iter().all(|e| matches!(e, Effect::Player(PlayerCommand::Resize(_)))));
        assert!(!a.player.as_ref().unwrap().mini, "v is the browse key for the full player");
        assert!(a.player.as_ref().unwrap().quality_menu.is_none());
        let mut a = mini();
        // / opens the search line, and keys go into it while it is open.
        assert!(key(&mut a, "/").is_empty());
        assert!(key(&mut a, "space").is_empty());
        assert!(key(&mut a, "x").is_empty());
        assert_eq!(a.search.input.as_deref(), Some(" x"));
        assert!(a.player.is_some());
        // The help comes first too.
        key(&mut a, "esc");
        key(&mut a, "?");
        assert!(key(&mut a, "space").is_empty());
    }

    #[test]
    fn v_shows_the_full_player_and_x_stops_it() {
        let mut a = mini();
        let full = Geometry::fit(120, 40, (10, 20), (16, 9));
        assert_eq!(key(&mut a, "v"), [Effect::Player(PlayerCommand::Resize(full))]);
        assert!(!a.player.as_ref().unwrap().mini);
        key(&mut a, "esc");
        let effects = a.update(Message::Input(Input::Key("x".parse().unwrap())));
        assert!(
            matches!(effects.as_slice(), [Effect::SaveWatch(_), Effect::Player(PlayerCommand::Stop)]),
            "{effects:?}"
        );
        assert_eq!(a.player, None);
        // Without a player, the keys do nothing.
        assert!(key(&mut a, "x").is_empty());
        assert!(key(&mut a, "v").is_empty());
        // q in browse quits, and keeps the place in the video.
        let mut a = mini();
        let effects = a.update(Message::Input(Input::Key("q".parse().unwrap())));
        assert!(matches!(effects.as_slice(), [Effect::SaveWatch(_), Effect::Quit]), "{effects:?}");
    }

    #[test]
    fn opening_a_video_from_the_mini_player() {
        // The video that plays goes back to full size, with no restart.
        let mut a = mini();
        a.search.results = vec![Item::Video(plain_video("v1", "Title")), Item::Video(plain_video("v2", "Other"))];
        assert!(matches!(key(&mut a, "enter").as_slice(), [Effect::Player(PlayerCommand::Resize(_))]));
        assert!(!a.player.as_ref().unwrap().mini);
        assert_eq!(a.player.as_ref().unwrap().session, 1);
        // Another video replaces it, full size.
        key(&mut a, "esc");
        key(&mut a, "j");
        let effects = key(&mut a, "enter");
        assert!(resolves(&effects, "v2"), "{effects:?}");
        assert!(!a.player.as_ref().unwrap().mini);
    }

    #[test]
    fn the_next_video_stays_in_the_mini_player() {
        let mut a = mini();
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1", "r2"])) });
        event(&mut a, PlayerEvent::Ended);
        for _ in 1..AUTOPLAY_SECS {
            a.update(Message::Tick { session: 1, now: 0 });
        }
        let effects = a.update(Message::Tick { session: 1, now: 0 });
        assert!(resolves(&effects, "r1"), "{effects:?}");
        let p = a.player.as_ref().unwrap();
        assert!(p.mini);
        assert_eq!(p.geometry, mini_layout(120, 40, (10, 20), (16, 9)).unwrap().1);
        let effects = a.update(Message::StreamsResolved { session: 2, now: 0, result: Ok(streams(None)) });
        let g = a.player.as_ref().unwrap().geometry;
        assert_eq!(g, mini_layout(120, 40, (10, 20), (16, 9)).unwrap().1);
        assert!(
            matches!(effects.as_slice(), [Effect::Player(PlayerCommand::Start { geometry, .. })] if *geometry == g)
        );
        // > and < keep the size too.
        a.update(Message::Player { session: 2, event: PlayerEvent::FileLoaded });
        let effects = key(&mut a, "<");
        assert!(resolves(&effects, "v1"), "{effects:?}");
        assert!(a.player.as_ref().unwrap().mini);
        let effects = key(&mut a, ">");
        assert!(resolves(&effects, "r1"), "{effects:?}");
        assert!(a.player.as_ref().unwrap().mini);
    }

    #[test]
    fn next_after_the_end_plays_the_first_video_in_up_next() {
        let mut a = mini();
        a.autoplay = false;
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1", "r2"])) });
        event(&mut a, PlayerEvent::Ended);
        let effects = key(&mut a, ">");
        assert!(resolves(&effects, "r1"), "{effects:?}");
        assert!(a.player.as_ref().unwrap().mini);
    }

    #[test]
    fn queue_changes_after_the_end_change_up_next() {
        let mut a = mini();
        a.queue = related(&["q1", "q2"]);
        a.update(Message::RelatedLoaded { session: 1, result: Ok(related(&["r1"])) });
        event(&mut a, PlayerEvent::Ended);
        assert_eq!(up_next_ids(&a), ["q1", "q2", "r1"]);
        // d on the first video in the Queue tab.
        a.tab = Tab::Queue;
        key(&mut a, "d");
        assert_eq!(up_next_ids(&a), ["q2", "r1"]);
        for _ in 1..AUTOPLAY_SECS {
            a.update(Message::Tick { session: 1, now: 0 });
        }
        let effects = a.update(Message::Tick { session: 1, now: 0 });
        assert!(resolves(&effects, "q2"), "{effects:?}");
    }

    #[test]
    fn the_video_that_plays_never_comes_next_from_the_queue() {
        let mut a = mini();
        a.search.results = vec![Item::Video(plain_video("v1", "Title"))];
        key(&mut a, "a");
        assert_eq!(a.queue.len(), 1);
        event(&mut a, PlayerEvent::Ended);
        assert!(a.player.as_ref().unwrap().up_next.is_none());
    }

    #[test]
    fn opening_the_video_after_its_end_plays_it_again() {
        let mut a = mini();
        a.search.results = vec![Item::Video(plain_video("v1", "Title"))];
        event(&mut a, PlayerEvent::Ended);
        let effects = key(&mut a, "enter");
        assert!(resolves(&effects, "v1"), "{effects:?}");
        assert_eq!(a.player.as_ref().unwrap().session, 2);
        assert!(!a.player.as_ref().unwrap().mini);
    }

    #[test]
    fn comments_keys_work_with_the_mini_player() {
        let mut a = mini();
        a.open(crate::app::screens::Target::Video { id: "v1".into(), title: "Title".into() });
        let effects = key(&mut a, "C");
        assert!(matches!(effects.as_slice(), [Effect::LoadComments { .. }]), "{effects:?}");
        assert_eq!(key(&mut a, "space"), [Effect::Player(PlayerCommand::TogglePause)]);
    }

    #[test]
    fn the_real_title_replaces_the_link_text() {
        let mut a = app();
        a.play_link("v1".into(), "Video v1".into(), Some(30));
        // Before mpv loads it, only the player changes.
        assert!(a.update(Message::VideoTitle { session: 1, title: "Real title".into() }).is_empty());
        assert_eq!(a.player.as_ref().unwrap().title, "Real title");
        a.update(Message::StreamsResolved { session: 1, now: 100, result: Ok(streams(None)) });
        a.update(Message::Player { session: 1, event: PlayerEvent::FileLoaded });
        assert_eq!(a.history[0].title, "Real title");
        // Once it is in the history, the history gets the new title too.
        let effects = a.update(Message::VideoTitle { session: 1, title: "Renamed".into() });
        assert!(matches!(effects.as_slice(), [Effect::SaveWatch(e)] if e.title == "Renamed"), "{effects:?}");
        // The same title, an empty one, or an old session change nothing.
        assert!(a.update(Message::VideoTitle { session: 1, title: "Renamed".into() }).is_empty());
        assert!(a.update(Message::VideoTitle { session: 1, title: String::new() }).is_empty());
        assert!(a.update(Message::VideoTitle { session: 0, title: "Old".into() }).is_empty());
        assert_eq!(a.player.as_ref().unwrap().title, "Renamed");
        // After the end, it stays finished.
        a.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        let effects = a.update(Message::VideoTitle { session: 1, title: "Last".into() });
        assert!(matches!(effects.as_slice(), [Effect::SaveWatch(e)] if e.finished && e.title == "Last"), "{effects:?}");
    }

    #[test]
    fn a_resize_fits_the_mini_player() {
        let mut a = mini();
        let effects = a.update(Message::Input(Input::Resize { width: 150, height: 50 }));
        let g = mini_layout(150, 50, (10, 20), (16, 9)).unwrap().1;
        assert_eq!(effects, [Effect::Player(PlayerCommand::Resize(g))]);
        // No room for the dock: a small picture that nothing shows.
        let effects = a.update(Message::Input(Input::Resize { width: 60, height: 20 }));
        let g = Geometry::fit_area(HIDDEN_CELLS.0, HIDDEN_CELLS.1, (10, 20), (16, 9));
        assert_eq!(effects, [Effect::Player(PlayerCommand::Resize(g))]);
    }
}
