//! The yt-dlp source: the fallback for metadata.
//!
//! Search runs `yt-dlp -J --flat-playlist "ytsearch20:<text>"` and reads the
//! `entries`. yt-dlp cannot apply search filters or give a next page.
//!
//! Channels and playlists use `--flat-playlist` too, so one call gives the
//! list and its first videos. Video details use `--no-playlist`. yt-dlp gives
//! exact numbers, so the text that YouTube would show is built here.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Deserialize;
use tokio::process::Command;

use crate::source::model::{
    ChannelInfo, ChannelRef, Chapter, Description, Item, ItemPage, PlaylistInfo, Thumbnail, Video, VideoKind,
};
use crate::source::{ChannelPage, PlaylistPage, Result, SearchPage, SearchQuery, Source, SourceError, VideoDetails};

/// How many results one search asks for.
const RESULTS: u32 = 20;
/// How many videos a channel page asks for. There is no next page.
const CHANNEL_VIDEOS: &str = "1:30";
/// How many videos a playlist page asks for. There is no next page.
const PLAYLIST_VIDEOS: &str = "1:200";
const TIMEOUT: Duration = Duration::from_secs(60);

pub struct YtDlp {
    pub program: PathBuf,
}

impl Source for YtDlp {
    fn name(&self) -> &'static str {
        "yt-dlp"
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>> {
        Box::pin(async move {
            let target = format!("ytsearch{RESULTS}:{}", query.text);
            let stdout = self.run(&["-J", "--flat-playlist", "--no-warnings", &target]).await?;
            let mut page = SearchPage { items: parse_search(&stdout)?, ..SearchPage::default() };
            if !query.filters.is_default() {
                page.note = Some("yt-dlp ignores search filters, so these results are not filtered".into());
            }
            Ok(page)
        })
    }

    fn search_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<SearchPage>> {
        let _ = continuation;
        Box::pin(async { Err(SourceError::Unsupported("yt-dlp cannot load more search results".into())) })
    }

    fn channel<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ChannelPage>> {
        Box::pin(async move {
            let url = format!("https://www.youtube.com/channel/{id}/videos");
            let args = ["-J", "--flat-playlist", "--no-warnings", "--playlist-items", CHANNEL_VIDEOS, &url];
            parse_channel(&self.run(&args).await?)
        })
    }

    fn playlist<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<PlaylistPage>> {
        Box::pin(async move {
            // Callers may pass the `VL` browse ID; the playlist URL takes the bare ID.
            let id = id.strip_prefix("VL").unwrap_or(id);
            let url = format!("https://www.youtube.com/playlist?list={id}");
            let args = ["-J", "--flat-playlist", "--no-warnings", "--playlist-items", PLAYLIST_VIDEOS, &url];
            parse_playlist(&self.run(&args).await?)
        })
    }

    fn video<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VideoDetails>> {
        Box::pin(async move {
            let url = format!("https://www.youtube.com/watch?v={id}");
            let args = ["-J", "--no-playlist", "--skip-download", "--no-warnings", &url];
            parse_video(&self.run(&args).await?)
        })
    }
}

impl YtDlp {
    /// Runs the program and returns its standard output.
    async fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let program = self.program.display();
        let child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SourceError::Program(format!("{program} was not found. Install yt-dlp or set yt_dlp in the config"))
                } else {
                    SourceError::Program(format!("cannot run {program}: {e}"))
                }
            })?;
        let output = tokio::time::timeout(TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| SourceError::Program(format!("{program} did not finish in {} s", TIMEOUT.as_secs())))?
            .map_err(|e| SourceError::Program(format!("{program}: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty());
            return Err(SourceError::Program(match last {
                Some(line) => format!("{program} failed: {line}"),
                None => format!("{program} failed: {}", output.status),
            }));
        }
        Ok(output.stdout)
    }
}

/// What `yt-dlp -J` prints: a list (a search, a channel tab, a playlist)
/// with `entries`, or one video. Any field can be missing or `null`.
#[derive(Deserialize)]
struct Info {
    id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    channel: Option<String>,
    channel_id: Option<String>,
    uploader: Option<String>,
    /// A handle such as `@name`, or an older user ID.
    uploader_id: Option<String>,
    channel_follower_count: Option<u64>,
    channel_is_verified: Option<bool>,
    playlist_count: Option<u64>,
    view_count: Option<u64>,
    /// People watching a live stream now.
    concurrent_view_count: Option<u64>,
    like_count: Option<u64>,
    /// `YYYYMMDD`.
    modified_date: Option<String>,
    /// `YYYYMMDD`.
    upload_date: Option<String>,
    duration: Option<f64>,
    is_live: Option<bool>,
    live_status: Option<String>,
    chapters: Option<Vec<Option<YtChapter>>>,
    thumbnails: Option<Vec<Option<EntryThumbnail>>>,
    /// yt-dlp writes `null` for entries it could not read.
    entries: Option<Vec<Option<Entry>>>,
}

#[derive(Deserialize)]
struct Entry {
    id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    duration: Option<f64>,
    channel: Option<String>,
    channel_id: Option<String>,
    uploader: Option<String>,
    uploader_id: Option<String>,
    view_count: Option<u64>,
    live_status: Option<String>,
    availability: Option<String>,
    #[serde(default)]
    thumbnails: Option<Vec<Option<EntryThumbnail>>>,
}

#[derive(Deserialize)]
struct EntryThumbnail {
    url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    /// `0`, `avatar_uncropped`, `banner_uncropped`, ...
    id: Option<String>,
}

#[derive(Deserialize)]
struct YtChapter {
    title: Option<String>,
    start_time: Option<f64>,
}

fn parse_info(json: &[u8]) -> Result<Info> {
    serde_json::from_slice(json).map_err(|e| SourceError::Parse(format!("yt-dlp output: {e}")))
}

/// Reads the output of `yt-dlp -J --flat-playlist ytsearch...`. Entries without an ID are skipped.
pub fn parse_search(json: &[u8]) -> Result<Vec<Item>> {
    Ok(videos(parse_info(json)?.entries))
}

/// Reads the output of `yt-dlp -J --flat-playlist <channel>/videos`.
///
/// The avatar is the square thumbnails and the one with an `avatar` ID. The
/// others are banners. yt-dlp does not give a video count.
pub fn parse_channel(json: &[u8]) -> Result<ChannelPage> {
    let info = parse_info(json)?;
    let id = info.channel_id.or(info.id).ok_or_else(|| SourceError::Parse("yt-dlp gave no channel ID".into()))?;
    let me = ChannelRef {
        id: Some(id.clone()),
        name: info.channel.or(info.uploader).unwrap_or_default(),
        handle: handle(info.uploader_id),
    };
    let mut items = videos(info.entries);
    // The entries of a channel tab do not name the channel.
    for item in &mut items {
        if let Item::Video(v) = item
            && v.channels.is_empty()
        {
            v.channels.push(me.clone());
        }
    }
    let avatar = info.thumbnails.into_iter().flatten().flatten().filter(is_avatar).filter_map(thumbnail).collect();
    Ok(ChannelPage {
        info: ChannelInfo {
            id,
            name: me.name,
            handle: me.handle,
            subscribers_text: info.channel_follower_count.map(subscribers_text),
            videos_text: None,
            description: info.description.filter(|d| !d.is_empty()),
            verified: info.channel_is_verified.unwrap_or(false),
            avatar,
        },
        videos: ItemPage { items, continuation: None },
    })
}

fn is_avatar(t: &EntryThumbnail) -> bool {
    let square = matches!((t.width, t.height), (Some(w), Some(h)) if w > 0 && w == h);
    square || t.id.as_deref().is_some_and(|id| id.starts_with("avatar"))
}

/// Reads the output of `yt-dlp -J --flat-playlist <playlist URL>`.
///
/// Entries with no title are private or deleted videos. YouTube hides them,
/// so they are skipped here too.
pub fn parse_playlist(json: &[u8]) -> Result<PlaylistPage> {
    let mut info = parse_info(json)?;
    let id = info.id.ok_or_else(|| SourceError::Parse("yt-dlp gave no playlist ID".into()))?;
    if let Some(entries) = &mut info.entries {
        entries.retain(|e| e.as_ref().is_some_and(|e| e.title.is_some()));
    }
    Ok(PlaylistPage {
        info: PlaylistInfo {
            id,
            title: info.title.unwrap_or_default(),
            channel: channel_ref(info.channel.or(info.uploader), info.channel_id, info.uploader_id),
            videos_text: info.playlist_count.map(|n| counted(n, "video", "videos")),
            views_text: info.view_count.map(views_text),
            updated_text: info.modified_date.as_deref().and_then(date_text).map(|d| format!("Last updated on {d}")),
            description: info.description.filter(|d| !d.is_empty()),
            thumbnails: info.thumbnails.into_iter().flatten().flatten().filter_map(thumbnail).collect(),
        },
        videos: ItemPage { items: videos(info.entries), continuation: None },
    })
}

/// Reads the output of `yt-dlp -J --no-playlist <video URL>`.
///
/// The description has no links: only InnerTube marks them. Only
/// thumbnails with a known size are kept; the others are copies in other
/// formats.
pub fn parse_video(json: &[u8]) -> Result<VideoDetails> {
    let info = parse_info(json)?;
    let id = info.id.ok_or_else(|| SourceError::Parse("yt-dlp gave no video ID".into()))?;
    let live = info.is_live == Some(true) || info.live_status.as_deref() == Some("is_live");
    let day = info.upload_date.as_deref();
    let date_text = day.and_then(date_text).map(|d| match info.live_status.as_deref() {
        _ if live => format!("Started streaming on {d}"),
        Some("was_live") => format!("Streamed live on {d}"),
        _ => d,
    });
    let views_text = if live {
        info.concurrent_view_count.map(|n| format!("{} watching now", grouped(n)))
    } else {
        info.view_count.map(views_text)
    };
    let mut chapters: Vec<Chapter> = info
        .chapters
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|c| {
            let start = c.start_time.filter(|s| s.is_finite() && *s >= 0.0)?;
            Some(Chapter { title: c.title.unwrap_or_default(), start_secs: round_secs(start) })
        })
        .collect();
    chapters.sort_by_key(|c| c.start_secs);
    Ok(VideoDetails {
        id,
        title: info.title.unwrap_or_default(),
        channel: channel_ref(info.channel.or(info.uploader), info.channel_id, info.uploader_id),
        subscribers_text: info.channel_follower_count.map(subscribers_text),
        views_text,
        date_text,
        publish_date: day.and_then(iso_date),
        duration_secs: if live { None } else { info.duration.filter(|d| d.is_finite() && *d >= 0.0).map(round_secs) },
        likes_text: info.like_count.map(compact),
        live,
        description: Description { text: info.description.unwrap_or_default(), links: Vec::new() },
        chapters,
        thumbnails: info
            .thumbnails
            .into_iter()
            .flatten()
            .flatten()
            .filter(|t| t.width.is_some() && t.height.is_some())
            .filter_map(thumbnail)
            .collect(),
    })
}

fn videos(entries: Option<Vec<Option<Entry>>>) -> Vec<Item> {
    entries.into_iter().flatten().flatten().filter_map(video).map(Item::Video).collect()
}

fn video(e: Entry) -> Option<Video> {
    let id = e.id?;
    let kind = match e.live_status.as_deref() {
        Some("is_live") => VideoKind::Live,
        Some("was_live") => VideoKind::PastStream,
        _ => VideoKind::Normal,
    };
    let duration_secs = e.duration.filter(|d| d.is_finite() && *d >= 0.0).map(round_secs);
    Some(Video {
        id,
        title: e.title.unwrap_or_default(),
        kind,
        channels: channel_ref(e.channel.or(e.uploader), e.channel_id, e.uploader_id).into_iter().collect(),
        duration_text: duration_secs.map(duration_text),
        duration_secs,
        views_text: e.view_count.map(views_text),
        published_text: None,
        thumbnails: e.thumbnails.into_iter().flatten().flatten().filter_map(thumbnail).collect(),
        members_only: e.availability.as_deref() == Some("subscriber_only"),
        badges: Vec::new(),
        description_snippet: e.description.filter(|d| !d.is_empty()),
    })
}

/// `None` when there is neither a name nor an ID.
fn channel_ref(name: Option<String>, id: Option<String>, uploader_id: Option<String>) -> Option<ChannelRef> {
    if name.is_none() && id.is_none() {
        return None;
    }
    Some(ChannelRef { id, name: name.unwrap_or_default(), handle: handle(uploader_id) })
}

/// A handle starts with `@`. Older uploader IDs are not handles.
fn handle(uploader_id: Option<String>) -> Option<String> {
    uploader_id.filter(|h| h.starts_with('@'))
}

fn thumbnail(t: EntryThumbnail) -> Option<Thumbnail> {
    let raw = t.url?;
    let url = if raw.starts_with("https://") {
        raw
    } else {
        let rest = raw.strip_prefix("//")?;
        format!("https://{rest}")
    };
    Some(Thumbnail { url, width: t.width.unwrap_or(0), height: t.height.unwrap_or(0) })
}

/// `d` is finite and not negative.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_secs(d: f64) -> u32 {
    d.round().min(f64::from(u32::MAX)) as u32
}

/// "4:05", "1:02:03".
fn duration_text(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// "1,234,567".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "1 video", "1,234 videos".
fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// "1 view", "1,234 views".
fn views_text(n: u64) -> String {
    counted(n, "view", "views")
}

/// "999 subscribers", "1.2K subscribers", "8.73M subscribers", as YouTube
/// shows them.
fn subscribers_text(n: u64) -> String {
    if n < 1000 { counted(n, "subscriber", "subscribers") } else { format!("{} subscribers", compact(n)) }
}

/// A short count with three digits at most: "999", "1.2K", "568K",
/// "8.73M". Rounded down, as YouTube does.
fn compact(n: u64) -> String {
    let (unit, suffix) = match n {
        1_000_000_000.. => (1_000_000_000, "B"),
        1_000_000.. => (1_000_000, "M"),
        1_000.. => (1_000, "K"),
        _ => return n.to_string(),
    };
    let (decimals, scale) = match n / unit {
        0..=9 => (2, 100),
        10..=99 => (1, 10),
        _ => (0, 1),
    };
    let fixed = n / (unit / scale);
    let mut out = (fixed / scale).to_string();
    let fraction = format!("{:0decimals$}", fixed % scale);
    let fraction = fraction.trim_end_matches('0');
    if !fraction.is_empty() {
        out.push('.');
        out.push_str(fraction);
    }
    out.push_str(suffix);
    out
}

/// The year, month, and day of a `YYYYMMDD` date.
fn ymd(s: &str) -> Option<(u32, u32, u32)> {
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, m, d) = (s[..4].parse().ok()?, s[4..6].parse().ok()?, s[6..].parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// `YYYYMMDD` to "Oct 5, 2017".
fn date_text(s: &str) -> Option<String> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, m, d) = ymd(s)?;
    let month = MONTHS[usize::try_from(m - 1).ok()?];
    Some(format!("{month} {d}, {y}"))
}

/// `YYYYMMDD` to "2017-10-05".
fn iso_date(s: &str) -> Option<String> {
    let (y, m, d) = ymd(s)?;
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use crate::source::filters::{SearchFilters, Sort};

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/ytdlp/search.json");

    fn summary(items: &[Item]) -> String {
        let mut out = String::new();
        for item in items {
            let Item::Video(v) = item else { panic!("not a video: {item:?}") };
            let channels: Vec<String> = v
                .channels
                .iter()
                .map(|c| {
                    format!("{} {} {}", c.name, c.handle.as_deref().unwrap_or("-"), c.id.as_deref().unwrap_or("-"))
                })
                .collect();
            let thumbs: Vec<String> = v.thumbnails.iter().map(|t| format!("{}x{}", t.width, t.height)).collect();
            writeln!(
                out,
                "{} {:?} {} ({}s) | {} | {} | thumbs {} | members {}\n  {}",
                v.id,
                v.kind,
                v.duration_text.as_deref().unwrap_or("-"),
                v.duration_secs.map_or("-".into(), |s| s.to_string()),
                v.views_text.as_deref().unwrap_or("-"),
                channels.join(", "),
                thumbs.join(" "),
                v.members_only,
                v.title,
            )
            .unwrap();
        }
        out
    }

    #[test]
    fn parses_the_fixture() {
        let items = parse_search(FIXTURE).unwrap();
        assert_eq!(items.len(), 20);
        for item in &items {
            let Item::Video(v) = item else { unreachable!() };
            assert!(v.thumbnails.iter().all(|t| t.url.starts_with("https://")));
            assert!(v.description_snippet.is_some());
        }
        insta::assert_snapshot!(summary(&items));
    }

    #[test]
    fn skips_null_entries_and_bad_thumbnails() {
        let json = br#"{"entries": [
            null,
            {"id": "a", "thumbnails": null},
            {"id": "b", "thumbnails": [null, {"width": 1}, {"url": "https://i.ytimg.com/b.jpg"}]}
        ]}"#;
        let items = parse_search(json).unwrap();
        let thumbs: Vec<usize> =
            items.iter().map(|i| if let Item::Video(v) = i { v.thumbnails.len() } else { 99 }).collect();
        assert_eq!(thumbs, [0, 1]);
        assert!(parse_search(br#"{"entries": null}"#).unwrap().is_empty());
    }

    #[test]
    fn reads_live_status_availability_and_missing_fields() {
        let json = br#"{"entries": [
            {"id": "a", "title": "Live", "live_status": "is_live", "availability": "subscriber_only",
             "thumbnails": [{"url": "//i.ytimg.com/a.jpg"}, {"url": "/relative.jpg"}]},
            {"id": "b", "title": "Was live", "live_status": "was_live", "duration": 61.6,
             "uploader": "Up", "uploader_id": "UCxyz"},
            {"title": "no id"}
        ]}"#;
        let items = parse_search(json).unwrap();
        assert_eq!(items.len(), 2);
        let Item::Video(a) = &items[0] else { unreachable!() };
        assert_eq!(a.kind, VideoKind::Live);
        assert!(a.members_only);
        assert!(a.channels.is_empty());
        assert_eq!(a.duration_secs, None);
        assert_eq!(a.thumbnails, [Thumbnail { url: "https://i.ytimg.com/a.jpg".into(), width: 0, height: 0 }]);
        let Item::Video(b) = &items[1] else { unreachable!() };
        assert_eq!(b.kind, VideoKind::PastStream);
        assert!(!b.members_only);
        assert_eq!(b.duration_secs, Some(62));
        assert_eq!(b.duration_text.as_deref(), Some("1:02"));
        // A handle starts with `@`. Older uploader IDs are not handles.
        assert_eq!(b.channels, [ChannelRef { id: None, name: "Up".into(), handle: None }]);
    }

    #[test]
    fn bad_json_is_a_parse_error() {
        assert!(matches!(parse_search(b"not json"), Err(SourceError::Parse(_))));
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration_text(0), "0:00");
        assert_eq!(duration_text(245), "4:05");
        assert_eq!(duration_text(3600), "1:00:00");
        assert_eq!(duration_text(3723), "1:02:03");
        assert_eq!(duration_text(50350), "13:59:10");
    }

    #[test]
    fn formats_views() {
        assert_eq!(views_text(0), "0 views");
        assert_eq!(views_text(1), "1 view");
        assert_eq!(views_text(999), "999 views");
        assert_eq!(views_text(1000), "1,000 views");
        assert_eq!(views_text(1_234_567), "1,234,567 views");
    }

    #[test]
    fn rounds_seconds() {
        assert_eq!(round_secs(0.4), 0);
        assert_eq!(round_secs(59.5), 60);
        assert_eq!(round_secs(1e12), u32::MAX);
    }

    #[test]
    fn formats_compact_counts() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1000), "1K");
        assert_eq!(compact(1234), "1.23K");
        assert_eq!(compact(1299), "1.29K");
        assert_eq!(compact(12_345), "12.3K");
        assert_eq!(compact(568_381), "568K");
        assert_eq!(compact(999_999), "999K");
        assert_eq!(compact(1_000_000), "1M");
        assert_eq!(compact(1_200_000), "1.2M");
        assert_eq!(compact(8_730_000), "8.73M");
        assert_eq!(compact(16_900_000), "16.9M");
        assert_eq!(compact(2_500_000_000), "2.5B");
        assert_eq!(compact(u64::MAX), "18446744073B");
    }

    #[test]
    fn formats_subscribers_and_counts() {
        assert_eq!(subscribers_text(1), "1 subscriber");
        assert_eq!(subscribers_text(999), "999 subscribers");
        assert_eq!(subscribers_text(1_234), "1.23K subscribers");
        assert_eq!(subscribers_text(4_320_000), "4.32M subscribers");
        assert_eq!(counted(1, "video", "videos"), "1 video");
        assert_eq!(counted(1_234, "video", "videos"), "1,234 videos");
    }

    #[test]
    fn formats_dates() {
        assert_eq!(date_text("20171005").as_deref(), Some("Oct 5, 2017"));
        assert_eq!(date_text("20140617").as_deref(), Some("Jun 17, 2014"));
        assert_eq!(date_text("20000131").as_deref(), Some("Jan 31, 2000"));
        assert_eq!(date_text("20001231").as_deref(), Some("Dec 31, 2000"));
        assert_eq!(iso_date("20171005").as_deref(), Some("2017-10-05"));
        for bad in ["", "2017105", "201710050", "20171305", "20170000", "20171032", "2017-1-5", "２０１７１００５"]
        {
            assert_eq!(date_text(bad), None, "{bad}");
            assert_eq!(iso_date(bad), None, "{bad}");
        }
    }

    const CHANNEL: &[u8] = include_bytes!("../../tests/fixtures/ytdlp/channel.json");
    const PLAYLIST: &[u8] = include_bytes!("../../tests/fixtures/ytdlp/playlist.json");
    const VIDEO: &[u8] = include_bytes!("../../tests/fixtures/ytdlp/video.json");

    #[test]
    fn parses_the_channel_fixture() {
        let page = parse_channel(CHANNEL).unwrap();
        assert_eq!(page.videos.items.len(), 30);
        assert_eq!(page.videos.continuation, None);
        assert!(page.info.avatar.iter().all(|t| t.width == t.height));
        insta::assert_snapshot!(format!("{:#?}\n{}", page.info, summary(&page.videos.items)));
    }

    #[test]
    fn parses_the_playlist_fixture() {
        let page = parse_playlist(PLAYLIST).unwrap();
        assert_eq!(page.videos.items.len(), 10);
        assert_eq!(page.videos.continuation, None);
        insta::assert_snapshot!(format!("{:#?}\n{}", page.info, summary(&page.videos.items)));
    }

    #[test]
    fn parses_the_video_fixture() {
        let details = parse_video(VIDEO).unwrap();
        assert!(!details.live);
        assert!(details.description.links.is_empty());
        insta::assert_debug_snapshot!(details);
    }

    #[test]
    fn reads_a_live_video() {
        let json = br#"{"id": "v", "title": "Radio", "is_live": true, "live_status": "is_live",
            "duration": null, "view_count": 50, "concurrent_view_count": 10132, "upload_date": "20260819",
            "uploader_id": "UC-old", "uploader": "Up", "chapters": null, "thumbnails": null}"#;
        let v = parse_video(json).unwrap();
        assert!(v.live);
        assert_eq!(v.duration_secs, None);
        assert_eq!(v.views_text.as_deref(), Some("10,132 watching now"));
        assert_eq!(v.date_text.as_deref(), Some("Started streaming on Aug 19, 2026"));
        assert_eq!(v.channel, Some(ChannelRef { id: None, name: "Up".into(), handle: None }));
        let past = br#"{"id": "v", "live_status": "was_live", "upload_date": "20220712", "duration": 3600.4}"#;
        let v = parse_video(past).unwrap();
        assert!(!v.live);
        assert_eq!(v.duration_secs, Some(3600));
        assert_eq!(v.date_text.as_deref(), Some("Streamed live on Jul 12, 2022"));
        assert_eq!(v.channel, None);
    }

    #[test]
    fn sorts_chapters_and_skips_bad_ones() {
        let json = br#"{"id": "v", "chapters": [
            {"title": "B", "start_time": 67.0}, null, {"title": "no start"},
            {"title": "A", "start_time": 0}, {"title": "neg", "start_time": -1}
        ]}"#;
        let chapters = parse_video(json).unwrap().chapters;
        let want = [Chapter { title: "A".into(), start_secs: 0 }, Chapter { title: "B".into(), start_secs: 67 }];
        assert_eq!(chapters, want);
    }

    #[test]
    fn missing_ids_are_parse_errors() {
        assert!(matches!(parse_video(b"{}"), Err(SourceError::Parse(_))));
        assert!(matches!(parse_playlist(b"{}"), Err(SourceError::Parse(_))));
        assert!(matches!(parse_channel(b"{}"), Err(SourceError::Parse(_))));
        assert!(matches!(parse_video(b"[]"), Err(SourceError::Parse(_))));
    }

    #[test]
    fn channel_entries_get_the_channel() {
        let json = br#"{"id": "UCx", "channel": "C", "uploader_id": "@c", "entries": [
            {"id": "a"}, {"id": "b", "channel": "Other", "channel_id": "UCy"}
        ]}"#;
        let page = parse_channel(json).unwrap();
        let names: Vec<_> = page
            .videos
            .items
            .iter()
            .map(|i| if let Item::Video(v) = i { v.channels[0].name.clone() } else { unreachable!() })
            .collect();
        assert_eq!(names, ["C", "Other"]);
        assert_eq!(page.info.id, "UCx");
        assert_eq!(page.info.handle.as_deref(), Some("@c"));
        assert!(!page.info.verified);
    }

    fn query(filters: SearchFilters) -> SearchQuery {
        SearchQuery { text: "rust".into(), filters }
    }

    #[tokio::test]
    async fn missing_program_names_it() {
        let y = YtDlp { program: "/nonexistent/yt-dlp".into() };
        let err = y.search(&query(SearchFilters::default())).await.unwrap_err();
        assert!(matches!(err, SourceError::Program(_)));
        assert!(err.to_string().contains("/nonexistent/yt-dlp was not found"), "{err}");
    }

    #[tokio::test]
    async fn search_more_is_unsupported() {
        let y = YtDlp { program: "yt-dlp".into() };
        assert!(matches!(y.search_more("x").await, Err(SourceError::Unsupported(_))));
    }

    #[cfg(unix)]
    fn script(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.path().join("fake-yt-dlp");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failure_gives_the_last_stderr_line() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(&dir, "echo first >&2; echo 'ERROR: last' >&2; echo >&2; exit 1");
        let err = YtDlp { program }.search(&query(SearchFilters::default())).await.unwrap_err();
        assert!(matches!(err, SourceError::Program(_)));
        assert!(err.to_string().ends_with("failed: ERROR: last"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn search_passes_the_query_and_notes_filters() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ytdlp/search.json");
        let program = script(
            &dir,
            &format!(
                r#"[ "$1 $2 $3 $4" = "-J --flat-playlist --no-warnings ytsearch20:rust" ] || exit 1; cat '{fixture}'"#
            ),
        );
        let y = YtDlp { program };
        let page = y.search(&query(SearchFilters::default())).await.unwrap();
        assert_eq!(page.items.len(), 20);
        assert_eq!(page.note, None);
        assert_eq!(page.continuation, None);
        let filtered = SearchFilters { sort: Sort::ViewCount, ..SearchFilters::default() };
        let page = y.search(&query(filtered)).await.unwrap();
        assert!(page.note.unwrap().contains("ignores search filters"));
    }

    /// A fake yt-dlp that prints `fixture` only when it gets exactly `args`.
    #[cfg(unix)]
    fn expects(dir: &tempfile::TempDir, args: &str, fixture: &str) -> YtDlp {
        let fixture = format!("{}/tests/fixtures/ytdlp/{fixture}", env!("CARGO_MANIFEST_DIR"));
        YtDlp { program: script(dir, &format!(r#"[ "$*" = '{args}' ] || exit 1; cat '{fixture}'"#)) }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn channel_playlist_and_video_pass_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let y = expects(
            &dir,
            "-J --flat-playlist --no-warnings --playlist-items 1:30 \
             https://www.youtube.com/channel/UCsBjURrPoezykLs9EqgamOA/videos",
            "channel.json",
        );
        assert_eq!(y.channel("UCsBjURrPoezykLs9EqgamOA").await.unwrap().info.name, "Fireship");

        let dir = tempfile::tempdir().unwrap();
        let y = expects(
            &dir,
            "-J --flat-playlist --no-warnings --playlist-items 1:200 \
             https://www.youtube.com/playlist?list=PLBCF2DAC6FFB574DE",
            "playlist.json",
        );
        assert_eq!(y.playlist("PLBCF2DAC6FFB574DE").await.unwrap().info.id, "PLBCF2DAC6FFB574DE");
        assert_eq!(y.playlist("VLPLBCF2DAC6FFB574DE").await.unwrap().info.id, "PLBCF2DAC6FFB574DE");

        let dir = tempfile::tempdir().unwrap();
        let y = expects(
            &dir,
            "-J --no-playlist --skip-download --no-warnings https://www.youtube.com/watch?v=aircAruvnKk",
            "video.json",
        );
        assert_eq!(y.video("aircAruvnKk").await.unwrap().id, "aircAruvnKk");
    }

    #[tokio::test]
    async fn browse_more_is_unsupported() {
        let y = YtDlp { program: "yt-dlp".into() };
        assert!(matches!(y.browse_more("x").await, Err(SourceError::Unsupported(_))));
    }
}
