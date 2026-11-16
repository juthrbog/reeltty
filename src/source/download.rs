//! Downloads a video with yt-dlp, to watch it without a network.
//!
//! yt-dlp writes the files and prints one line for each event, in a form
//! that this module sets with `--print` and `--progress-template`. Each
//! line starts with `RT`, a tab, and a kind:
//!
//! - `fmt`: before a download starts, the format and its size as JSON.
//! - `dl`: progress, as tab-separated fields.
//! - `done`: after a file is in its folder, its path and details as JSON.
//!
//! The files are named by the video ID, so they never need cleaning. yt-dlp
//! writes into the `temp` folder, then moves each file when it is complete.
//! A stopped download leaves its `.part` files there, and the next run
//! continues from them.
//!
//! Live streams and videos that have not started cannot be downloaded.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::model::{ChannelRef, Chapter};
use super::streams::{self, FormatInfo};
use super::ytdlp::{PLAYER_CLIENTS, YtDlp};
use super::{Result, SourceError};

/// How long yt-dlp can print nothing before the download stops. Progress
/// lines come every second while bytes arrive, so a long silence means
/// that it hangs.
const QUIET: Duration = Duration::from_mins(5);

/// How long yt-dlp can print nothing after both formats are on disk. ffmpeg
/// merges them for the `Mkv` format and prints nothing, and that takes long
/// for a large video on a slow disk.
const MERGE: Duration = Duration::from_hours(1);

/// How a download is kept on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    /// A video file and an audio file, as YouTube sends them. This needs no
    /// other program, and mpv plays the two together.
    #[default]
    Separate,
    /// One Matroska file with the video and the audio, which any player can
    /// open. yt-dlp needs ffmpeg to merge them.
    Mkv,
}

impl FileFormat {
    /// The name in the configuration file and the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Separate => "separate",
            Self::Mkv => "mkv",
        }
    }

    /// The format with this name. An unknown name gives `Separate`.
    pub fn from_name(name: &str) -> Self {
        if name == "mkv" { Self::Mkv } else { Self::Separate }
    }
}

/// What to download, and where to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    /// At most this many pixels on the short side, as for playback.
    pub max_height: u32,
    pub format: FileFormat,
    /// The folder for the finished files. It must be an absolute path.
    pub dir: PathBuf,
    /// The folder for the files while they download. It must be an
    /// absolute path, on the same disk as `dir`, so that the last move is a
    /// rename.
    pub temp: PathBuf,
}

/// How far a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// Bytes on disk, for all the files of the download.
    pub done: u64,
    /// The size of all the files, when yt-dlp knows it. It can grow when
    /// the second file starts.
    pub total: Option<u64>,
    /// Bytes per second.
    pub speed: Option<u64>,
}

impl Progress {
    /// The part that is done, from 0 to 1.
    pub fn fraction(&self) -> Option<f64> {
        #[expect(clippy::cast_precision_loss, reason = "a fraction for the screen")]
        self.total.filter(|t| *t > 0).map(|t| (self.done as f64 / t as f64).min(1.0))
    }
}

/// A finished download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// The video file. In the `Mkv` format it has the audio too.
    pub video: PathBuf,
    /// The audio file, in the `Separate` format.
    pub audio: Option<PathBuf>,
    pub size_bytes: u64,
    pub format: FormatInfo,
    pub title: Option<String>,
    pub channel: Option<ChannelRef>,
    pub duration_secs: Option<u32>,
    pub chapters: Vec<Chapter>,
    /// The languages of the caption tracks made by a person, to choose the
    /// tracks to save.
    pub manual_captions: Vec<String>,
}

/// The caption tracks to save for the caption language `lang`, as the
/// player would choose them online (`pick_caption`): the track made by a
/// person in that language, or else in the same language of another region
/// (`en-US` for `en`). Then the speech recognition track, which is
/// `<language>-orig` for a video in that language. Without a track made by
/// a person, also the automatic track in that language, which is a
/// machine translation for a video in another language.
pub fn caption_langs(lang: &str, manual: &[String]) -> Vec<String> {
    let base = |l: &str| l.split('-').next().unwrap_or_default().to_ascii_lowercase();
    let wanted = base(lang);
    let person = manual
        .iter()
        .find(|m| m.eq_ignore_ascii_case(lang))
        .or_else(|| manual.iter().find(|m| base(m) == wanted && !m.ends_with("-orig")));
    let mut langs: Vec<String> = person.cloned().into_iter().collect();
    langs.push(format!("{wanted}-orig"));
    if person.is_none() {
        langs.push(wanted);
    }
    langs
}

/// A caption track on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionFile {
    /// A language code such as `en` or `en-orig`.
    pub lang: String,
    /// The language name, such as "English (Original)".
    pub name: String,
    /// Made by speech recognition, not by a person.
    pub auto: bool,
    pub path: PathBuf,
}

/// How long the caption run can take.
const CAPTIONS_TIMEOUT: Duration = Duration::from_mins(2);

/// The arguments to download only the caption tracks `langs` of a video,
/// from `caption_langs`. Exact codes, because a pattern also matches many
/// machine translations, and YouTube then refuses with HTTP 429. Files
/// that do not finish stay in `temp`, which reeltty cleans.
pub fn caption_args(id: &str, langs: &[String], dir: &std::path::Path, temp: &std::path::Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "--ignore-config",
        "--no-playlist",
        "--no-warnings",
        "--quiet",
        "--skip-download",
        "--write-subs",
        "--write-auto-subs",
        "--sub-format",
        "vtt",
        "--extractor-args",
        PLAYER_CLIENTS,
        // `--print` alone would not write the files.
        "--no-simulate",
        "--print",
        "RT\tsubs\t%(.{requested_subtitles,subtitles})j",
        "-o",
        "%(id)s.%(ext)s",
    ]
    .map(String::from)
    .into();
    args.extend(["--sub-langs".into(), langs.join(",")]);
    args.extend(["-P".into(), format!("home:{}", dir.display())]);
    args.extend(["-P".into(), format!("temp:{}", temp.display())]);
    args.push(format!("https://www.youtube.com/watch?v={id}"));
    args
}

/// What the caption run prints: the tracks it downloads, and the tracks
/// made by a person.
#[derive(Deserialize)]
struct SubsLine {
    requested_subtitles: Option<BTreeMap<String, SubTrack>>,
    subtitles: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct SubTrack {
    name: Option<String>,
    ext: Option<String>,
}

/// The caption files that the run wrote, from its output and the folder.
fn caption_files(stdout: &str, id: &str, dir: &std::path::Path) -> Vec<CaptionFile> {
    let Some(line) = stdout.lines().find_map(|l| l.strip_prefix("RT\tsubs\t")) else { return Vec::new() };
    let Ok(subs) = serde_json::from_str::<SubsLine>(line) else { return Vec::new() };
    let manual = subs.subtitles.unwrap_or_default();
    subs.requested_subtitles
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(lang, track)| {
            let ext = track.ext.unwrap_or_else(|| "vtt".into());
            let path = dir.join(format!("{id}.{lang}.{ext}"));
            // A track that failed, for example with HTTP 429, has no file.
            std::fs::metadata(&path).ok().filter(|m| m.is_file() && m.len() > 0)?;
            let auto = !manual.contains_key(&lang);
            Some(CaptionFile { name: track.name.unwrap_or_else(|| lang.clone()), auto, path, lang })
        })
        .collect()
}

/// A size as people read it, in powers of 1000 as disks count: "980 KB",
/// "412 MB", "3.4 GB", or "12 GB".
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    #[expect(clippy::cast_precision_loss, reason = "a size for people")]
    let mut n = bytes as f64;
    let mut unit = 0;
    while n >= 999.5 && unit < UNITS.len() - 1 {
        n /= 1000.0;
        unit += 1;
    }
    if unit >= 3 && n < 9.95 { format!("{n:.1} {}", UNITS[unit]) } else { format!("{n:.0} {}", UNITS[unit]) }
}

/// Removes the files in `temp` of downloads that will not go on, and gives
/// how many it removed. Each file name starts with the video ID and a dot,
/// such as `<id>.f137.mp4.part`. `keep` says which IDs go on.
pub fn sweep(temp: &std::path::Path, keep: impl Fn(&str) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(temp) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let id = name.to_string_lossy().split('.').next().unwrap_or_default().to_string();
        if !keep(&id) && entry.file_type().is_ok_and(|t| t.is_file()) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The arguments for yt-dlp.
pub fn args(req: &Request) -> Vec<String> {
    let merged = req.format == FileFormat::Mkv;
    let mut args: Vec<String> = [
        // The user's yt-dlp configuration could change the file names or add
        // steps that fail, such as writing captions.
        "--ignore-config",
        // Without it, yt-dlp goes on after some errors and exits with 0. For
        // example, without ffmpeg it does not merge, and keeps two files.
        "--abort-on-error",
        "--no-playlist",
        "--no-warnings",
        "--quiet",
        // `--quiet` hides the progress too, so ask for it again, one line for
        // each update, at most once a second.
        "--progress",
        "--newline",
        "--progress-delta",
        "1",
        "--progress-template",
        "download:RT\tdl\t%(info.format_id)s\t%(progress.downloaded_bytes,progress.total_bytes)s\t%(progress.total_bytes)s\t\
         %(progress.total_bytes_estimate)s\t%(progress.speed)s",
        "--print",
        "before_dl:RT\tfmt\t%(.{format_id,filesize,filesize_approx})j",
        // The caption tracks made by a person, to choose the ones to save.
        "--print",
        "before_dl:RT\tmanual\t%(subtitles)j",
        "--print",
        "after_move:RT\tdone\t%(.{filepath,format_id,width,height,fps,vcodec,acodec,duration,chapters,\
         title,channel,channel_id,uploader_id})j",
        "--extractor-args",
        PLAYER_CLIENTS,
        "--match-filters",
        "!is_live & live_status!=is_upcoming & live_status!=post_live",
        // The fix-up steps need ffmpeg, and mpv plays the files without them.
        "--fixup",
        "never",
        "--no-mtime",
    ]
    .map(String::from)
    .into();
    args.extend(["-f".into(), streams::download_selector(req.max_height, merged)]);
    args.extend(["-S".into(), streams::sort(req.max_height)]);
    args.extend(["-P".into(), format!("home:{}", req.dir.display())]);
    args.extend(["-P".into(), format!("temp:{}", req.temp.display())]);
    if merged {
        args.extend(["--merge-output-format".into(), "mkv".into(), "-o".into(), "%(id)s.%(ext)s".into()]);
    } else {
        args.extend(["-o".into(), "%(id)s.f%(format_id)s.%(ext)s".into()]);
    }
    args.push(format!("https://www.youtube.com/watch?v={}", req.id));
    args
}

/// One line from yt-dlp.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    /// A download is about to start. `format_id` is `137+251` when yt-dlp
    /// downloads two formats to merge them.
    Format { format_id: String, size: Option<u64> },
    /// Progress of one format.
    Bytes { format_id: String, done: u64, total: Option<u64>, speed: Option<f64> },
    /// A file is complete and in its folder.
    Done(Box<DoneFile>),
    /// The languages of the caption tracks made by a person.
    Manual(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct SizeLine {
    format_id: Option<String>,
    filesize: Option<u64>,
    filesize_approx: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct DoneFile {
    filepath: Option<PathBuf>,
    format_id: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    duration: Option<f64>,
    chapters: Option<Vec<DoneChapter>>,
    title: Option<String>,
    channel: Option<String>,
    channel_id: Option<String>,
    uploader_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct DoneChapter {
    title: Option<String>,
    start_time: Option<f64>,
}

/// A codec name, or `None` for yt-dlp's "none".
fn codec(c: Option<&String>) -> Option<&str> {
    c.map(String::as_str).filter(|c| !c.is_empty() && *c != "none")
}

impl DoneFile {
    fn has_video(&self) -> bool {
        codec(self.vcodec.as_ref()).is_some()
    }

    fn has_audio(&self) -> bool {
        codec(self.acodec.as_ref()).is_some()
    }
}

/// Reads one line, or `None` for a line that is not ours, such as a message
/// from an extractor.
fn parse_line(line: &str) -> Option<Line> {
    let rest = line.trim_end_matches(['\r', '\n']).strip_prefix("RT\t")?;
    let (kind, rest) = rest.split_once('\t')?;
    match kind {
        "fmt" => {
            let s: SizeLine = serde_json::from_str(rest).ok()?;
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "file sizes")]
            let approx = s.filesize_approx.filter(|n| n.is_finite() && *n > 0.0).map(|n| n as u64);
            Some(Line::Format { format_id: s.format_id?, size: s.filesize.or(approx) })
        }
        "dl" => {
            let fields: Vec<&str> = rest.split('\t').collect();
            let [format_id, done, total, estimate, speed] = fields[..] else { return None };
            // yt-dlp writes `NA` when it does not know a number, and the
            // estimate as a float.
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "byte counts")]
            let int = |s: &str| s.parse::<u64>().ok().or_else(|| float(s).map(|f| f as u64));
            Some(Line::Bytes {
                format_id: format_id.to_string(),
                done: int(done)?,
                total: int(total).or_else(|| int(estimate)).filter(|t| *t > 0),
                speed: float(speed),
            })
        }
        "done" => serde_json::from_str(rest).ok().map(|f| Line::Done(Box::new(f))),
        "manual" => {
            let tracks: Option<BTreeMap<String, serde_json::Value>> = serde_json::from_str(rest).ok()?;
            Some(Line::Manual(tracks.unwrap_or_default().into_keys().collect()))
        }
        _ => None,
    }
}

fn float(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|f| f.is_finite() && *f >= 0.0)
}

/// Adds up the progress of the formats of one download.
#[derive(Debug, Default)]
struct Tracker {
    /// The sizes from the `fmt` lines, by format ID.
    sizes: BTreeMap<String, u64>,
    /// Bytes on disk and the size, from the `dl` lines, by format ID.
    bytes: BTreeMap<String, (u64, Option<u64>)>,
    speed: Option<f64>,
}

impl Tracker {
    fn add(&mut self, line: &Line) {
        match line {
            Line::Format { format_id, size: Some(size) } => {
                self.sizes.insert(format_id.clone(), *size);
            }
            Line::Bytes { format_id, done, total, speed } => {
                self.bytes.insert(format_id.clone(), (*done, *total));
                self.speed = *speed;
            }
            // A format that was complete before this run can have no
            // progress lines, so count it when its file is done.
            Line::Done(file) => {
                let Some(format_id) = &file.format_id else { return };
                for id in format_id.split('+') {
                    if self.bytes.contains_key(id) {
                        continue;
                    }
                    let size = self.sizes.get(id).copied().or_else(|| {
                        let path = file.filepath.as_ref().filter(|_| !format_id.contains('+'))?;
                        std::fs::metadata(path).ok().map(|m| m.len())
                    });
                    if let Some(size) = size {
                        self.bytes.insert(id.to_string(), (size, Some(size)));
                    }
                }
            }
            Line::Format { .. } | Line::Manual(_) => {}
        }
    }

    /// How many formats are on disk.
    fn complete(&self) -> usize {
        self.bytes.values().filter(|(done, total)| total.is_some_and(|t| *done >= t)).count()
    }

    fn progress(&self) -> Progress {
        let done = self.bytes.values().map(|(done, _)| done).sum();
        // The size of each single format, from its progress if there is any.
        let mut ids: Vec<&String> = self.sizes.keys().filter(|id| !id.contains('+')).collect();
        ids.extend(self.bytes.keys().filter(|id| !self.sizes.contains_key(*id)));
        let sizes: Vec<u64> = ids
            .into_iter()
            .filter_map(|id| self.bytes.get(id).and_then(|(_, total)| *total).or_else(|| self.sizes.get(id).copied()))
            .collect();
        let sum = (!sizes.is_empty()).then(|| sizes.iter().sum::<u64>());
        // A merged download has one size for both formats.
        let merged = self.sizes.iter().filter(|(id, _)| id.contains('+')).map(|(_, size)| *size).max();
        let total = match (sum, merged) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "bytes per second")]
        Progress { done, total, speed: self.speed.map(|s| s as u64) }
    }
}

/// Checks the finished files and gives the download.
fn finish(files: &[DoneFile], format: FileFormat) -> std::result::Result<Saved, String> {
    let nothing = "yt-dlp downloaded nothing. Live streams and videos that have not started cannot be downloaded";
    let first = files.first().ok_or(nothing)?;
    let (video, audio) = match format {
        FileFormat::Separate => {
            let video = files.iter().find(|f| f.has_video() && !f.has_audio());
            let audio = files.iter().find(|f| f.has_audio() && !f.has_video());
            match (video, audio) {
                (Some(v), Some(a)) => (v, Some(a)),
                _ => return Err("yt-dlp did not download both the video and the audio".into()),
            }
        }
        FileFormat::Mkv => {
            let both = files.iter().find(|f| f.has_video() && f.has_audio());
            (both.ok_or("yt-dlp did not merge the video and the audio")?, None)
        }
    };
    let mut size_bytes = 0;
    let mut path = |f: &DoneFile| -> std::result::Result<PathBuf, String> {
        let path = f.filepath.clone().ok_or("yt-dlp did not name the file")?;
        let len = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if len == 0 {
            return Err(format!("{} is empty", path.display()));
        }
        size_bytes += len;
        Ok(path)
    };
    let video_path = path(video)?;
    let audio_path = audio.map(&mut path).transpose()?;
    let id = |f: &DoneFile| f.format_id.clone().unwrap_or_default();
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "frame rates are small")]
    let format_info = FormatInfo {
        id: audio.map_or_else(|| id(video), |a| format!("{}+{}", id(video), id(a))),
        width: video.width,
        height: video.height,
        fps: video.fps.filter(|f| f.is_finite() && *f > 0.0).map(|f| f.round() as u32),
        vcodec: codec(video.vcodec.as_ref()).unwrap_or_default().to_string(),
        acodec: codec(audio.unwrap_or(video).acodec.as_ref()).unwrap_or_default().to_string(),
    };
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "video lengths")]
    let chapters = first
        .chapters
        .iter()
        .flatten()
        .filter_map(|c| {
            let start = c.start_time.filter(|t| t.is_finite() && *t >= 0.0)?;
            Some(Chapter { title: c.title.clone().unwrap_or_default(), start_secs: start as u32 })
        })
        .collect();
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "video lengths")]
    let duration_secs = first.duration.filter(|d| d.is_finite() && *d > 0.0).map(|d| d.round() as u32);
    let channel = first.channel.clone().map(|name| ChannelRef {
        id: first.channel_id.clone(),
        name,
        handle: first.uploader_id.clone().filter(|h| h.starts_with('@')),
    });
    Ok(Saved {
        video: video_path,
        audio: audio_path,
        size_bytes,
        format: format_info,
        title: first.title.clone(),
        channel,
        duration_secs,
        chapters,
        manual_captions: Vec::new(),
    })
}

/// Stops a process and all the processes that it started, such as ffmpeg,
/// when the download stops before its end.
struct Group(Option<i32>);

impl Group {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            // SAFETY: `killpg` only sends a signal. The group is the child's
            // own, made by `process_group(0)`, and the child is not reaped yet.
            unsafe {
                libc::killpg(pgid, libc::SIGTERM);
            }
        }
    }
}

impl YtDlp {
    /// Downloads the caption tracks `langs` of a video into `dir`. A track
    /// that fails is left out, so this can give fewer tracks, or none.
    pub async fn download_captions(
        &self,
        id: &str,
        langs: &[String],
        dir: &std::path::Path,
        temp: &std::path::Path,
    ) -> Result<Vec<CaptionFile>> {
        let program = self.program.display().to_string();
        let child = Command::new(&self.program)
            .args(caption_args(id, langs, dir, temp))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| self.spawn_error(&e))?;
        let output = tokio::time::timeout(CAPTIONS_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| SourceError::Program(format!("{program} did not finish in {} s", CAPTIONS_TIMEOUT.as_secs())))?
            .map_err(|e| SourceError::Program(format!("{program}: {e}")))?;
        // yt-dlp exits with 1 when one track fails, and keeps the others.
        let files = caption_files(&String::from_utf8_lossy(&output.stdout), id, dir);
        if files.is_empty() && !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no output");
            return Err(SourceError::Program(format!("{program} failed: {last}")));
        }
        Ok(files)
    }

    /// Downloads a video, and calls `progress` for each progress line. Drop
    /// the future to stop the download: yt-dlp and its children stop, and
    /// the `.part` files stay for the next run.
    pub async fn download(&self, req: &Request, progress: impl FnMut(Progress) + Send) -> Result<Saved> {
        self.download_with(req, QUIET, progress).await
    }

    async fn download_with(
        &self,
        req: &Request,
        quiet: Duration,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<Saved> {
        let program = self.program.display().to_string();
        let mut child = Command::new(&self.program)
            .args(args(req))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| self.spawn_error(&e))?;
        let mut group = Group(child.id().and_then(|id| i32::try_from(id).ok()));
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        // The last error line, for the message. yt-dlp can print warnings
        // even with `--no-warnings`, so an `ERROR:` line wins over them.
        let errors = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let (mut last, mut error) = (None, None);
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.starts_with("ERROR:") {
                    error = Some(line.clone());
                }
                if !line.is_empty() {
                    last = Some(line);
                }
            }
            error.or(last)
        });
        let mut lines = BufReader::new(stdout).lines();
        let mut tracker = Tracker::default();
        let mut files = Vec::new();
        let mut manual = Vec::new();
        loop {
            // Each download is two formats, a video and an audio.
            let limit = if tracker.complete() >= 2 { MERGE.max(quiet) } else { quiet };
            let line = tokio::time::timeout(limit, lines.next_line())
                .await
                .map_err(|_| SourceError::Program(format!("{program} printed nothing for {} s", limit.as_secs())))?
                .map_err(|e| SourceError::Program(format!("{program}: {e}")))?;
            let Some(line) = line else { break };
            let Some(line) = parse_line(&line) else { continue };
            tracker.add(&line);
            match line {
                Line::Done(file) => {
                    files.push(*file);
                    progress(tracker.progress());
                }
                Line::Bytes { .. } => progress(tracker.progress()),
                Line::Manual(langs) => manual = langs,
                Line::Format { .. } => {}
            }
        }
        let status = child.wait().await.map_err(|e| SourceError::Program(format!("{program}: {e}")))?;
        group.disarm();
        let last = errors.await.ok().flatten();
        if !status.success() {
            return Err(SourceError::Program(match last {
                Some(line) => format!("{program} failed: {line}"),
                None => format!("{program} failed: {status}"),
            }));
        }
        let saved = finish(&files, req.format).map(|saved| Saved { manual_captions: manual, ..saved });
        saved.map_err(|e| {
            // For example only the audio, when the video has no video stream
            // that fits the rules. The files are ours, named by the ID.
            for path in files.iter().filter_map(|f| f.filepath.as_ref()) {
                let _ = std::fs::remove_file(path);
            }
            SourceError::Program(e)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn request(dir: &Path, format: FileFormat) -> Request {
        Request { id: "aqz-KE-bpKQ".into(), max_height: 720, format, dir: dir.into(), temp: dir.join(".part") }
    }

    #[test]
    fn args_name_the_folders_formats_and_files() {
        let args = args(&request(Path::new("/d"), FileFormat::Separate));
        let after = |flag: &str| args.iter().skip_while(|a| *a != flag).nth(1).cloned().unwrap();
        assert_eq!(after("-f"), streams::download_selector(720, false));
        assert_eq!(after("-S"), "res:720,vcodec:avc1");
        assert_eq!(after("-o"), "%(id)s.f%(format_id)s.%(ext)s");
        assert_eq!(after("--fixup"), "never");
        assert_eq!(after("--extractor-args"), PLAYER_CLIENTS);
        assert!(args.windows(2).any(|w| w == ["-P", "home:/d"]));
        assert!(args.windows(2).any(|w| w == ["-P", "temp:/d/.part"]));
        assert_eq!(args.first().unwrap(), "--ignore-config");
        assert_eq!(args.last().unwrap(), "https://www.youtube.com/watch?v=aqz-KE-bpKQ");
        assert!(!args.iter().any(|a| a == "--merge-output-format"));

        let args = super::args(&request(Path::new("/d"), FileFormat::Mkv));
        let after = |flag: &str| args.iter().skip_while(|a| *a != flag).nth(1).cloned().unwrap();
        assert_eq!(after("-f"), streams::download_selector(720, true));
        assert_eq!(after("--merge-output-format"), "mkv");
        assert_eq!(after("-o"), "%(id)s.%(ext)s");
    }

    #[test]
    fn caption_langs_follow_the_player() {
        let manual = |langs: &[&str]| langs.iter().map(|l| (*l).to_string()).collect::<Vec<_>>();
        assert_eq!(caption_langs("en", &manual(&["de", "en"])), ["en", "en-orig"]);
        assert_eq!(caption_langs("en", &manual(&["en-GB", "en-US"])), ["en-GB", "en-orig"]);
        assert_eq!(caption_langs("pt-br", &manual(&["pt", "pt-BR"])), ["pt-BR", "pt-orig"]);
        assert_eq!(caption_langs("pt-BR", &manual(&["pt"])), ["pt", "pt-orig"]);
        assert_eq!(caption_langs("de", &manual(&["en"])), ["de-orig", "de"]);
        assert_eq!(caption_langs("en", &[]), ["en-orig", "en"]);
    }

    #[test]
    fn caption_args_ask_for_exact_languages() {
        let langs = ["de".to_string(), "de-orig".to_string()];
        let args = caption_args("x", &langs, Path::new("/d"), Path::new("/d/.part"));
        let after = |flag: &str| args.iter().skip_while(|a| *a != flag).nth(1).cloned().unwrap();
        assert_eq!(after("--sub-langs"), "de,de-orig");
        assert!(args.windows(2).any(|w| w == ["-P", "home:/d"]));
        assert!(args.windows(2).any(|w| w == ["-P", "temp:/d/.part"]));
        assert_eq!(after("-o"), "%(id)s.%(ext)s");
        assert!(args.iter().any(|a| a == "--skip-download") && args.iter().any(|a| a == "--no-simulate"));
        assert_eq!(args.last().unwrap(), "https://www.youtube.com/watch?v=x");
    }

    #[test]
    fn caption_files_are_the_tracks_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.en.vtt"), b"WEBVTT").unwrap();
        std::fs::write(dir.path().join("x.en-orig.vtt"), b"WEBVTT").unwrap();
        let out = "[youtube] x: Downloading\nRT\tsubs\t{\"requested_subtitles\": {\
            \"en\": {\"ext\": \"vtt\", \"name\": \"English\"}, \
            \"en-orig\": {\"ext\": \"vtt\", \"name\": \"English (Original)\"}, \
            \"fr\": {\"ext\": \"vtt\", \"name\": \"French\"}}, \"subtitles\": {\"en\": []}}\n";
        let files = caption_files(out, "x", dir.path());
        let got: Vec<(&str, &str, bool)> = files.iter().map(|f| (f.lang.as_str(), f.name.as_str(), f.auto)).collect();
        // fr failed, so it has no file.
        assert_eq!(got, [("en", "English", false), ("en-orig", "English (Original)", true)]);
        assert_eq!(files[0].path, dir.path().join("x.en.vtt"));
        assert!(caption_files("nothing", "x", dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn captions_that_partly_fail_keep_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        let y = script(
            &dir,
            &format!(
                r#"printf 'WEBVTT' > '{d}/x.en.vtt'
printf 'RT\tsubs\t{{"requested_subtitles": {{"en": {{"ext": "vtt", "name": "English"}}, "en-orig": {{"ext": "vtt"}}}}}}\n'
echo 'ERROR: Unable to download video subtitles for en-orig: HTTP Error 429' >&2; exit 1"#
            ),
        );
        let langs = caption_langs("en", &[]);
        let files = y.download_captions("x", &langs, dir.path(), dir.path()).await.unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].auto, "not in the list of manual tracks");
        let y = script(&dir, "echo 'ERROR: offline' >&2; exit 1");
        let err = y.download_captions("x", &langs, dir.path(), dir.path()).await.unwrap_err();
        assert!(err.to_string().ends_with("failed: ERROR: offline"), "{err}");
    }

    #[test]
    fn sweep_keeps_the_files_of_downloads_that_go_on() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["keep.f137.mp4.part", "keep.f251.webm.part", "gone.f137.mp4.part", "gone.temp.mkv"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("gone.dir")).unwrap();
        assert_eq!(sweep(dir.path(), |id| id == "keep"), 2);
        let mut left: Vec<String> =
            std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        left.sort();
        assert_eq!(left, ["gone.dir", "keep.f137.mp4.part", "keep.f251.webm.part"]);
        assert_eq!(sweep(&dir.path().join("missing"), |_| false), 0);
    }

    #[test]
    fn sizes_as_people_read_them() {
        let cases = [
            (0, "0 B"),
            (999, "999 B"),
            (1000, "1 KB"),
            (980_400, "980 KB"),
            (999_600, "1 MB"),
            (412_000_000, "412 MB"),
            (3_420_000_000, "3.4 GB"),
            (9_960_000_000, "10 GB"),
            (12_400_000_000, "12 GB"),
            (2_500_000_000_000, "2.5 TB"),
        ];
        for (bytes, text) in cases {
            assert_eq!(size_text(bytes), text, "{bytes}");
        }
    }

    #[test]
    fn parses_lines() {
        assert_eq!(
            parse_line("RT\tfmt\t{\"format_id\": \"134\", \"filesize\": 18294110, \"filesize_approx\": 18294061}\n"),
            Some(Line::Format { format_id: "134".into(), size: Some(18_294_110) })
        );
        assert_eq!(
            parse_line("RT\tfmt\t{\"format_id\": \"134+251\", \"filesize_approx\": 28496320.5}"),
            Some(Line::Format { format_id: "134+251".into(), size: Some(28_496_320) })
        );
        assert_eq!(
            parse_line("RT\tdl\t140\t1024\t10271496\tNA\tNA"),
            Some(Line::Bytes { format_id: "140".into(), done: 1024, total: Some(10_271_496), speed: None })
        );
        assert_eq!(
            parse_line("RT\tdl\t616\t2048\tNA\t9000.7\t1500.5"),
            Some(Line::Bytes { format_id: "616".into(), done: 2048, total: Some(9000), speed: Some(1500.5) })
        );
        let Some(Line::Done(f)) = parse_line(
            "RT\tdone\t{\"filepath\": \"/d/x.f134.mp4\", \"format_id\": \"134\", \"vcodec\": \"avc1.4D401E\", \
             \"acodec\": \"none\", \"chapters\": null}",
        ) else {
            panic!("a done line")
        };
        assert_eq!(f.filepath.as_deref(), Some(Path::new("/d/x.f134.mp4")));
        assert!(f.has_video() && !f.has_audio());
        for junk in [
            "",
            "RT",
            "RT\tdl\tx",
            "RT\tdl\t1\tNA\tNA\tNA\tNA",
            "RT\tother\t{}",
            "[youtube] x: Downloading",
            "RT\tfmt\t{",
        ] {
            assert_eq!(parse_line(junk), None, "{junk:?}");
        }
    }

    fn bytes(id: &str, done: u64, total: Option<u64>) -> Line {
        Line::Bytes { format_id: id.into(), done, total, speed: Some(10.0) }
    }

    #[test]
    fn tracker_adds_up_separate_files() {
        let mut t = Tracker::default();
        t.add(&Line::Format { format_id: "134".into(), size: Some(1000) });
        assert_eq!(t.progress().total, Some(1000));
        t.add(&bytes("134", 500, Some(1100)));
        assert_eq!((t.progress().done, t.progress().total), (500, Some(1100)));
        t.add(&bytes("134", 1100, Some(1100)));
        t.add(&Line::Format { format_id: "140".into(), size: Some(400) });
        t.add(&bytes("140", 100, None));
        let p = t.progress();
        assert_eq!((p.done, p.total, p.speed), (1200, Some(1500), Some(10)));
        assert!((p.fraction().unwrap() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn tracker_uses_the_merged_size() {
        let mut t = Tracker::default();
        t.add(&Line::Format { format_id: "134+251".into(), size: Some(3000) });
        t.add(&bytes("134", 1000, Some(2000)));
        assert_eq!(t.progress().total, Some(3000));
        t.add(&bytes("134", 2000, Some(2000)));
        t.add(&bytes("251", 1500, Some(1500)));
        assert_eq!((t.progress().done, t.progress().total), (3500, Some(3500)));
        assert_eq!(Progress { done: 9, total: Some(3), speed: None }.fraction(), Some(1.0));
        assert_eq!(Progress::default().fraction(), None);
    }

    #[test]
    fn tracker_counts_formats_that_were_already_complete() {
        // A merged download where the video was complete before this run:
        // yt-dlp prints its size as the bytes done, then the audio.
        let mut t = Tracker::default();
        t.add(&Line::Format { format_id: "137+251".into(), size: Some(3000) });
        t.add(&parse_line("RT\tdl\t137\t2000\t2000\tNA\tNA").unwrap());
        t.add(&bytes("251", 1000, Some(1000)));
        assert_eq!(t.complete(), 2);
        assert_eq!(t.progress().fraction(), Some(1.0));

        // Separate files where the video was already in its folder: no
        // progress lines, only its done line.
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("x.f136.mp4");
        std::fs::write(&v, b"12345").unwrap();
        let mut t = Tracker::default();
        t.add(&Line::Done(Box::new(done(&v, "136", "avc1", "none"))));
        t.add(&Line::Format { format_id: "251".into(), size: Some(5) });
        t.add(&bytes("251", 5, Some(5)));
        let p = t.progress();
        assert_eq!((p.done, p.total, t.complete()), (10, Some(10), 2));
        // A done line for a format with progress changes nothing.
        t.add(&Line::Done(Box::new(done(&v, "251", "none", "opus"))));
        assert_eq!(t.progress().done, 10);
    }

    fn done(path: &Path, id: &str, vcodec: &str, acodec: &str) -> DoneFile {
        serde_json::from_value(serde_json::json!({
            "filepath": path, "format_id": id, "vcodec": vcodec, "acodec": acodec,
            "width": 1280, "height": 720, "fps": 29.97, "duration": 634.5, "title": "Big Buck Bunny",
            "channel": "Blender", "channel_id": "UCx", "uploader_id": "@blender",
            "chapters": [{"title": "Start", "start_time": 0.0}, {"title": "bad"}],
        }))
        .unwrap()
    }

    #[test]
    fn finish_checks_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let (v, a) = (dir.path().join("x.f136.mp4"), dir.path().join("x.f251.webm"));
        std::fs::write(&v, b"video").unwrap();
        std::fs::write(&a, b"aud").unwrap();
        let files = [done(&v, "136", "avc1", "none"), done(&a, "251", "none", "opus")];
        let saved = finish(&files, FileFormat::Separate).unwrap();
        assert_eq!((saved.video.as_path(), saved.audio.as_deref()), (v.as_path(), Some(a.as_path())));
        assert_eq!(saved.size_bytes, 8);
        assert_eq!(saved.format.id, "136+251");
        assert_eq!((saved.format.vcodec.as_str(), saved.format.acodec.as_str()), ("avc1", "opus"));
        assert_eq!((saved.format.height, saved.format.fps), (Some(720), Some(30)));
        assert_eq!(saved.duration_secs, Some(635));
        assert_eq!(saved.chapters, [Chapter { title: "Start".into(), start_secs: 0 }]);
        let channel = saved.channel.unwrap();
        assert_eq!((channel.id.as_deref(), channel.handle.as_deref()), (Some("UCx"), Some("@blender")));
        assert_eq!(saved.title.as_deref(), Some("Big Buck Bunny"));

        // The `Mkv` format needs one file with both.
        assert!(finish(&files, FileFormat::Mkv).unwrap_err().contains("did not merge"));
        let mkv = [done(&v, "136+251", "avc1", "opus")];
        let saved = finish(&mkv, FileFormat::Mkv).unwrap();
        assert_eq!((saved.audio, saved.format.id.as_str()), (None, "136+251"));

        assert!(finish(&[], FileFormat::Separate).unwrap_err().contains("downloaded nothing"));
        assert!(finish(&files[..1], FileFormat::Separate).unwrap_err().contains("both"));
        let missing = [done(&dir.path().join("gone.mp4"), "136", "avc1", "none"), files[1].clone()];
        assert!(finish(&missing, FileFormat::Separate).unwrap_err().contains("gone.mp4"));
        let empty = dir.path().join("empty.mp4");
        std::fs::write(&empty, b"").unwrap();
        let empty = [done(&empty, "136", "avc1", "none"), files[1].clone()];
        assert!(finish(&empty, FileFormat::Separate).unwrap_err().contains("is empty"));
    }

    /// Writes a fake yt-dlp, as in the yt-dlp tests.
    #[cfg(unix)]
    fn script(dir: &tempfile::TempDir, body: &str) -> YtDlp {
        let source = dir.path().join("fake-yt-dlp.txt");
        let path = dir.path().join("fake-yt-dlp");
        std::fs::write(&source, format!("#!/bin/sh\n{body}\n")).unwrap();
        let status =
            std::process::Command::new("install").args(["-m", "755"]).arg(&source).arg(&path).status().unwrap();
        assert!(status.success());
        YtDlp { program: path }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn downloads_and_reports_progress() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        let y = script(
            &dir,
            &format!(
                r#"printf 'v' > '{d}/x.f136.mp4'; printf 'au' > '{d}/x.f251.webm'
echo '[youtube] x: Downloading webpage'
printf 'RT\tfmt\t{{"format_id": "136", "filesize": 10}}\n'
printf 'RT\tdl\t136\t5\t10\tNA\t2.5\n'
printf 'RT\tdl\t136\t10\t10\tNA\t2.5\n'
printf 'RT\tdone\t{{"filepath": "{d}/x.f136.mp4", "format_id": "136", "vcodec": "avc1", "acodec": "none"}}\n'
printf 'RT\tdl\t251\t4\t4\tNA\tNA\n'
printf 'RT\tdone\t{{"filepath": "{d}/x.f251.webm", "format_id": "251", "vcodec": "none", "acodec": "opus"}}\n'
echo 'WARNING: Unknown codec' >&2"#
            ),
        );
        let mut seen = Vec::new();
        let saved = y.download(&request(dir.path(), FileFormat::Separate), |p| seen.push(p)).await.unwrap();
        assert_eq!(saved.size_bytes, 3);
        assert_eq!(saved.format.id, "136+251");
        let done: Vec<(u64, Option<u64>)> = seen.iter().map(|p| (p.done, p.total)).collect();
        assert_eq!(done, [(5, Some(10)), (10, Some(10)), (10, Some(10)), (14, Some(14)), (14, Some(14))]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failures_give_the_error_line() {
        let dir = tempfile::tempdir().unwrap();
        let y = script(&dir, "echo 'ERROR: [youtube] x: Video unavailable' >&2; echo 'WARNING: later' >&2; exit 1");
        let err = y.download(&request(dir.path(), FileFormat::Separate), |_| {}).await.unwrap_err();
        assert!(err.to_string().ends_with("failed: ERROR: [youtube] x: Video unavailable"), "{err}");

        // A live stream is skipped by the filter, and yt-dlp exits with 0.
        let dir = tempfile::tempdir().unwrap();
        let y = script(&dir, "echo 'x does not pass filter' >&2");
        let err = y.download(&request(dir.path(), FileFormat::Separate), |_| {}).await.unwrap_err();
        assert!(err.to_string().contains("downloaded nothing"), "{err}");

        let y = YtDlp { program: "/nonexistent/yt-dlp".into() };
        let err = y.download(&request(dir.path(), FileFormat::Separate), |_| {}).await.unwrap_err();
        assert!(err.to_string().contains("/nonexistent/yt-dlp was not found"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_incomplete_download_removes_its_files() {
        // Only the audio, because the video has no stream that fits.
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("x.f251.webm");
        let y = script(
            &dir,
            &format!(
                r#"printf 'au' > '{a}'
printf 'RT\tdone\t{{"filepath": "{a}", "format_id": "251", "vcodec": "none", "acodec": "opus"}}\n'"#,
                a = a.display()
            ),
        );
        let err = y.download(&request(dir.path(), FileFormat::Separate), |_| {}).await.unwrap_err();
        assert!(err.to_string().contains("both"), "{err}");
        assert!(!a.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_merge_can_be_quiet_for_longer() {
        // Both formats are on disk, then ffmpeg merges them for a while.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.mkv");
        let y = script(
            &dir,
            &format!(
                r#"printf 'RT\tdl\t137\t2\t2\tNA\tNA\nRT\tdl\t251\t1\t1\tNA\tNA\n'
sleep 1; printf 'av' > '{f}'
printf 'RT\tdone\t{{"filepath": "{f}", "format_id": "137+251", "vcodec": "avc1", "acodec": "opus"}}\n'"#,
                f = f.display()
            ),
        );
        let req = request(dir.path(), FileFormat::Mkv);
        let saved = y.download_with(&req, Duration::from_millis(200), |_| {}).await.unwrap();
        assert_eq!(saved.video, f);
    }

    /// Whether a process is still there, and not a zombie.
    #[cfg(target_os = "linux")]
    fn alive(pid: i32) -> bool {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        !stat.is_empty() && !stat.rsplit(')').next().unwrap_or_default().trim_start().starts_with('Z')
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn stopping_kills_the_children_too() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let y = script(
            &dir,
            &format!("sleep 300 & echo $! > '{}'; printf 'RT\\tdl\\t1\\t1\\t2\\tNA\\tNA\\n'; wait", pid_file.display()),
        );
        let req = request(dir.path(), FileFormat::Separate);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut tx = Some(tx);
        let task = tokio::spawn(async move {
            y.download(&req, move |_| {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(());
                }
            })
            .await
        });
        rx.await.unwrap();
        task.abort();
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        let mut gone = false;
        for _ in 0..100 {
            if !alive(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone, "the child of yt-dlp still runs");
    }

    /// Downloads a 19-second video in both formats with the real yt-dlp.
    /// The `Mkv` format needs ffmpeg. Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "uses the network"]
    async fn live_download() {
        let y = YtDlp { program: "yt-dlp".into() };
        for format in [FileFormat::Separate, FileFormat::Mkv] {
            let dir = tempfile::tempdir().unwrap();
            let req = Request { id: "jNQXAC9IVRw".into(), max_height: 240, ..request(dir.path(), format) };
            let mut last = Progress::default();
            let saved = y.download(&req, |p| last = p).await.unwrap();
            assert_eq!(saved.audio.is_some(), format == FileFormat::Separate, "{saved:?}");
            assert!(saved.video.starts_with(dir.path()), "{saved:?}");
            assert_eq!(saved.duration_secs, Some(19));
            assert_eq!(last.fraction(), Some(1.0), "{last:?}");
            // The temporary folder is empty when the download is complete.
            let left = std::fs::read_dir(&req.temp).map_or_default(Iterator::count);
            assert_eq!(left, 0);
        }
    }

    /// Downloads the English captions of a video with the real yt-dlp.
    /// Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "uses the network"]
    async fn live_captions() {
        let y = YtDlp { program: "yt-dlp".into() };
        let dir = tempfile::tempdir().unwrap();
        let langs = caption_langs("en", &["en".to_string()]);
        let files = y.download_captions("aircAruvnKk", &langs, dir.path(), dir.path()).await.unwrap();
        let langs: Vec<(&str, bool)> = files.iter().map(|f| (f.lang.as_str(), f.auto)).collect();
        assert!(langs.contains(&("en", false)), "{files:?}");
        assert!(files.iter().all(|f| std::fs::read_to_string(&f.path).unwrap().starts_with("WEBVTT")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_silent_download_stops() {
        let dir = tempfile::tempdir().unwrap();
        let y = script(&dir, "sleep 300");
        let req = request(dir.path(), FileFormat::Separate);
        let err = y.download_with(&req, Duration::from_millis(200), |_| {}).await.unwrap_err();
        assert!(err.to_string().contains("printed nothing"), "{err}");
    }
}
