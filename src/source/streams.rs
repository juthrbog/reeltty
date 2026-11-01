//! The stream URLs of a video, from `yt-dlp -J -f <selector>`.
//!
//! yt-dlp picks the formats. For a normal video, the result has a video URL
//! and an audio URL in `requested_formats`. For a stream with one muxed
//! format (live streams), the URL is at the top level.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::model::{ChannelRef, Chapter};
use super::{Result, SourceError};

/// The sizes that YouTube offers, in pixels on the short side.
pub const RUNGS: [u32; 8] = [144, 240, 360, 480, 720, 1080, 1440, 2160];

/// The format selector for DESIGN.md Q13: H.264 or VP9, never AV1, and never
/// HDR. Opus or stereo AAC audio, never IAMF. The last two choices are for
/// live streams, which YouTube gives as separate HLS video and audio, or as
/// one muxed HLS stream. The size, and H.264 before VP9 at the same
/// size, come from `sort`, not from the selector. YouTube has H.264 only up
/// to 1080p, so larger sizes are VP9.
pub fn selector(max: u32) -> String {
    let video = "bv[vcodec~='^(avc1|vp0?9)'][dynamic_range=SDR][protocol=https]";
    // `sort` limits the short side. A very wide video can have a stream that
    // is short enough but much wider than `max` allows, so first try the
    // streams that fit in a 16:9 picture of `max` either way up.
    let long = max_long(max);
    let fitting = format!("{video}[width<=?{long}][height<=?{long}]");
    let opus = "ba[acodec=opus][protocol=https]";
    let aac = "ba[acodec^=mp4a][protocol=https]";
    let stereo_aac = "ba[acodec^=mp4a][audio_channels<=2][protocol=https]";
    [
        format!("{fitting}+{opus}"),
        format!("{fitting}+{stereo_aac}"),
        format!("{fitting}+{aac}"),
        format!("{video}+{opus}"),
        format!("{video}+{stereo_aac}"),
        format!("{video}+{aac}"),
        "bv[vcodec^=avc1][protocol^=m3u8]+ba[protocol^=m3u8]".to_string(),
        "b[vcodec^=avc1][acodec~='^(opus|mp4a)']".to_string(),
    ]
    .join("/")
}

/// The long side of a 16:9 picture that is `max` pixels on the short side.
pub fn max_long(max: u32) -> u32 {
    max.saturating_mul(16) / 9
}

/// The format sort: the largest size up to `max` pixels on the short side,
/// or the smallest above it, and H.264 before VP9 at the same size. The
/// short side, so that vertical videos are not given a smaller size than
/// wide ones.
pub fn sort(max: u32) -> String {
    format!("res:{max},vcodec:avc1")
}

/// The size to ask for: the next one up from `pixels`, so a 700 px picture
/// gets 720p and mpv scales it down, but never more than `max`.
pub fn rung(pixels: u32, max: u32) -> u32 {
    RUNGS.into_iter().find(|r| *r >= pixels).unwrap_or(RUNGS[RUNGS.len() - 1]).min(max)
}

/// `pixels` rounded down to a size that YouTube offers, but at least the smallest.
pub fn rung_below(pixels: u32) -> u32 {
    RUNGS.into_iter().rev().find(|r| *r <= pixels).unwrap_or(RUNGS[0])
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
    pub chapters: Vec<Chapter>,
    pub channel: Option<ChannelRef>,
    /// `None` for a live stream.
    pub duration_secs: Option<u32>,
    /// Live now, so it has no fixed length and no place to resume.
    pub live: bool,
    /// The other sizes of the video stream, smallest first. Empty for live
    /// streams, which come over HLS.
    pub qualities: Vec<Quality>,
    /// Captions in the `WebVTT` format, in the order YouTube gives them.
    pub captions: Vec<Caption>,
}

/// One size of the video stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quality {
    /// Pixels on the short side, as in "720p".
    pub res: u32,
    /// Pixels on the long side.
    pub long: u32,
    pub url: String,
    /// Unix time when the URL works. Formats from different YouTube clients
    /// have different ads waits.
    pub available_at: Option<u64>,
}

/// One caption track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caption {
    /// A language code such as `en`, `en-US`, or `en-orig`.
    pub lang: String,
    /// The language name, such as "English".
    pub name: String,
    pub url: String,
    /// Made by speech recognition, not by a person.
    pub auto: bool,
}

/// The best caption track for `lang`, in this order:
/// 1. Made by a person, in the same language and region.
/// 2. Made by a person, in the same language (`pt` for `pt-BR`, or `en-US` for `en`).
/// 3. Made by speech recognition, in that language.
/// 4. Translated by machine into that language.
pub fn pick_caption<'a>(captions: &'a [Caption], lang: &str) -> Option<&'a Caption> {
    let base = |l: &str| l.split('-').next().unwrap_or_default().to_ascii_lowercase();
    let same = |c: &&Caption| c.lang.eq_ignore_ascii_case(lang);
    let related = |c: &&Caption| base(&c.lang) == base(lang);
    // YouTube translates the speech recognition track into every language.
    let translated = |c: &&Caption| c.url.contains("tlang=");
    let manual = || captions.iter().filter(|c| !c.auto);
    let recognized = || captions.iter().filter(|c| c.auto && !translated(c));
    let machine = || captions.iter().filter(|c| c.auto && translated(c));
    manual()
        .find(same)
        .or_else(|| manual().find(related))
        .or_else(|| recognized().find(related))
        .or_else(|| machine().find(same))
        .or_else(|| machine().find(related))
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
    /// yt-dlp can write it as a float.
    available_at: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    protocol: Option<String>,
    /// `SDR`, or an HDR kind such as `HDR10`.
    dynamic_range: Option<String>,
    channel: Option<String>,
    channel_id: Option<String>,
    /// A handle such as `@name`.
    uploader_id: Option<String>,
    duration: Option<f64>,
    is_live: Option<bool>,
    live_status: Option<String>,
    requested_formats: Option<Vec<Info>>,
    formats: Option<Vec<Info>>,
    chapters: Option<Vec<YtChapter>>,
    subtitles: Option<BTreeMap<String, Vec<Track>>>,
    automatic_captions: Option<BTreeMap<String, Vec<Track>>>,
}

#[derive(Deserialize)]
struct YtChapter {
    title: Option<String>,
    start_time: Option<f64>,
}

#[derive(Deserialize)]
struct Track {
    ext: Option<String>,
    url: Option<String>,
    name: Option<String>,
}

impl Info {
    fn has_video(&self) -> bool {
        self.vcodec.as_deref().is_some_and(|c| c != "none")
    }

    fn has_audio(&self) -> bool {
        self.acodec.as_deref().is_some_and(|c| c != "none")
    }
}

/// One video-only SDR stream for each size, H.264 before VP9, as the
/// selector chooses. The player leaves out the sizes above its maximum.
fn qualities(formats: &[Info]) -> Vec<Quality> {
    let mut by_res: BTreeMap<u32, (bool, Quality)> = BTreeMap::new();
    for f in formats {
        let (Some(w), Some(h), Some(url)) = (f.width, f.height, &f.url) else { continue };
        let codec = f.vcodec.as_deref().unwrap_or_default();
        let (avc, vp9) = (codec.starts_with("avc1"), codec == "vp9" || codec.starts_with("vp09"));
        // yt-dlp marks every video format without a range as SDR.
        let sdr = f.dynamic_range.as_deref().is_none_or(|r| r == "SDR");
        let res = w.min(h);
        if !(avc || vp9) || !sdr || f.has_audio() || f.protocol.as_deref() != Some("https") {
            continue;
        }
        // Keep the last H.264 stream of a size: yt-dlp lists the better ones last.
        match by_res.get(&res) {
            Some((true, _)) if !avc => {}
            _ => {
                let available_at = unix_time(f.available_at);
                by_res.insert(res, (avc, Quality { res, long: w.max(h), url: url.clone(), available_at }));
            }
        }
    }
    by_res.into_values().map(|(_, q)| q).collect()
}

fn captions(manual: Option<BTreeMap<String, Vec<Track>>>, auto: Option<BTreeMap<String, Vec<Track>>>) -> Vec<Caption> {
    let tracks = |map: Option<BTreeMap<String, Vec<Track>>>, auto: bool| {
        map.unwrap_or_default().into_iter().filter_map(move |(lang, tracks)| {
            let vtt = tracks.into_iter().find(|t| t.ext.as_deref() == Some("vtt"))?;
            Some(Caption { name: vtt.name.unwrap_or_else(|| lang.clone()), lang, url: vtt.url?, auto })
        })
    };
    tracks(manual, false).chain(tracks(auto, true)).collect()
}

/// A Unix time from yt-dlp, which can write it as a float, rounded up.
fn unix_time(t: Option<f64>) -> Option<u64> {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "Unix times")]
    t.filter(|t| t.is_finite() && *t >= 0.0).map(|t| t.ceil() as u64)
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
    let available_at = all.iter().flatten().filter_map(|f| unix_time(f.available_at)).max();
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
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "video lengths")]
    let chapters = info
        .chapters
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| {
            let start = c.start_time.filter(|t| t.is_finite() && *t >= 0.0)?;
            Some(Chapter { title: c.title.unwrap_or_default(), start_secs: start as u32 })
        })
        .collect();
    let channel = info.channel.clone().map(|name| ChannelRef {
        id: info.channel_id.clone(),
        name,
        handle: info.uploader_id.clone().filter(|h| h.starts_with('@')),
    });
    let live = info.is_live == Some(true) || info.live_status.as_deref() == Some("is_live");
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "video lengths")]
    let duration_secs = info.duration.filter(|d| d.is_finite() && *d > 0.0 && !live).map(|d| d.round() as u32);
    let qualities =
        if audio_url.is_some() { qualities(info.formats.as_deref().unwrap_or_default()) } else { Vec::new() };
    Ok(Streams {
        video_url,
        audio_url,
        headers: headers.into_iter().collect(),
        available_at,
        expires_at,
        format,
        chapters,
        channel,
        duration_secs,
        live,
        qualities,
        captions: captions(info.subtitles, info.automatic_captions),
    })
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
        let s = selector(1080);
        let choices: Vec<&str> = s.split('/').collect();
        assert_eq!(choices.len(), 8);
        assert_eq!(
            choices[0],
            "bv[vcodec~='^(avc1|vp0?9)'][dynamic_range=SDR][protocol=https][width<=?1920][height<=?1920]\
             +ba[acodec=opus][protocol=https]"
        );
        assert!(choices[..3].iter().all(|c| c.contains("[width<=?1920][height<=?1920]+")));
        assert!(choices[3..6].iter().all(|c| !c.contains("width")));
        // Only the last choice is a muxed format, so a small muxed one never
        // wins over separate video and audio.
        assert!(choices[..6].iter().all(|c| c.starts_with("bv[") && c.contains("[dynamic_range=SDR]")));
        assert_eq!(choices[6], "bv[vcodec^=avc1][protocol^=m3u8]+ba[protocol^=m3u8]");
        assert_eq!(choices[7], "b[vcodec^=avc1][acodec~='^(opus|mp4a)']");
        assert!(selector(2160).contains("[width<=?3840]"));
        assert!(!s.contains("av01"));
        assert_eq!(sort(480), "res:480,vcodec:avc1");
    }

    #[test]
    fn rungs_round_up() {
        assert_eq!(rung(100, 1080), 144);
        assert_eq!(rung(360, 1080), 360);
        assert_eq!(rung(361, 1080), 480);
        assert_eq!(rung(700, 1080), 720);
        assert_eq!(rung(1000, 1080), 1080);
        assert_eq!(rung(1300, 1080), 1080);
        assert_eq!(rung(1300, 2160), 1440);
        assert_eq!(rung(5000, 2160), 2160);
        assert_eq!(rung(700, 480), 480);
    }

    #[test]
    fn rungs_round_down() {
        assert_eq!(rung_below(0), 144);
        assert_eq!(rung_below(720), 720);
        assert_eq!(rung_below(1000), 720);
        assert_eq!(rung_below(9999), 2160);
    }

    #[test]
    fn parses_separate_video_and_audio() {
        let s = parse_streams(&fixture()).unwrap();
        assert!(s.video_url.contains("itag=298"));
        assert!(s.audio_url.as_deref().unwrap().contains("itag=251"));
        assert_eq!(s.headers.len(), 4);
        assert!(s.headers.iter().any(|(k, v)| k == "Accept" && v.contains(',')));
        assert_eq!(s.available_at, Some(1_793_373_866));
        assert_eq!(s.expires_at, Some(1_793_395_466));
        let f = &s.format;
        assert_eq!((f.id.as_str(), f.width, f.height, f.fps), ("298+251-8", Some(1280), Some(720), Some(60)));
        assert_eq!((f.vcodec.as_str(), f.acodec.as_str()), ("avc1.4D4020", "opus"));
    }

    #[test]
    fn parses_chapters_qualities_and_captions() {
        let s = parse_streams(&fixture()).unwrap();
        let channel = s.channel.as_ref().unwrap();
        assert_eq!((channel.name.as_str(), channel.handle.as_deref()), ("3Blue1Brown", Some("@3blue1brown")));
        assert_eq!(s.duration_secs, Some(1120));
        let chapters: Vec<(u32, &str)> = s.chapters.iter().map(|c| (c.start_secs, c.title.as_str())).collect();
        assert_eq!(chapters, [(0, "Introduction example"), (67, "Series preview"), (162, "What are neurons?")]);
        // H.264 where there is one, VP9 for the other sizes. No muxed format 18.
        let q: Vec<(u32, &str)> = s.qualities.iter().map(|q| (q.res, q.url.as_str())).collect();
        assert_eq!(q.len(), 6);
        assert_eq!(q.iter().map(|q| q.0).collect::<Vec<_>>(), [144, 240, 360, 480, 720, 1080]);
        assert!(q[1].1.contains("itag=242"), "VP9 when there is no H.264");
        assert!(q[2].1.contains("itag=134"));
        assert!(q[4].1.contains("itag=298"), "the last, best H.264 stream of a size");
        let langs: Vec<(&str, bool)> = s.captions.iter().map(|c| (c.lang.as_str(), c.auto)).collect();
        assert_eq!(langs, [("de", false), ("en", false), ("en-orig", true), ("fr", true)]);
        assert!(s.captions.iter().all(|c| c.url.contains("fmt=vtt")));
    }

    #[test]
    fn qualities_go_up_to_vp9_2160p_without_hdr() {
        let f = |id: &str, h: u32, vcodec: &str, range: Option<&str>, protocol: &str| {
            serde_json::json!({
                "format_id": id, "url": format!("https://x/{id}"), "width": h * 16 / 9, "height": h,
                "vcodec": vcodec, "acodec": "none", "protocol": protocol, "dynamic_range": range,
            })
        };
        let formats: Vec<Info> = serde_json::from_value(serde_json::json!([
            f("137", 1080, "avc1.640028", Some("SDR"), "https"),
            f("248", 1080, "vp9", Some("SDR"), "https"),
            f("271", 1440, "vp9", Some("SDR"), "https"),
            f("336", 1440, "vp09.02.51.10", Some("HDR10"), "https"),
            f("313", 2160, "vp9", None, "https"),
            f("401", 2160, "av01.0.12M.08", Some("SDR"), "https"),
            f("616", 1080, "vp09.00.40.08", Some("SDR"), "m3u8_native"),
        ]))
        .unwrap();
        let q: Vec<(u32, u32, String)> = qualities(&formats).into_iter().map(|q| (q.res, q.long, q.url)).collect();
        assert_eq!(
            q,
            [
                (1080, 1920, "https://x/137".into()),
                (1440, 2560, "https://x/271".into()),
                (2160, 3840, "https://x/313".into())
            ]
        );
    }

    fn caption(lang: &str, auto: bool) -> Caption {
        Caption { lang: lang.into(), name: lang.into(), url: format!("https://x/{lang}/{auto}"), auto }
    }

    #[test]
    fn picks_captions_by_language() {
        let translated = |lang: &str| Caption {
            lang: lang.into(),
            name: lang.into(),
            url: format!("https://x/{lang}?tlang={lang}"),
            auto: true,
        };
        let all = [caption("de", false), caption("en-US", false), caption("en-orig", true), translated("en")];
        assert_eq!(pick_caption(&all, "en").unwrap().lang, "en-US");
        assert_eq!(pick_caption(&all, "EN-us").unwrap().lang, "en-US", "case does not matter");
        assert_eq!(pick_caption(&all[2..], "en").unwrap().lang, "en-orig", "recognition before translation");
        assert_eq!(pick_caption(&all[3..], "en").unwrap().lang, "en");
        assert_eq!(pick_caption(&all, "e"), None, "a prefix is not a language");
        assert_eq!(pick_caption(&all, "fr"), None);
        // A human track in a related form beats a machine translation.
        let pt = [caption("pt", false), translated("pt-BR")];
        assert_eq!(pick_caption(&pt, "pt-BR").unwrap().lang, "pt");
        let zh = [translated("zh-Hans"), caption("zh-CN", false)];
        assert_eq!(pick_caption(&zh, "zh-Hans").unwrap().lang, "zh-CN");
        let en = [caption("en", false)];
        assert_eq!(pick_caption(&en, "en-GB").unwrap().lang, "en");
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
    fn available_at_can_be_a_float() {
        let json = br#"{"url": "https://x/v", "vcodec": "avc1", "available_at": 1793373866.25}"#;
        assert_eq!(parse_streams(json).unwrap().available_at, Some(1_793_373_867));
    }

    #[test]
    fn reads_expire() {
        assert_eq!(expire("https://x/v?a=1&expire=42&b=2"), Some(42));
        assert_eq!(expire("https://x/v?a=1"), None);
        assert_eq!(expire("https://x/v"), None);
    }
}
