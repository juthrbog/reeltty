//! The yt-dlp source: the fallback for metadata.
//!
//! Search runs `yt-dlp -J --flat-playlist "ytsearch20:<text>"` and reads the
//! `entries`. yt-dlp cannot apply search filters or give a next page.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Deserialize;
use tokio::process::Command;

use crate::source::model::{ChannelRef, Item, Thumbnail, Video, VideoKind};
use crate::source::{Result, SearchPage, SearchQuery, Source, SourceError};

/// How many results one search asks for.
const RESULTS: u32 = 20;
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

#[derive(Deserialize)]
struct Playlist {
    #[serde(default)]
    entries: Vec<Entry>,
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
    thumbnails: Vec<EntryThumbnail>,
}

#[derive(Deserialize)]
struct EntryThumbnail {
    url: String,
    width: Option<u32>,
    height: Option<u32>,
}

/// Reads the output of `yt-dlp -J --flat-playlist ytsearch...`. Entries without an ID are skipped.
pub fn parse_search(json: &[u8]) -> Result<Vec<Item>> {
    let playlist: Playlist =
        serde_json::from_slice(json).map_err(|e| SourceError::Parse(format!("yt-dlp output: {e}")))?;
    Ok(playlist.entries.into_iter().filter_map(video).map(Item::Video).collect())
}

fn video(e: Entry) -> Option<Video> {
    let id = e.id?;
    let kind = match e.live_status.as_deref() {
        Some("is_live") => VideoKind::Live,
        Some("was_live") => VideoKind::PastStream,
        _ => VideoKind::Normal,
    };
    let duration_secs = e.duration.filter(|d| d.is_finite() && *d >= 0.0).map(round_secs);
    let name = e.channel.or(e.uploader);
    let handle = e.uploader_id.filter(|h| h.starts_with('@'));
    let channels = match (name, e.channel_id) {
        (None, None) => Vec::new(),
        (name, id) => vec![ChannelRef { id, name: name.unwrap_or_default(), handle }],
    };
    Some(Video {
        id,
        title: e.title.unwrap_or_default(),
        kind,
        channels,
        duration_text: duration_secs.map(duration_text),
        duration_secs,
        views_text: e.view_count.map(views_text),
        published_text: None,
        thumbnails: e.thumbnails.into_iter().filter_map(thumbnail).collect(),
        members_only: e.availability.as_deref() == Some("subscriber_only"),
        badges: Vec::new(),
        description_snippet: e.description.filter(|d| !d.is_empty()),
    })
}

fn thumbnail(t: EntryThumbnail) -> Option<Thumbnail> {
    let url = if t.url.starts_with("https://") {
        t.url
    } else if let Some(rest) = t.url.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        return None;
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

/// "1 view", "1,234 views".
fn views_text(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n == 1 { format!("{out} view") } else { format!("{out} views") }
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
}
