//! Videos saved to watch without a network.
//!
//! The app is the only scheduler: one download runs at a time, the oldest
//! queued one first. The runtime runs it and reports progress. Each change
//! of state goes to the database, so the queue goes on after a restart.

use std::path::{Path, PathBuf};

use super::{App, Effect, StatusMessage};
use crate::source::download::{FileFormat, Progress, Request, Saved, size_text};
use crate::source::model::{Item, Video, VideoKind};
use crate::source::streams::Streams;
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
            captions: Vec::new(),
        })
    }

    /// The temporary folder for files that download.
    pub fn temp(&self) -> PathBuf {
        self.dir.join(".part")
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
                self.status = Some(self.queued_message(&v.title));
            }
            None if matches!(v.kind, VideoKind::Live) => {
                self.status = Some(StatusMessage::warn("Live streams cannot be downloaded."));
                return Vec::new();
            }
            None => {
                let d = Download::queued(v.clone(), self.max_quality, self.downloads.format, now);
                effects.push(Effect::SaveDownload(Box::new(d.clone())));
                self.downloads.rows.push(d);
                self.status = Some(self.queued_message(&v.title));
            }
        }
        effects.extend(self.next_download());
        effects
    }

    /// "Downloading …" when nothing else downloads, or the place in the queue.
    fn queued_message(&self, title: &str) -> StatusMessage {
        if self.downloads.running.is_none() {
            return StatusMessage::success(format!("Downloading {title}."));
        }
        // The waiting ones count this one, not the one that runs.
        let ahead = self.downloads.waiting();
        StatusMessage::success(format!("Added {title} to the downloads, {ahead} ahead."))
    }

    /// Starts the oldest queued download, if none runs.
    pub(super) fn next_download(&mut self) -> Vec<Effect> {
        if self.downloads.running.is_some() {
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

    /// The lasting status line of the downloads: "↓ title 42% · 2 queued".
    pub fn download_status(&self) -> Option<StatusMessage> {
        let id = self.downloads.running.as_deref()?;
        let title = self.downloads.get(id).map_or(id, |d| d.video.title.as_str());
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a percent")]
        let percent = match self.downloads.progress.and_then(|p| p.fraction()) {
            Some(f) => format!("{}%", (f * 100.0).floor() as u8),
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
}
