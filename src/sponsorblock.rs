//! SponsorBlock: parts of videos that the community marked, such as
//! sponsor messages, so that playback can skip them.
//!
//! The request names only the first 4 hex characters of the SHA-256 hash of
//! the video ID. The server answers with the segments of all videos whose
//! hash starts with them, and reeltty picks the video it plays.

use std::time::Duration;

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::source::innertube::client::{USER_AGENT, error_chain};

const API_URL: &str = "https://sponsor.ajay.app";
const TIMEOUT: Duration = Duration::from_secs(5);
/// A bucket is about 50 KB.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// A segment that starts this close to the video start starts at 0, and
/// one that ends this close to the video end ends there.
const EDGE_MS: u64 = 1000;
/// Segments with a smaller gap between them are skipped in one jump.
const MERGE_GAP_MS: u64 = 200;
/// A position this close to a segment end is past it. After a skip, mpv
/// can land a little before the end.
const END_SLACK_MS: u64 = 50;
/// A reload starts at a whole second, and the app knows the position only
/// once a second, so it can start this much before where the user was.
const LANDING_MS: u64 = 2500;

/// The kinds of segments that can be skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Sponsor,
    Selfpromo,
    Interaction,
    Intro,
    Outro,
    Preview,
    Hook,
    Filler,
    MusicOfftopic,
}

impl Category {
    const ALL: [Self; 9] = [
        Self::Sponsor,
        Self::Selfpromo,
        Self::Interaction,
        Self::Intro,
        Self::Outro,
        Self::Preview,
        Self::Hook,
        Self::Filler,
        Self::MusicOfftopic,
    ];

    /// The name in the API and in the configuration file.
    pub fn api_name(self) -> &'static str {
        match self {
            Self::Sponsor => "sponsor",
            Self::Selfpromo => "selfpromo",
            Self::Interaction => "interaction",
            Self::Intro => "intro",
            Self::Outro => "outro",
            Self::Preview => "preview",
            Self::Hook => "hook",
            Self::Filler => "filler",
            Self::MusicOfftopic => "music_offtopic",
        }
    }

    /// For "Skipped sponsor."
    pub fn label(self) -> &'static str {
        match self {
            Self::Sponsor => "sponsor",
            Self::Selfpromo => "self-promotion",
            Self::Interaction => "interaction reminder",
            Self::Intro => "intro",
            Self::Outro => "outro",
            Self::Preview => "preview",
            Self::Hook => "hook",
            Self::Filler => "filler",
            Self::MusicOfftopic => "non-music part",
        }
    }

    fn from_api(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.api_name() == name)
    }
}

/// A part of a video to skip. Times are milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub category: Category,
    pub start_ms: u64,
    pub end_ms: u64,
    /// The length of the video when the segment was submitted, if known.
    pub video_duration_ms: Option<u64>,
}

/// The first 4 hex characters of the SHA-256 hash of `id`.
pub fn hash_prefix(id: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, id.as_bytes());
    let d = digest.as_ref();
    format!("{:02x}{:02x}", d[0], d[1])
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiVideo {
    #[serde(rename = "videoID")]
    video_id: String,
    segments: Vec<ApiSegment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiSegment {
    category: String,
    action_type: String,
    segment: (f64, f64),
    #[serde(default)]
    video_duration: f64,
}

/// The segments of video `id` in a response, in the order they come. Only
/// segments to skip, of known categories, are kept.
pub fn parse(body: &str, id: &str) -> Result<Vec<Segment>> {
    let videos: Vec<ApiVideo> = serde_json::from_str(body)?;
    let Some(video) = videos.into_iter().find(|v| v.video_id == id) else { return Ok(Vec::new()) };
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "rounded and clamped")]
    let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
    Ok(video
        .segments
        .into_iter()
        .filter(|s| s.action_type == "skip")
        .filter_map(|s| {
            let category = Category::from_api(&s.category)?;
            let (start_ms, end_ms) = (ms(s.segment.0), ms(s.segment.1));
            // [0, 0] marks the whole video, as in "this video is sponsored".
            (end_ms > start_ms).then_some(Segment {
                category,
                start_ms,
                end_ms,
                video_duration_ms: Some(ms(s.video_duration)).filter(|d| *d > 0),
            })
        })
        .collect())
}

/// The segments to skip in a video of `duration_ms`, sorted, with near ones
/// merged. Segments from a video of another length are dropped: they are
/// probably from an older upload. The rules are the ones yt-dlp uses.
pub fn usable(segments: &[Segment], duration_ms: Option<u64>) -> Vec<Segment> {
    let mut kept: Vec<Segment> = segments
        .iter()
        .filter_map(|s| {
            let mut s = s.clone();
            if s.start_ms <= EDGE_MS {
                s.start_ms = 0;
            }
            let Some(duration) = duration_ms else { return Some(s) };
            if duration.saturating_sub(s.end_ms) <= EDGE_MS {
                s.end_ms = duration;
            }
            let diff = s.video_duration_ms.map_or(0, |d| d.abs_diff(duration));
            let length = s.end_ms.saturating_sub(s.start_ms).max(1);
            // Under 1 s, or under 5 s and under 5 % of the segment.
            let same = diff < 1000 || (diff < 5000 && diff * 20 < length);
            (same && s.end_ms > s.start_ms).then_some(s)
        })
        .collect();
    kept.sort_by_key(|s| (s.start_ms, s.end_ms));
    let mut merged: Vec<Segment> = Vec::with_capacity(kept.len());
    for s in kept {
        match merged.last_mut() {
            Some(last) if s.start_ms <= last.end_ms + MERGE_GAP_MS => last.end_ms = last.end_ms.max(s.end_ms),
            _ => merged.push(s),
        }
    }
    merged
}

/// Fetches segments from the SponsorBlock server.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    categories: Vec<Category>,
}

impl Client {
    pub fn new(categories: Vec<Category>) -> Result<Self> {
        Self::with_base_url(API_URL.into(), categories)
    }

    pub fn with_base_url(base_url: String, categories: Vec<Category>) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| anyhow::anyhow!("cannot build the HTTP client: {}", error_chain(&e)))?;
        Ok(Self { http, base_url, categories })
    }

    /// The segments to skip in video `id`, in the configured categories.
    pub async fn segments(&self, id: &str) -> Result<Vec<Segment>> {
        let mut url = reqwest::Url::parse(&format!("{}/api/skipSegments/{}", self.base_url, hash_prefix(id)))?;
        let names: Vec<&str> = self.categories.iter().map(|c| c.api_name()).collect();
        url.query_pairs_mut()
            .append_pair("categories", &serde_json::to_string(&names)?)
            .append_pair("actionTypes", r#"["skip"]"#);
        let network = |e: reqwest::Error| anyhow::anyhow!("{}", error_chain(&e));
        let mut response = self.http.get(url).send().await.map_err(network)?;
        // No video in the bucket has segments.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        response = response.error_for_status().map_err(network)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_RESPONSE_BYTES {
                bail!("SponsorBlock response bigger than {MAX_RESPONSE_BYTES} bytes");
            }
        }
        parse(&String::from_utf8_lossy(&bytes), id)
    }
}

/// Decides when to skip, from the positions that mpv reports.
///
/// Playing into a segment skips it, also after a seek back to before it. A
/// seek that lands in a segment does not: the user chose to watch it.
///
/// A reload at the same place (a quality change, or a new player after an
/// HTTP 403 error) also counts as a seek. It starts at a whole second, up to
/// about 2 s before where the user was, so a segment that starts just after
/// the landing does not skip either.
///
/// When the first segments come, a position in a segment skips it, so an
/// intro at 0 and a resume inside a segment skip.
#[derive(Debug, Default)]
#[expect(clippy::struct_excessive_bools, reason = "independent facts about mpv's state")]
pub struct Skipper {
    segments: Vec<Segment>,
    /// The last position, in milliseconds.
    prev: Option<u64>,
    /// The next position is where a seek or a new file landed.
    jumped: bool,
    /// A seek runs. Positions until it ends can be from either side of it.
    seeking: bool,
    /// The next position after a seek ends is where a reload landed.
    landing: bool,
    /// mpv loaded a file before, so the next one is a reload.
    loaded: bool,
    /// The start of a segment that a reload landed in or just before. It
    /// plays until the next seek.
    watching: Option<u64>,
    /// Segments came once. Later ones keep the position: only their segments change.
    had_segments: bool,
    /// This player goes on from another one, so the first segments do not
    /// skip where playback is.
    continues: bool,
}

impl Skipper {
    /// Sets the segments. The same segments again change nothing: the app
    /// sends them again for each file, such as after a quality change.
    pub fn set(&mut self, segments: Vec<Segment>, continues: bool) {
        if segments == self.segments {
            return;
        }
        self.segments = segments;
        if std::mem::replace(&mut self.had_segments, true) {
            return;
        }
        self.continues = continues;
        match (continues, self.prev) {
            (true, Some(ms)) => self.watching = self.landed_in(ms),
            (true, None) => self.landing = true,
            (false, _) => self.prev = None,
        }
    }

    /// mpv starts a seek.
    pub fn on_seek(&mut self) {
        self.jumped = true;
        self.seeking = true;
        self.watching = None;
    }

    /// mpv plays again after a seek or a new file.
    pub fn on_restart(&mut self) {
        self.seeking = false;
    }

    /// mpv loaded a new file. It starts where it was told to, like a seek.
    pub fn on_file_loaded(&mut self) {
        self.on_seek();
        self.landing = std::mem::replace(&mut self.loaded, true) || self.continues;
    }

    /// The segment to skip at position `ms`, if any.
    pub fn on_position(&mut self, ms: u64) -> Option<&Segment> {
        let prev = self.prev.replace(ms);
        let jumped = self.jumped;
        if !self.seeking {
            self.jumped = false;
            if std::mem::take(&mut self.landing) {
                self.watching = self.landed_in(ms);
            }
        }
        let segment = self.segments.iter().find(|s| s.start_ms <= ms && ms + END_SLACK_MS < s.end_ms)?;
        if self.watching == Some(segment.start_ms) {
            return None;
        }
        match prev {
            None if !self.continues => Some(segment),
            Some(prev) if !jumped && prev < segment.start_ms => Some(segment),
            _ => None,
        }
    }

    /// The start of the segment that a reload landing at `ms` belongs to.
    fn landed_in(&self, ms: u64) -> Option<u64> {
        self.segments.iter().find(|s| s.start_ms <= ms + LANDING_MS && ms < s.end_ms).map(|s| s.start_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{reply, serve};

    fn seg(category: Category, start_ms: u64, end_ms: u64) -> Segment {
        Segment { category, start_ms, end_ms, video_duration_ms: None }
    }

    fn bucket() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sponsorblock/bucket.json"))
            .unwrap()
    }

    #[test]
    fn hashes_the_video_id() {
        assert_eq!(hash_prefix("dQw4w9WgXcQ"), "5f6b");
    }

    #[test]
    fn parses_the_segments_of_one_video() {
        let segments = parse(&bucket(), "cxFyU2NlMdg").unwrap();
        let d = Some(1_273_621);
        assert_eq!(
            segments,
            [
                Segment { category: Category::Intro, start_ms: 0, end_ms: 10_500, video_duration_ms: d },
                Segment { category: Category::Sponsor, start_ms: 37_958, end_ms: 107_877, video_duration_ms: d },
                Segment { category: Category::Sponsor, start_ms: 1_074_959, end_ms: 1_273_621, video_duration_ms: d },
            ]
        );
        assert!(parse(&bucket(), "dQw4w9WgXcQ").unwrap().is_empty(), "not in the bucket");
        assert!(parse("not json", "x").is_err());
    }

    #[test]
    fn keeps_only_known_categories_to_skip() {
        let body = r#"[{"videoID":"v","segments":[
            {"category":"sponsor","actionType":"skip","segment":[5,9],"videoDuration":0},
            {"category":"filler","actionType":"mute","segment":[10,20],"videoDuration":0},
            {"category":"selfpromo","actionType":"full","segment":[0,0],"videoDuration":0},
            {"category":"sponsor","actionType":"skip","segment":[0,0],"videoDuration":0},
            {"category":"poi_highlight","actionType":"poi","segment":[30,30],"videoDuration":0},
            {"category":"new_kind","actionType":"skip","segment":[40,50],"videoDuration":0}]}]"#;
        assert_eq!(parse(body, "v").unwrap(), [seg(Category::Sponsor, 5000, 9000)]);
    }

    #[test]
    fn usable_segments_are_clamped_merged_and_of_this_video() {
        let segments = parse(&bucket(), "601HYb7gFz0").unwrap();
        let usable = usable(&segments, Some(946_161));
        let spans: Vec<(u64, u64)> = usable.iter().map(|s| (s.start_ms, s.end_ms)).collect();
        // The sponsor and the self-promotion from 285 s overlap. The last one
        // ends 161 ms before the video end.
        assert_eq!(spans, [(75_152, 80_000), (285_331, 347_819), (743_769, 835_318), (931_433, 946_161)]);
        assert_eq!(usable[1].category, Category::Sponsor);

        let near_start = [seg(Category::Intro, 800, 5000)];
        assert_eq!(super::usable(&near_start, None)[0].start_ms, 0);
        // From an upload 10 s longer.
        let old = [Segment { video_duration_ms: Some(110_000), ..seg(Category::Sponsor, 10_000, 20_000) }];
        assert!(super::usable(&old, Some(100_000)).is_empty());
        // 3 s off is fine for a long segment, not for a short one.
        let long = [Segment { video_duration_ms: Some(1_003_000), ..seg(Category::Sponsor, 0, 100_000) }];
        assert_eq!(super::usable(&long, Some(1_000_000)).len(), 1);
        let short = [Segment { video_duration_ms: Some(1_003_000), ..seg(Category::Sponsor, 0, 10_000) }];
        assert!(super::usable(&short, Some(1_000_000)).is_empty());
        // Without a length, all stay.
        assert_eq!(super::usable(&old, None).len(), 1);
    }

    fn skipper(segments: &[Segment]) -> Skipper {
        let mut s = Skipper::default();
        s.set(segments.to_vec(), false);
        s
    }

    fn skips(s: &mut Skipper, ms: u64) -> Option<u64> {
        s.on_position(ms).map(|seg| seg.end_ms)
    }

    #[test]
    fn skips_when_playback_reaches_a_segment() {
        let mut s = skipper(&[seg(Category::Intro, 0, 5000), seg(Category::Sponsor, 10_000, 20_000)]);
        assert_eq!(skips(&mut s, 0), Some(5000), "an intro at the start");
        // Our own seek lands at the end, or a little before it.
        s.on_seek();
        s.on_restart();
        assert_eq!(skips(&mut s, 4980), None);
        assert_eq!(skips(&mut s, 9990), None);
        assert_eq!(skips(&mut s, 10_020), Some(20_000));
    }

    #[test]
    fn a_seek_into_a_segment_does_not_skip() {
        let mut s = skipper(&[seg(Category::Sponsor, 10_000, 20_000)]);
        assert_eq!(skips(&mut s, 1000), None);
        s.on_seek();
        // A position from before the seek ended.
        assert_eq!(skips(&mut s, 1030), None);
        s.on_restart();
        assert_eq!(skips(&mut s, 12_000), None);
        assert_eq!(skips(&mut s, 12_030), None, "the user watches it");
        // Back to before it, then playing into it skips again.
        s.on_seek();
        s.on_restart();
        assert_eq!(skips(&mut s, 9000), None);
        assert_eq!(skips(&mut s, 10_010), Some(20_000));
    }

    #[test]
    fn a_new_file_continues_where_the_user_was() {
        let mut s = skipper(&[seg(Category::Sponsor, 10_000, 20_000)]);
        assert_eq!(skips(&mut s, 1000), None);
        s.on_seek();
        s.on_restart();
        assert_eq!(skips(&mut s, 12_000), None);
        // A quality change loads the file again at the same place, and the
        // app sends the same segments again.
        s.on_file_loaded();
        s.set(vec![seg(Category::Sponsor, 10_000, 20_000)], false);
        assert_eq!(skips(&mut s, 0), None, "mpv can report 0 first");
        s.on_restart();
        assert_eq!(skips(&mut s, 12_100), None);
    }

    #[test]
    fn new_segments_skip_where_playback_is() {
        // The segments come after playback started, or it resumed inside one.
        let mut s = Skipper::default();
        assert_eq!(skips(&mut s, 12_000), None);
        s.set(vec![seg(Category::Sponsor, 10_000, 20_000)], false);
        assert_eq!(skips(&mut s, 12_030), Some(20_000));
    }

    #[test]
    fn a_reload_just_before_a_watched_segment_does_not_skip_it() {
        let mut s = skipper(&[seg(Category::Sponsor, 10_300, 20_000)]);
        s.on_file_loaded();
        s.on_restart();
        assert_eq!(skips(&mut s, 1000), None);
        s.on_seek();
        s.on_restart();
        assert_eq!(skips(&mut s, 10_400), None);
        // A quality change at 10.6 s starts the new file at 10 s.
        s.on_file_loaded();
        s.on_restart();
        assert_eq!(skips(&mut s, 10_000), None);
        assert_eq!(skips(&mut s, 10_310), None);
        // A seek back to before it, then playing into it, skips again.
        s.on_seek();
        s.on_restart();
        assert_eq!(skips(&mut s, 9000), None);
        assert_eq!(skips(&mut s, 10_320), Some(20_000));
    }

    #[test]
    fn the_first_file_is_not_a_reload() {
        let mut s = skipper(&[seg(Category::Sponsor, 1000, 4000)]);
        s.on_file_loaded();
        assert_eq!(skips(&mut s, 0), None);
        s.on_restart();
        assert_eq!(skips(&mut s, 33), None);
        assert_eq!(skips(&mut s, 1010), Some(4000));
    }

    #[test]
    fn changed_segments_keep_the_position() {
        let mut s = skipper(&[seg(Category::Sponsor, 10_000, 20_000), seg(Category::Outro, 90_000, 100_000)]);
        assert_eq!(skips(&mut s, 1000), None);
        s.on_seek();
        s.on_restart();
        assert_eq!(skips(&mut s, 12_000), None);
        // mpv's exact length moves the end of the outro.
        s.set(vec![seg(Category::Sponsor, 10_000, 20_000), seg(Category::Outro, 90_000, 100_200)], false);
        assert_eq!(skips(&mut s, 12_030), None, "the user still watches it");
        assert_eq!(skips(&mut s, 90_010), Some(100_200));
    }

    #[test]
    fn a_new_player_that_goes_on_does_not_skip_where_it_starts() {
        // After an HTTP 403 error, at 12 s, in a segment the user watched.
        let mut s = Skipper::default();
        s.on_file_loaded();
        s.on_restart();
        assert_eq!(skips(&mut s, 12_000), None);
        s.set(vec![seg(Category::Sponsor, 10_000, 20_000), seg(Category::Sponsor, 30_000, 40_000)], true);
        assert_eq!(skips(&mut s, 12_030), None);
        assert_eq!(skips(&mut s, 30_010), Some(40_000));
        // The segments come before the first position.
        let mut s = Skipper::default();
        s.set(vec![seg(Category::Sponsor, 10_000, 20_000)], true);
        s.on_file_loaded();
        s.on_restart();
        assert_eq!(skips(&mut s, 12_000), None);
        assert_eq!(skips(&mut s, 12_030), None);
    }

    #[tokio::test]
    async fn fetches_by_hash_prefix() {
        let body = bucket();
        let (url, log) = serve(move |_| reply(200, body.clone())).await;
        let client = Client::with_base_url(url, vec![Category::Sponsor, Category::Intro]).unwrap();
        assert_eq!(client.segments("cxFyU2NlMdg").await.unwrap().len(), 3);
        let target = log.lock().unwrap()[0].target.clone();
        let prefix = hash_prefix("cxFyU2NlMdg");
        assert_eq!(
            target,
            format!(
                "/api/skipSegments/{prefix}?categories=%5B%22sponsor%22%2C%22intro%22%5D&actionTypes=%5B%22skip%22%5D"
            )
        );
    }

    #[tokio::test]
    async fn not_found_is_no_segments_and_errors_are_errors() {
        let (url, _) = serve(|_| reply(404, "[]")).await;
        let client = Client::with_base_url(url, vec![Category::Sponsor]).unwrap();
        assert!(client.segments("x").await.unwrap().is_empty());
        let (url, _) = serve(|_| reply(500, "down")).await;
        let client = Client::with_base_url(url, vec![Category::Sponsor]).unwrap();
        let err = client.segments("x").await.unwrap_err();
        assert!(err.to_string().contains("500"), "{err}");
    }
}
