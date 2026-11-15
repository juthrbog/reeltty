//! Videos saved to watch without a network.
//!
//! The app is the only scheduler: one download runs at a time, the oldest
//! queued one first. The runtime runs it and reports progress. Each change
//! of state goes to the database, so the queue goes on after a restart.

use std::path::{Path, PathBuf};

use super::{App, Effect, StatusMessage};
use crate::source::download::{FileFormat, Progress, Request, Saved, size_text};
use crate::source::model::{Item, Video, VideoKind};
use crate::source::streams::{Caption, Streams};
use crate::sponsorblock::{Category, Segment};
use crate::store::SavedCaption;
use crate::store::{Download, DownloadState};

/// The error of a download that the user stopped.
pub const STOPPED: &str = "Stopped.";

#[derive(Debug, Clone, Default)]
pub struct Downloads {
    /// All downloads, in the order they were added.
    pub rows: Vec<Download>,
    /// The video that downloads now.
    pub running: Option<String>,
    /// How far the running download is.
    pub progress: Option<Progress>,
    /// The folder of the files.
    pub dir: PathBuf,
    /// How new downloads are kept, from the configuration.
    pub format: FileFormat,
    /// Another reeltty runs the downloads, so this one starts none.
    pub elsewhere: bool,
    /// The video whose delete waits for a second press.
    pub confirm: Option<String>,
    /// The selected video on the Downloads tab. Rows move between groups
    /// when they change, and the selection stays on the same video.
    pub selected: Option<String>,
    /// The SponsorBlock categories that reeltty skips, from the configuration.
    pub skip: Vec<Category>,
}

/// The state of a video's download, for a mark on its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadMark {
    Queued,
    /// The percent, when it is known.
    Running(Option<u8>),
    Stopped,
    Failed,
    Saved,
}

impl DownloadMark {
    pub fn text(self) -> String {
        match self {
            Self::Queued => "QUEUED".into(),
            Self::Running(Some(p)) => format!("↓ {p}%"),
            Self::Running(None) => "↓".into(),
            Self::Stopped => "STOPPED".into(),
            Self::Failed => "FAILED".into(),
            Self::Saved => "SAVED".into(),
        }
    }
}

/// The percent of a download, rounded down.
pub fn percent(p: Progress) -> Option<u8> {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a percent")]
    p.fraction().map(|f| (f * 100.0).floor() as u8)
}

impl Downloads {
    pub fn get(&self, id: &str) -> Option<&Download> {
        self.rows.iter().find(|d| d.video.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Download> {
        self.rows.iter_mut().find(|d| d.video.id == id)
    }

    /// The download of a video, if its files are on disk.
    pub fn saved(&self, id: &str) -> Option<&Download> {
        self.get(id).filter(|d| d.state == DownloadState::Done)
    }

    /// The downloads that wait, not counting the one that runs.
    pub fn waiting(&self) -> usize {
        self.rows
            .iter()
            .filter(|d| d.state == DownloadState::Queued && self.running.as_deref() != Some(d.video.id.as_str()))
            .count()
    }

    /// The streams of a saved video, from its files.
    pub fn streams(&self, d: &Download) -> Option<Streams> {
        let path = |name: &str| self.dir.join(name).to_string_lossy().into_owned();
        Some(Streams {
            video_url: path(d.video_file.as_deref()?),
            audio_url: d.audio_file.as_deref().map(path),
            headers: Vec::new(),
            available_at: None,
            expires_at: None,
            format: d.format.clone(),
            chapters: d.chapters.clone(),
            channel: d.video.channels.first().cloned(),
            duration_secs: d.video.duration_secs,
            live: false,
            qualities: Vec::new(),
            captions: d
                .captions
                .iter()
                .map(|c| Caption { lang: c.lang.clone(), name: c.name.clone(), url: path(&c.file), auto: c.auto })
                .collect(),
        })
    }

    /// The saved SponsorBlock segments of a video, in the categories that
    /// reeltty skips now.
    pub fn segments(&self, d: &Download) -> Option<Vec<Segment>> {
        let (json, _) = d.segments.as_ref()?;
        let all: Vec<Segment> = serde_json::from_str(json).ok()?;
        Some(all.into_iter().filter(|s| self.skip.contains(&s.category)).collect())
    }

    /// The temporary folder for files that download.
    pub fn temp(&self) -> PathBuf {
        self.dir.join(".part")
    }

    /// The mark for the row of a video, if it has a download.
    pub fn mark(&self, id: &str) -> Option<DownloadMark> {
        let d = self.get(id)?;
        Some(match d.state {
            DownloadState::Done => DownloadMark::Saved,
            DownloadState::Queued if self.running.as_deref() == Some(id) => {
                DownloadMark::Running(self.progress.and_then(percent))
            }
            DownloadState::Queued => DownloadMark::Queued,
            DownloadState::Failed if d.error.as_deref() == Some(STOPPED) => DownloadMark::Stopped,
            DownloadState::Failed => DownloadMark::Failed,
        })
    }

    /// The downloads in the order of the Downloads tab: the one that runs,
    /// the ones that wait, the ones that failed or stopped, then the saved
    /// ones, newest first.
    pub fn shown(&self) -> Vec<&Download> {
        let group = |d: &Download| match d.state {
            DownloadState::Queued if self.running.as_deref() == Some(d.video.id.as_str()) => 0,
            DownloadState::Queued => 1,
            DownloadState::Failed => 2,
            DownloadState::Done => 3,
        };
        let mut shown: Vec<&Download> = self.rows.iter().collect();
        // A stable sort keeps the order they were added in each group.
        shown.sort_by_key(|d| (group(d), if d.state == DownloadState::Done { -d.finished_at.unwrap_or(0) } else { 0 }));
        shown
    }

    /// The size of the saved files.
    pub fn saved_bytes(&self) -> u64 {
        self.rows.iter().filter(|d| d.state == DownloadState::Done).filter_map(|d| d.size_bytes).sum()
    }
}

/// The name of a file in `dir`, or its whole path if it is not there.
fn name_in(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir).unwrap_or(path).to_string_lossy().into_owned()
}

impl App {
    /// The download key on a list item.
    pub(super) fn download_item(&mut self, item: &Item) -> Vec<Effect> {
        if let Item::Video(v) = item {
            return self.toggle_download(v);
        }
        self.status = Some(StatusMessage::warn("Only single videos can be downloaded."));
        Vec::new()
    }

    /// Downloads a video, or stops its download, or tries a failed one
    /// again. A saved video stays saved.
    pub(super) fn toggle_download(&mut self, v: &Video) -> Vec<Effect> {
        let now = self.clock;
        let running = self.downloads.running.as_deref() == Some(v.id.as_str());
        if self.downloads.elsewhere && self.downloads.saved(&v.id).is_none() {
            self.status = Some(StatusMessage::warn("Another reeltty runs the downloads. Download in that one."));
            return Vec::new();
        }
        let mut effects = Vec::new();
        match self.downloads.get_mut(&v.id) {
            Some(d) if d.state == DownloadState::Done => {
                let size = d.size_bytes.map_or_default(|s| format!(" ({})", size_text(s)));
                self.status = Some(StatusMessage::info(format!("{} is downloaded{size}.", d.video.title)));
                return Vec::new();
            }
            Some(d) if d.state == DownloadState::Queued => {
                // The `.part` files stay, so it goes on from there.
                d.state = DownloadState::Failed;
                d.error = Some(STOPPED.into());
                effects.push(Effect::SaveDownload(Box::new(d.clone())));
                let again = self
                    .keymap
                    .first_key(super::Action::Download)
                    .map_or_else(String::new, |k| format!(" Press {k} to go on."));
                self.status = Some(StatusMessage::info(format!("Stopped the download of {}.{again}", d.video.title)));
                if running {
                    self.downloads.running = None;
                    self.downloads.progress = None;
                    effects.push(Effect::CancelDownload { id: v.id.clone() });
                }
            }
            Some(d) => {
                d.state = DownloadState::Queued;
                d.error = None;
                effects.push(Effect::SaveDownload(Box::new(d.clone())));
                self.status = Some(self.queued_message(&v.id, &v.title));
            }
            None if matches!(v.kind, VideoKind::Live) => {
                self.status = Some(StatusMessage::warn("Live streams cannot be downloaded."));
                return Vec::new();
            }
            None => {
                let d = Download::queued(v.clone(), self.max_quality, self.downloads.format, now);
                effects.push(Effect::SaveDownload(Box::new(d.clone())));
                self.downloads.rows.push(d);
                self.status = Some(self.queued_message(&v.id, &v.title));
            }
        }
        effects.extend(self.next_download());
        effects
    }

    /// "Downloading …" when nothing else downloads, or the place in the queue.
    fn queued_message(&self, id: &str, title: &str) -> StatusMessage {
        let running = self.downloads.running.as_deref();
        if running.is_none() {
            return StatusMessage::success(format!("Downloading {title}."));
        }
        // The one that runs, and the queued ones before this one: rows keep
        // the order they were added, also when a stopped one goes on.
        let before = self
            .downloads
            .rows
            .iter()
            .take_while(|d| d.video.id != id)
            .filter(|d| d.state == DownloadState::Queued && Some(d.video.id.as_str()) != running)
            .count();
        StatusMessage::success(format!("Added {title} to the downloads, {} ahead.", before + 1))
    }

    /// Starts the oldest queued download, if none runs.
    pub(super) fn next_download(&mut self) -> Vec<Effect> {
        if self.downloads.running.is_some() || self.downloads.elsewhere {
            return Vec::new();
        }
        let Some(d) = self.downloads.rows.iter().find(|d| d.state == DownloadState::Queued) else { return Vec::new() };
        let request = Request {
            id: d.video.id.clone(),
            max_height: d.max_height,
            format: d.file_format,
            dir: self.downloads.dir.clone(),
            temp: self.downloads.temp(),
        };
        self.downloads.running = Some(request.id.clone());
        self.downloads.progress = None;
        vec![Effect::StartDownload(Box::new(request))]
    }

    pub(super) fn on_download_progress(&mut self, id: &str, progress: Progress) -> Vec<Effect> {
        if self.downloads.running.as_deref() == Some(id) {
            self.downloads.progress = Some(progress);
        }
        Vec::new()
    }

    pub(super) fn on_download_finished(
        &mut self,
        id: &str,
        now: i64,
        result: Result<Box<Saved>, String>,
    ) -> Vec<Effect> {
        if self.downloads.running.as_deref() == Some(id) {
            self.downloads.running = None;
            self.downloads.progress = None;
        }
        let dir = self.downloads.dir.clone();
        let mut effects = Vec::new();
        if let Some(d) = self.downloads.get_mut(id) {
            match result {
                // Also when the user stopped it just as it ended: the files are there.
                Ok(saved) => {
                    d.state = DownloadState::Done;
                    d.error = None;
                    d.video_file = Some(name_in(&dir, &saved.video));
                    d.audio_file = saved.audio.as_deref().map(|a| name_in(&dir, a));
                    d.size_bytes = Some(saved.size_bytes);
                    d.format = saved.format;
                    d.chapters = saved.chapters;
                    d.captions = saved
                        .captions
                        .iter()
                        .map(|c| SavedCaption {
                            lang: c.lang.clone(),
                            name: c.name.clone(),
                            auto: c.auto,
                            file: name_in(&dir, &c.path),
                        })
                        .collect();
                    if let Some(segments) = &saved.segments {
                        d.segments = serde_json::to_string(segments).ok().map(|json| (json, now));
                    }
                    d.video.duration_secs = saved.duration_secs.or(d.video.duration_secs);
                    if d.video.channels.is_empty() {
                        d.video.channels.extend(saved.channel);
                    }
                    d.finished_at = Some(now);
                    self.status = Some(StatusMessage::success(format!(
                        "Downloaded {} ({}).",
                        d.video.title,
                        size_text(saved.size_bytes)
                    )));
                    effects.push(Effect::SaveDownload(Box::new(d.clone())));
                }
                // A download that was stopped keeps its "Stopped." error.
                Err(_) if d.state != DownloadState::Queued => {}
                Err(e) => {
                    d.state = DownloadState::Failed;
                    self.status = Some(StatusMessage::error(format!("Cannot download {}: {e}", d.video.title)));
                    d.error = Some(e);
                    effects.push(Effect::SaveDownload(Box::new(d.clone())));
                }
            }
        }
        effects.extend(self.next_download());
        effects
    }

    /// The saved file of a video that started to play is not there. It
    /// streams instead, and its download is marked failed.
    pub(super) fn on_local_missing(&mut self, session: u64, id: &str) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(d) = self.downloads.get_mut(id).filter(|d| d.state == DownloadState::Done) {
            d.state = DownloadState::Failed;
            d.error = Some("The files are missing.".into());
            effects.push(Effect::SaveDownload(Box::new(d.clone())));
        }
        let Some(p) = self.player.as_mut().filter(|p| p.session == session) else { return effects };
        if p.phase != super::player::Phase::Resolving {
            return effects;
        }
        p.local = false;
        self.status = Some(StatusMessage::warn("The saved files of this video are missing, so it streams."));
        let resolve = self.player.as_ref().map(|p| self.resolve(p));
        effects.extend(resolve);
        effects
    }

    /// Keys on the Downloads tab. `confirm` is the video whose delete
    /// waits for a second press.
    pub(super) fn on_downloads_key(
        &mut self,
        action: Option<super::Action>,
        confirm: Option<&str>,
    ) -> Option<Vec<Effect>> {
        use super::Action;
        let action = action?;
        let rows = self.plain_rows();
        let len = self.downloads.rows.len();
        if self.downloads_selection.apply(action, len, rows) {
            let index = self.downloads_selection.index;
            self.downloads.selected = self.downloads.shown().get(index).map(|d| d.video.id.clone());
            return Some(Vec::new());
        }
        let d = self.downloads.shown().get(self.downloads_selection.index).map(|d| (*d).clone())?;
        let video = d.video.clone();
        match action {
            Action::Open => Some(self.play(video.id, video.title)),
            Action::Download => Some(self.toggle_download(&video)),
            // `r` tries a failed download again.
            Action::Refresh if d.state == DownloadState::Failed => Some(self.toggle_download(&video)),
            Action::Refresh => Some(Vec::new()),
            // The other reeltty has the row too, and would save it again.
            Action::Delete if self.downloads.elsewhere => {
                self.status = Some(StatusMessage::warn("Another reeltty runs the downloads. Delete in that one."));
                Some(Vec::new())
            }
            Action::Delete if confirm == Some(video.id.as_str()) => Some(self.delete_download(&d)),
            Action::Delete => {
                let what = match d.size_bytes.filter(|_| d.state == DownloadState::Done) {
                    Some(size) => format!("delete {} ({})", video.title, size_text(size)),
                    None => format!("remove {} from the downloads", video.title),
                };
                let again =
                    self.keymap.first_key(Action::Delete).map_or_else(|| "the key".to_string(), |k| k.to_string());
                self.status = Some(StatusMessage::warn(format!("Press {again} again to {what}.")));
                self.downloads.confirm = Some(video.id);
                Some(Vec::new())
            }
            Action::Enqueue => {
                self.toggle_queued(&video);
                Some(Vec::new())
            }
            Action::Details => Some(self.open(super::screens::Target::open(&Item::Video(video)))),
            Action::Channel => match super::screens::Target::channel_of(&Item::Video(video)) {
                Some(t) => Some(self.open(t)),
                None => Some(Vec::new()),
            },
            _ => None,
        }
    }

    /// Keeps the Downloads selection on the selected video when the rows
    /// change, or on the row in its place when that video is gone.
    pub(super) fn anchor_downloads(&mut self) {
        let rows = self.plain_rows();
        let shown = self.downloads.shown();
        let at = self.downloads.selected.as_ref().and_then(|id| shown.iter().position(|d| &d.video.id == id));
        if let Some(i) = at {
            self.downloads_selection.index = i;
        }
        self.downloads_selection.clamp(shown.len(), rows);
        let id = shown.get(self.downloads_selection.index).map(|d| d.video.id.clone());
        self.downloads.selected = id;
    }

    /// Deletes a download and its files, and stops it if it runs.
    fn delete_download(&mut self, d: &Download) -> Vec<Effect> {
        let id = d.video.id.clone();
        let mut effects = Vec::new();
        if self.downloads.running.as_deref() == Some(id.as_str()) {
            self.downloads.running = None;
            self.downloads.progress = None;
            effects.push(Effect::CancelDownload { id: id.clone() });
        }
        self.downloads.rows.retain(|r| r.video.id != id);
        self.downloads_selection.clamp(self.downloads.rows.len(), self.plain_rows());
        let files = [&d.video_file, &d.audio_file].into_iter().flatten().map(|f| self.downloads.dir.join(f)).collect();
        effects.push(Effect::DeleteDownload { id, files, temp: self.downloads.temp() });
        self.status = Some(StatusMessage::success(match d.size_bytes.filter(|_| d.state == DownloadState::Done) {
            Some(size) => format!("Deleted {} ({}).", d.video.title, size_text(size)),
            None => format!("Removed {} from the downloads.", d.video.title),
        }));
        effects.extend(self.next_download());
        effects
    }

    /// The lasting status line of the downloads: "↓ title 42% · 2 queued".
    pub fn download_status(&self) -> Option<StatusMessage> {
        let id = self.downloads.running.as_deref()?;
        let title = self.downloads.get(id).map_or(id, |d| d.video.title.as_str());
        let percent = match self.downloads.progress.and_then(percent) {
            Some(p) => format!("{p}%"),
            None => "…".to_string(),
        };
        let waiting = match self.downloads.waiting() {
            0 => String::new(),
            n => format!(" · {n} queued"),
        };
        Some(StatusMessage::info(format!("↓ {title} {percent}{waiting}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::player::Phase;
    use crate::app::{Input, Message};
    use crate::keymap::KeyMap;
    use crate::source::streams::FormatInfo;
    use crate::terminal::detect::Graphics;

    fn video(id: &str) -> Video {
        let Item::Video(v) = crate::app::tests::video(id) else { unreachable!() };
        v
    }

    /// An app on the Queue tab, with videos a and b in the queue.
    fn app() -> App {
        let mut a = App::new(KeyMap::default(), Graphics::KittyShm);
        a.update(Message::Input(Input::Resize { width: 80, height: 24 }));
        a.update(Message::Input(Input::CellSize { width: 10, height: 20 }));
        a.downloads.dir = PathBuf::from("/d");
        a.queue = vec![video("a"), video("b")];
        key(&mut a, "5");
        a
    }

    fn key(a: &mut App, s: &str) -> Vec<Effect> {
        a.update(Message::Input(Input::Key(s.parse().unwrap())))
    }

    fn started(effects: &[Effect]) -> Vec<&str> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::StartDownload(r) => Some(r.id.as_str()),
                _ => None,
            })
            .collect()
    }

    fn saved_states(effects: &[Effect]) -> Vec<(&str, DownloadState)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::SaveDownload(d) => Some((d.video.id.as_str(), d.state)),
                _ => None,
            })
            .collect()
    }

    fn done(id: &str) -> Saved {
        Saved {
            video: PathBuf::from(format!("/d/{id}.f136.mp4")),
            audio: Some(PathBuf::from(format!("/d/{id}.f251.webm"))),
            size_bytes: 412_000_000,
            format: FormatInfo { id: "136+251".into(), width: Some(1280), height: Some(720), ..FormatInfo::default() },
            title: Some("Real title".into()),
            channel: None,
            duration_secs: Some(60),
            chapters: Vec::new(),
            captions: Vec::new(),
            segments: None,
        }
    }

    fn finish(a: &mut App, id: &str, result: Result<Box<Saved>, String>) -> Vec<Effect> {
        a.update(Message::DownloadFinished { id: id.into(), now: 9, result })
    }

    #[test]
    fn d_queues_one_download_at_a_time() {
        let mut a = app();
        let effects = key(&mut a, "D");
        assert_eq!(saved_states(&effects), [("a", DownloadState::Queued)]);
        assert_eq!(started(&effects), ["a"]);
        let Some(Effect::StartDownload(r)) = effects.iter().find(|e| matches!(e, Effect::StartDownload(_))) else {
            unreachable!()
        };
        assert_eq!((r.dir.as_path(), r.temp.as_path(), r.max_height), (Path::new("/d"), Path::new("/d/.part"), 1080));
        assert_eq!(a.status.as_deref(), Some("Downloading Video a."));

        // The second waits for the first.
        key(&mut a, "j");
        let effects = key(&mut a, "D");
        assert_eq!(saved_states(&effects), [("b", DownloadState::Queued)]);
        assert!(started(&effects).is_empty());
        assert_eq!(a.status.as_deref(), Some("Added Video b to the downloads, 1 ahead."));

        // Progress shows in the status line.
        assert_eq!(a.download_status().as_deref(), Some("↓ Video a … · 1 queued"));
        let progress = Progress { done: 42, total: Some(100), speed: None };
        a.update(Message::DownloadProgress { id: "a".into(), progress });
        assert_eq!(a.download_status().as_deref(), Some("↓ Video a 42% · 1 queued"));

        // When it ends, it is saved, and the next one starts.
        let effects = finish(&mut a, "a", Ok(Box::new(done("a"))));
        assert_eq!(saved_states(&effects), [("a", DownloadState::Done)]);
        assert_eq!(started(&effects), ["b"]);
        assert_eq!(a.status.as_deref(), Some("Downloaded Video a (412 MB)."));
        let d = a.downloads.saved("a").unwrap();
        assert_eq!((d.video_file.as_deref(), d.audio_file.as_deref()), (Some("a.f136.mp4"), Some("a.f251.webm")));
        assert_eq!((d.finished_at, d.video.duration_secs), (Some(9), Some(60)));

        // A failure is kept with its reason, and the queue goes on.
        let effects = finish(&mut a, "b", Err("HTTP 403".into()));
        assert_eq!(saved_states(&effects), [("b", DownloadState::Failed)]);
        assert_eq!(a.status.as_deref(), Some("Cannot download Video b: HTTP 403"));
        assert_eq!(a.downloads.running, None);
        assert_eq!(a.download_status(), None);
    }

    #[test]
    fn d_stops_and_goes_on() {
        let mut a = app();
        key(&mut a, "D");
        let effects = key(&mut a, "D");
        assert!(effects.contains(&Effect::CancelDownload { id: "a".into() }));
        assert_eq!(saved_states(&effects), [("a", DownloadState::Failed)]);
        assert_eq!(a.status.as_deref(), Some("Stopped the download of Video a. Press D to go on."));
        assert_eq!(a.downloads.get("a").unwrap().error.as_deref(), Some(STOPPED));
        // The end of the stopped task does not change it.
        finish(&mut a, "a", Err("killed".into()));
        assert_eq!(a.downloads.get("a").unwrap().error.as_deref(), Some(STOPPED));

        let effects = key(&mut a, "D");
        assert_eq!(saved_states(&effects), [("a", DownloadState::Queued)]);
        assert_eq!(started(&effects), ["a"]);

        // A saved video stays saved.
        finish(&mut a, "a", Ok(Box::new(done("a"))));
        assert!(key(&mut a, "D").is_empty());
        assert_eq!(a.status.as_deref(), Some("Video a is downloaded (412 MB)."));
    }

    #[test]
    fn a_download_that_goes_on_counts_the_ones_before_it() {
        let mut a = app();
        a.queue.push(video("c"));
        for _ in 0..3 {
            key(&mut a, "D");
            key(&mut a, "j");
        }
        // a runs, b and c wait. Stop b, and go on with it: it keeps its place.
        key(&mut a, "k");
        key(&mut a, "D");
        key(&mut a, "D");
        assert_eq!(a.status.as_deref(), Some("Added Video b to the downloads, 1 ahead."));
        key(&mut a, "j");
        key(&mut a, "D");
        key(&mut a, "D");
        assert_eq!(a.status.as_deref(), Some("Added Video c to the downloads, 2 ahead."));
    }

    #[test]
    fn another_reeltty_runs_the_downloads() {
        let mut a = app();
        a.downloads.elsewhere = true;
        a.downloads.rows = vec![Download::queued(video("y"), 720, FileFormat::Mkv, 2)];
        assert!(a.start().iter().all(|e| !matches!(e, Effect::StartDownload(_))));
        assert!(key(&mut a, "D").is_empty());
        assert_eq!(a.status.as_deref(), Some("Another reeltty runs the downloads. Download in that one."));
    }

    #[test]
    fn queued_downloads_go_on_at_the_start() {
        let mut a = app();
        a.downloads.rows = vec![
            Download { state: DownloadState::Failed, ..Download::queued(video("x"), 720, FileFormat::Mkv, 1) },
            Download::queued(video("y"), 720, FileFormat::Mkv, 2),
        ];
        let effects = a.start();
        let Some(Effect::StartDownload(r)) = effects.first() else { panic!("{effects:?}") };
        assert_eq!((r.id.as_str(), r.format, r.max_height), ("y", FileFormat::Mkv, 720));
    }

    #[test]
    fn only_videos_that_are_not_live_download() {
        let mut a = app();
        let live = Video { kind: VideoKind::Live, ..video("l") };
        assert!(a.toggle_download(&live).is_empty());
        assert_eq!(a.status.as_deref(), Some("Live streams cannot be downloaded."));
        let channel = Item::Channel(crate::source::model::Channel {
            id: "UC1".into(),
            name: "C".into(),
            handle: None,
            subscribers_text: None,
            description_snippet: None,
            verified: false,
            thumbnails: Vec::new(),
        });
        assert!(a.download_item(&channel).is_empty());
        assert_eq!(a.status.as_deref(), Some("Only single videos can be downloaded."));
    }

    #[test]
    fn a_saved_video_plays_from_its_files() {
        let mut a = app();
        key(&mut a, "D");
        finish(&mut a, "a", Ok(Box::new(done("a"))));
        let effects = key(&mut a, "enter");
        let Some(Effect::PlayLocal { session, id, streams }) =
            effects.iter().find(|e| matches!(e, Effect::PlayLocal { .. }))
        else {
            panic!("{effects:?}")
        };
        assert!(!effects.iter().any(|e| matches!(e, Effect::ResolveStreams { .. })));
        assert_eq!(
            (streams.video_url.as_str(), streams.audio_url.as_deref()),
            ("/d/a.f136.mp4", Some("/d/a.f251.webm"))
        );
        assert!(streams.headers.is_empty() && streams.available_at.is_none());
        let (session, id) = (*session, id.clone());
        assert!(a.player.as_ref().unwrap().local);

        // The files are gone: it streams, and the download is failed.
        let effects = a.update(Message::LocalMissing { session, id });
        assert!(effects.iter().any(|e| matches!(e, Effect::ResolveStreams { .. })), "{effects:?}");
        assert_eq!(saved_states(&effects), [("a", DownloadState::Failed)]);
        let p = a.player.as_ref().unwrap();
        assert!(!p.local && p.phase == Phase::Resolving);
    }

    #[test]
    fn a_saved_video_that_fails_does_not_stream() {
        let mut a = app();
        key(&mut a, "D");
        finish(&mut a, "a", Ok(Box::new(done("a"))));
        let effects = key(&mut a, "enter");
        let Some(Effect::PlayLocal { session, streams, .. }) =
            effects.into_iter().find(|e| matches!(e, Effect::PlayLocal { .. }))
        else {
            unreachable!()
        };
        a.update(Message::StreamsResolved { session, now: 0, result: Ok(streams) });
        assert_eq!(a.player.as_ref().unwrap().phase, Phase::Loading);
        // An HTTP error is about streams, so a file never gets one, but no
        // error of a file starts a stream search.
        let event = crate::app::player::PlayerEvent::Failed("HTTP 403 Forbidden".into());
        let effects = a.update(Message::Player { session, event });
        assert!(!effects.iter().any(|e| matches!(e, Effect::ResolveStreams { .. })), "{effects:?}");
        assert!(matches!(a.player.as_ref().unwrap().phase, Phase::Failed(_)));
    }

    fn deleted(effects: &[Effect]) -> Option<(&str, &[PathBuf])> {
        effects.iter().find_map(|e| match e {
            Effect::DeleteDownload { id, files, temp } => {
                assert_eq!(temp, Path::new("/d/.part"));
                Some((id.as_str(), files.as_slice()))
            }
            _ => None,
        })
    }

    #[test]
    fn the_downloads_tab_deletes_on_the_second_press() {
        let mut a = app();
        key(&mut a, "D");
        finish(&mut a, "a", Ok(Box::new(done("a"))));
        key(&mut a, "7");
        assert!(key(&mut a, "d").is_empty());
        assert_eq!(a.status.as_deref(), Some("Press d again to delete Video a (412 MB)."));
        // Any other key forgets the first press.
        key(&mut a, "j");
        assert!(deleted(&key(&mut a, "d")).is_none());
        let effects = key(&mut a, "d");
        let files = [PathBuf::from("/d/a.f136.mp4"), PathBuf::from("/d/a.f251.webm")];
        assert_eq!(deleted(&effects), Some(("a", &files[..])));
        assert_eq!(a.status.as_deref(), Some("Deleted Video a (412 MB)."));
        assert!(a.downloads.rows.is_empty());
        assert_eq!(a.downloads.mark("a"), None);
    }

    #[test]
    fn deleting_the_running_download_stops_it_and_starts_the_next() {
        let mut a = app();
        key(&mut a, "D");
        key(&mut a, "j");
        key(&mut a, "D");
        key(&mut a, "7");
        // The running one is first.
        key(&mut a, "d");
        assert_eq!(a.status.as_deref(), Some("Press d again to remove Video a from the downloads."));
        let effects = key(&mut a, "d");
        assert!(effects.contains(&Effect::CancelDownload { id: "a".into() }));
        assert_eq!(deleted(&effects), Some(("a", &[][..])));
        assert_eq!(started(&effects), ["b"]);
        assert_eq!(a.status.as_deref(), Some("Removed Video a from the downloads."));
    }

    #[test]
    fn the_selection_stays_on_its_video() {
        let mut a = app();
        a.queue.push(video("c"));
        for _ in 0..3 {
            key(&mut a, "D");
            key(&mut a, "j");
        }
        key(&mut a, "7");
        // a runs, b and c wait. Select b.
        key(&mut a, "j");
        assert_eq!(a.downloads.selected.as_deref(), Some("b"));
        // a ends and b starts: b moves to the top, and stays selected.
        finish(&mut a, "a", Ok(Box::new(done("a"))));
        assert_eq!(a.downloads_selection.index, 0);
        // D stops b, which moves down to the stopped ones.
        key(&mut a, "D");
        assert_eq!(a.downloads.shown()[a.downloads_selection.index].video.id, "b");
        assert_eq!(a.downloads.selected.as_deref(), Some("b"));
    }

    #[test]
    fn another_reeltty_does_not_delete() {
        let mut a = app();
        a.downloads.elsewhere = true;
        a.downloads.rows = vec![Download::queued(video("y"), 720, FileFormat::Mkv, 2)];
        key(&mut a, "7");
        key(&mut a, "d");
        assert!(key(&mut a, "d").is_empty());
        assert_eq!(a.status.as_deref(), Some("Another reeltty runs the downloads. Delete in that one."));
        assert_eq!(a.downloads.rows.len(), 1);
    }

    fn segment(category: Category, start_ms: u64) -> Segment {
        Segment { category, start_ms, end_ms: start_ms + 5000, video_duration_ms: None }
    }

    #[test]
    fn a_saved_video_plays_with_its_captions_and_segments() {
        use crate::source::download::CaptionFile;
        let mut a = app();
        a.sponsorblock = true;
        a.downloads.skip = vec![Category::Sponsor];
        key(&mut a, "D");
        let saved = Saved {
            captions: vec![CaptionFile {
                lang: "en".into(),
                name: "English".into(),
                auto: false,
                path: PathBuf::from("/d/a.en.vtt"),
            }],
            segments: Some(vec![segment(Category::Sponsor, 10_000), segment(Category::Intro, 0)]),
            ..done("a")
        };
        finish(&mut a, "a", Ok(Box::new(saved)));
        let d = a.downloads.saved("a").unwrap();
        assert_eq!(d.captions[0].file, "a.en.vtt");
        assert_eq!(d.segments.as_ref().map(|(_, at)| *at), Some(9));

        // It plays with them, and only the categories that reeltty skips now.
        a.captions_on = true;
        let effects = key(&mut a, "enter");
        let Some(Effect::PlayLocal { session, streams, .. }) =
            effects.into_iter().find(|e| matches!(e, Effect::PlayLocal { .. }))
        else {
            unreachable!()
        };
        assert_eq!(streams.captions[0].url, "/d/a.en.vtt");
        let p = a.player.as_ref().unwrap();
        assert_eq!(p.skip_segments(), [segment(Category::Sponsor, 10_000)]);
        a.update(Message::StreamsResolved { session, now: 0, result: Ok(streams) });
        assert_eq!(a.player.as_ref().unwrap().caption.as_deref(), Some("English"));

        // Without a network, the saved segments stay.
        a.update(Message::SegmentsLoaded { session, result: Err("offline".into()) });
        assert_eq!(a.player.as_ref().unwrap().skip_segments().len(), 1);
        // With one, the new segments take their place, and are saved.
        let newer = vec![segment(Category::Sponsor, 20_000)];
        let effects = a.update(Message::SegmentsLoaded { session, result: Ok(newer.clone()) });
        assert_eq!(saved_states(&effects), [("a", DownloadState::Done)]);
        assert_eq!(a.downloads.segments(a.downloads.saved("a").unwrap()), Some(newer));
    }

    #[test]
    fn r_tries_a_failed_download_again() {
        let mut a = app();
        key(&mut a, "D");
        finish(&mut a, "a", Err("HTTP 403".into()));
        key(&mut a, "7");
        let effects = key(&mut a, "r");
        assert_eq!(saved_states(&effects), [("a", DownloadState::Queued)]);
        assert_eq!(started(&effects), ["a"]);
        // On a download that runs, r does nothing.
        assert!(key(&mut a, "r").is_empty());
    }

    #[test]
    fn the_tab_orders_by_state_and_marks_each_row() {
        let mut a = app();
        let row = |id: &str, state, finished_at, error: Option<&str>| Download {
            state,
            finished_at,
            error: error.map(String::from),
            ..Download::queued(video(id), 720, FileFormat::Separate, 0)
        };
        a.downloads.rows = vec![
            row("old", DownloadState::Done, Some(1), None),
            row("stopped", DownloadState::Failed, None, Some(STOPPED)),
            row("new", DownloadState::Done, Some(5), None),
            row("wait", DownloadState::Queued, None, None),
            row("run", DownloadState::Queued, None, None),
        ];
        a.downloads.running = Some("run".into());
        a.downloads.progress = Some(Progress { done: 1, total: Some(4), speed: None });
        let ids: Vec<&str> = a.downloads.shown().iter().map(|d| d.video.id.as_str()).collect();
        assert_eq!(ids, ["run", "wait", "stopped", "new", "old"]);
        let marks: Vec<String> = ids.iter().map(|id| a.downloads.mark(id).unwrap().text()).collect();
        assert_eq!(marks, ["↓ 25%", "QUEUED", "STOPPED", "SAVED", "SAVED"]);
        assert_eq!(DownloadMark::Failed.text(), "FAILED");
    }
}
