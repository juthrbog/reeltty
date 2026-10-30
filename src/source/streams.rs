//! The stream URLs of a video, from `yt-dlp -J -f <selector>`.
//!
//! yt-dlp picks the formats. For a normal video, the result has a video URL
//! and an audio URL in `requested_formats`. For a stream with one muxed
//! format (live streams), the URL is at the top level.

use serde::Deserialize;

use super::{Result, SourceError};

/// The format selector for DESIGN.md Q13: H.264 first, then VP9, never AV1.
/// Opus or stereo AAC audio, never IAMF. At most `max_height` pixels high.
/// The last choice, one muxed format, is for live streams.
pub fn selector(max_height: u32) -> String {
    let h = max_height;
    let avc = format!("bv*[vcodec^=avc1][height<={h}][protocol=https]");
    let vp9 = format!("bv*[vcodec~='^vp0?9'][height<={h}][protocol=https]");
    let opus = "ba[acodec=opus][protocol=https]";
    let aac = "ba[acodec^=mp4a][protocol=https]";
    let stereo_aac = "ba[acodec^=mp4a][audio_channels<=2][protocol=https]";
    [
        format!("{avc}+{opus}"),
        format!("{avc}+{stereo_aac}"),
        format!("{avc}+{aac}"),
        format!("{vp9}+{opus}"),
        format!("{vp9}+{aac}"),
        format!("b[vcodec^=avc1][acodec~='^(opus|mp4a)'][height<={h}]"),
    ]
    .join("/")
}

/// The heights that YouTube offers. A pane is given the next one up, so a
/// 700 px pane gets 720p and mpv scales it down.
pub fn rung(pixels: u32) -> u32 {
    const RUNGS: [u32; 5] = [144, 240, 360, 480, 720];
    RUNGS.into_iter().find(|r| *r >= pixels).unwrap_or(720)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Streams {
    pub video_url: String,
    /// `None` when the video URL has the audio too.
    pub audio_url: Option<String>,
    /// HTTP headers that the URLs need, such as `User-Agent`.
    pub headers: Vec<(String, String)>,
    /// Unix time when the URLs work. YouTube makes the guest wait as long as
    /// its ads would play.
    pub available_at: Option<u64>,
    /// Unix time when the URLs stop working.
    pub expires_at: Option<u64>,
    pub format: FormatInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatInfo {
    pub id: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub vcodec: String,
    pub acodec: String,
}

#[derive(Deserialize)]
struct Info {
    format_id: Option<String>,
    url: Option<String>,
    http_headers: Option<std::collections::BTreeMap<String, String>>,
    available_at: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    requested_formats: Option<Vec<Info>>,
}

impl Info {
    fn has_video(&self) -> bool {
        self.vcodec.as_deref().is_some_and(|c| c != "none")
    }
}

pub fn parse_streams(json: &[u8]) -> Result<Streams> {
    let info: Info = serde_json::from_slice(json).map_err(|e| SourceError::Parse(format!("yt-dlp output: {e}")))?;
    let parts = info.requested_formats.as_deref().unwrap_or_default();
    let (video, audio) = if parts.is_empty() {
        (&info, None)
    } else {
        let video = parts.iter().find(|f| f.has_video());
        let audio = parts.iter().find(|f| !f.has_video());
        (video.ok_or_else(|| SourceError::Parse("yt-dlp chose no video format".into()))?, audio)
    };
    let video_url = video.url.clone().ok_or_else(|| SourceError::Parse("yt-dlp gave no video URL".into()))?;
    let audio_url = audio.and_then(|a| a.url.clone());
    let all = [Some(&info), Some(video), audio];
    let available_at = all.iter().flatten().filter_map(|f| f.available_at).max();
    let expires_at = [Some(&video_url), audio_url.as_ref()].into_iter().flatten().filter_map(|u| expire(u)).min();
    let headers = video.http_headers.clone().or_else(|| info.http_headers.clone()).unwrap_or_default();
    let codec = |f: Option<&Info>, pick: fn(&Info) -> Option<&String>| {
        f.and_then(pick).filter(|c| *c != "none").cloned().unwrap_or_default()
    };
    let acodec = codec(audio, |f| f.acodec.as_ref());
    let format = FormatInfo {
        id: info.format_id.clone().or_else(|| video.format_id.clone()).unwrap_or_default(),
        width: video.width,
        height: video.height,
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "frame rates are small")]
        fps: video.fps.filter(|f| f.is_finite() && *f > 0.0).map(|f| f.round() as u32),
        vcodec: codec(Some(video), |f| f.vcodec.as_ref()),
        acodec: if acodec.is_empty() { codec(Some(video), |f| f.acodec.as_ref()) } else { acodec },
    };
    Ok(Streams { video_url, audio_url, headers: headers.into_iter().collect(), available_at, expires_at, format })
}

/// The `expire=` value in a googlevideo URL.
fn expire(url: &str) -> Option<u64> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| kv.strip_prefix("expire="))?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ytdlp/streams.json")).unwrap()
    }

    #[test]
    fn selector_prefers_h264_and_names_every_codec() {
        let s = selector(720);
        let choices: Vec<&str> = s.split('/').collect();
        assert_eq!(choices.len(), 6);
        assert_eq!(choices[0], "bv*[vcodec^=avc1][height<=720][protocol=https]+ba[acodec=opus][protocol=https]");
        assert!(choices[3].starts_with("bv*[vcodec~='^vp0?9']"));
        assert_eq!(choices[5], "b[vcodec^=avc1][acodec~='^(opus|mp4a)'][height<=720]");
        assert!(!s.contains("av01"));
    }

    #[test]
    fn rungs_round_up() {
        assert_eq!(rung(100), 144);
        assert_eq!(rung(360), 360);
        assert_eq!(rung(361), 480);
        assert_eq!(rung(700), 720);
        assert_eq!(rung(2000), 720);
    }

    #[test]
    fn parses_separate_video_and_audio() {
        let s = parse_streams(&fixture()).unwrap();
        assert!(s.video_url.contains("itag=298"));
        assert!(s.audio_url.as_deref().unwrap().contains("itag=251"));
        assert_eq!(s.headers.len(), 4);
        assert!(s.headers.iter().any(|(k, v)| k == "Accept" && v.contains(',')));
        // The latest of the formats.
        assert_eq!(s.available_at, Some(1_793_373_881));
        assert_eq!(s.expires_at, Some(1_793_395_466));
        let f = &s.format;
        assert_eq!((f.id.as_str(), f.width, f.height, f.fps), ("298+251", Some(1280), Some(720), Some(60)));
        assert_eq!((f.vcodec.as_str(), f.acodec.as_str()), ("avc1.4D4020", "opus"));
    }

    #[test]
    fn parses_one_muxed_format() {
        let json = br#"{"format_id": "95", "url": "https://x/live.m3u8", "vcodec": "avc1.4d401f",
            "acodec": "mp4a.40.2", "height": 720, "requested_formats": null,
            "http_headers": {"User-Agent": "ua"}}"#;
        let s = parse_streams(json).unwrap();
        assert_eq!(s.video_url, "https://x/live.m3u8");
        assert_eq!(s.audio_url, None);
        assert_eq!(s.available_at, None);
        assert_eq!(s.expires_at, None);
        assert_eq!(s.headers, [("User-Agent".to_string(), "ua".to_string())]);
        assert_eq!(s.format.acodec, "mp4a.40.2");
    }

    #[test]
    fn audio_only_is_an_error() {
        let json = br#"{"requested_formats": [{"url": "https://x/a", "vcodec": "none", "acodec": "opus"}]}"#;
        assert!(matches!(parse_streams(json), Err(SourceError::Parse(_))));
        assert!(matches!(parse_streams(b"{}"), Err(SourceError::Parse(_))));
        assert!(matches!(parse_streams(b"nope"), Err(SourceError::Parse(_))));
    }

    #[test]
    fn reads_expire() {
        assert_eq!(expire("https://x/v?a=1&expire=42&b=2"), Some(42));
        assert_eq!(expire("https://x/v?a=1"), None);
        assert_eq!(expire("https://x/v"), None);
    }
}
