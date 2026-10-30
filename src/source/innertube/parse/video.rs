//! Video details from `next` and `player`.

use serde_json::Value;

use super::text::{array, channel_ref, clean, str_at, text, text_at, thumbnails};
use crate::source::model::{ChannelRef, Chapter, Description, Item, Link, LinkTarget, Video, VideoDetails, VideoKind};
use crate::source::{Result, SourceError};

const CONTENTS: &str = "/contents/twoColumnWatchNextResults/results/results/contents";
const RELATED: &str = "/contents/twoColumnWatchNextResults/secondaryResults/secondaryResults/results";
const PLAYER_BAR: &str = "/playerOverlays/playerOverlayRenderer/decoratedPlayerBarRenderer/decoratedPlayerBarRenderer/playerBar/multiMarkersPlayerBarRenderer/markersMap";
const LIKE_BUTTON: &str = "/likeButtonViewModel/likeButtonViewModel/toggleButtonViewModel/toggleButtonViewModel/defaultButtonViewModel/buttonViewModel/title";

/// Reads the details of one video. `player` fills in the duration, the ISO
/// date, the thumbnails, and fields that `next` lacks.
pub fn details(next: &Value, player: Option<&Value>) -> Result<VideoDetails> {
    let contents = next
        .pointer(CONTENTS)
        .and_then(Value::as_array)
        .ok_or_else(|| SourceError::Parse("next response has no watch page".into()))?;
    // The renderers come in a set order, but some are optional: select by key.
    let find = |key: &str| contents.iter().find_map(|c| c.get(key)).unwrap_or(&Value::Null);
    let primary = find("videoPrimaryInfoRenderer");
    let secondary = find("videoSecondaryInfoRenderer");
    let pd = player.and_then(|p| p.get("videoDetails")).unwrap_or(&Value::Null);

    let id = str_at(next, "/currentVideoEndpoint/watchEndpoint/videoId")
        .or_else(|| str_at(pd, "/videoId"))
        .ok_or_else(|| SourceError::Parse("next response has no video id".into()))?
        .to_string();

    let views = &primary["viewCount"]["videoViewCountRenderer"];
    let live = views["isLive"].as_bool() == Some(true) || pd["isLive"].as_bool() == Some(true);
    let likes = array(primary, "/videoActions/menuRenderer/topLevelButtons")
        .iter()
        .find_map(|b| text_at(b.get("segmentedLikeDislikeButtonViewModel")?, LIKE_BUTTON))
        // A video with hidden likes shows the label "Like" here.
        .filter(|t| t.starts_with(|c: char| c.is_ascii_digit()));
    let duration_secs = str_at(pd, "/lengthSeconds").and_then(|s| s.parse().ok()).filter(|&s| s > 0 && !live);

    Ok(VideoDetails {
        title: text_at(primary, "/title").or_else(|| text_at(pd, "/title")).unwrap_or_default(),
        channel: channel(secondary, pd),
        subscribers_text: text_at(secondary, "/owner/videoOwnerRenderer/subscriberCountText"),
        views_text: text_at(views, "/viewCount").or_else(|| player_views(pd)),
        date_text: text_at(primary, "/dateText"),
        publish_date: player
            .and_then(|p| str_at(p, "/microformat/playerMicroformatRenderer/publishDate"))
            .map(str::to_string),
        duration_secs,
        likes_text: likes,
        live,
        description: description(&secondary["attributedDescription"], &id)
            .or_else(|| str_at(pd, "/shortDescription").map(|s| Description { text: s.to_string(), links: Vec::new() }))
            .unwrap_or_default(),
        chapters: chapters(next),
        thumbnails: thumbnails(pd, "/thumbnail/thumbnails"),
        related: related(next),
        id,
    })
}

/// The related videos that can play next: no Shorts, playlists, or
/// members-only videos.
fn related(next: &Value) -> Vec<Video> {
    array(next, RELATED)
        .iter()
        .flat_map(super::items)
        .filter_map(|item| match item {
            Item::Video(v) if v.kind != VideoKind::Short && !v.members_only => Some(v),
            _ => None,
        })
        .collect()
}

/// The channel from `videoOwnerRenderer`, with the ID from `player` if it is missing.
fn channel(secondary: &Value, pd: &Value) -> Option<ChannelRef> {
    let owner = &secondary["owner"]["videoOwnerRenderer"];
    let player_id = str_at(pd, "/channelId").map(str::to_string);
    let first_run = str_at(owner, "/title/runs/0/text").map(clean).filter(|n| !n.is_empty());
    let Some(name) = first_run.or_else(|| text_at(owner, "/title")) else {
        let name = text_at(pd, "/author")?;
        return Some(ChannelRef { id: player_id, name, handle: None });
    };
    let mut c = channel_ref(name, owner.pointer("/navigationEndpoint/browseEndpoint"));
    if c.id.is_none() {
        c.id = player_id;
    }
    Some(c)
}

/// `viewCount` in `player` is a bare number, "25037777".
fn player_views(pd: &Value) -> Option<String> {
    let n = str_at(pd, "/viewCount")?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut out = String::new();
    for (i, c) in n.chars().enumerate() {
        if i > 0 && (n.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    Some(format!("{out} views"))
}

/// `attributedDescription`. `None` when it has no text.
fn description(d: &Value, video_id: &str) -> Option<Description> {
    // Not cleaned or trimmed: the link offsets count from the raw text.
    let content = str_at(d, "/content")?;
    let bounds = utf16_bounds(content);
    let links = array(d, "/commandRuns")
        .iter()
        .filter_map(|run| {
            let start = usize::try_from(run["startIndex"].as_u64()?).ok()?;
            let len = usize::try_from(run["length"].as_u64()?).ok()?;
            let Some((start, len)) = char_range(&bounds, start, len) else {
                tracing::debug!(start, len, "skipping description link with bad offsets");
                return None;
            };
            let target = link_target(&run["onTap"]["innertubeCommand"], video_id)?;
            Some(Link { start, len, target })
        })
        .collect();
    Some(Description { text: content.to_string(), links })
}

/// The UTF-16 offset of each char in `s`, and the length of `s` in UTF-16 at the end.
fn utf16_bounds(s: &str) -> Vec<usize> {
    let mut bounds = Vec::with_capacity(s.len() + 1);
    let mut units = 0;
    for c in s.chars() {
        bounds.push(units);
        units += c.len_utf16();
    }
    bounds.push(units);
    bounds
}

/// A UTF-16 range as a char range. `None` when it is empty, ends past the
/// text, or splits a surrogate pair.
fn char_range(bounds: &[usize], start: usize, len: usize) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let first = bounds.binary_search(&start).ok()?;
    let end = bounds.binary_search(&start.checked_add(len)?).ok()?;
    Some((first, end - first))
}

fn link_target(command: &Value, video_id: &str) -> Option<LinkTarget> {
    if let Some(url) = str_at(command, "/urlEndpoint/url") {
        return Some(LinkTarget::Url(redirect_target(url).unwrap_or_else(|| url.to_string())));
    }
    if let Some(watch) = command.get("watchEndpoint") {
        let id = str_at(watch, "/videoId")?.to_string();
        let start = watch["startTimeSeconds"].as_u64().and_then(|s| u32::try_from(s).ok());
        // A link to this same video is a timestamp. "0:00" has no start time.
        let start_secs = if start.is_none() && id == video_id { Some(0) } else { start };
        return Some(LinkTarget::Video { id, start_secs });
    }
    if let Some(id) = str_at(command, "/reelWatchEndpoint/videoId") {
        return Some(LinkTarget::Video { id: id.to_string(), start_secs: None });
    }
    if let Some(browse) = command.get("browseEndpoint") {
        let id = str_at(browse, "/browseId")?;
        if id.starts_with("UC") {
            return Some(LinkTarget::Channel { id: id.to_string() });
        }
        // Hashtags and other pages: a link to the YouTube page.
        let path = str_at(browse, "/canonicalBaseUrl").filter(|p| p.starts_with('/'))?;
        return Some(LinkTarget::Url(format!("https://www.youtube.com{path}")));
    }
    tracing::debug!("skipping description link with unknown command");
    None
}

/// The real URL in a `youtube.com/redirect?...&q=<url>` link.
fn redirect_target(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, rest) = rest.split_once('/')?;
    let query = rest.strip_prefix("redirect?")?;
    if !matches!(host, "www.youtube.com" | "youtube.com" | "m.youtube.com") {
        return None;
    }
    let q = query.split('&').find_map(|pair| pair.strip_prefix("q="))?;
    Some(percent_decode(q)).filter(|s| !s.is_empty())
}

/// Decodes `%XX` sequences. Other text, `+` and broken sequences stay as they
/// are. Gives the input back if the result is not UTF-8.
fn percent_decode(s: &str) -> String {
    let hex = |b: u8| char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok());
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some((h, l)) = bytes.get(i + 1).and_then(|&h| hex(h)).zip(bytes.get(i + 2).and_then(|&l| hex(l)))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// The chapters a video's uploader wrote, or else YouTube's automatic ones.
/// Each kind is in the player bar, and also in a panel.
const CHAPTER_SOURCES: [(&str, &str); 2] = [
    ("DESCRIPTION_CHAPTERS", "engagement-panel-macro-markers-description-chapters"),
    ("AUTO_CHAPTERS", "engagement-panel-macro-markers-auto-chapters"),
];

fn chapters(next: &Value) -> Vec<Chapter> {
    for (key, panel) in CHAPTER_SOURCES {
        let mut list = bar_chapters(next, key);
        if list.is_empty() {
            list = panel_chapters(next, panel);
        }
        if !list.is_empty() {
            list.sort_by_key(|c| c.start_secs);
            return list;
        }
    }
    Vec::new()
}

fn bar_chapters(next: &Value, key: &str) -> Vec<Chapter> {
    array(next, PLAYER_BAR)
        .iter()
        .find(|m| m["key"] == key)
        .map(|m| array(m, "/value/chapters"))
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let c = c.get("chapterRenderer")?;
            let start = u32::try_from(c["timeRangeStartMillis"].as_u64()? / 1000).ok()?;
            Some(Chapter { title: text_at(c, "/title")?, start_secs: start })
        })
        .collect()
}

fn panel_chapters(next: &Value, panel: &str) -> Vec<Chapter> {
    array(next, "/engagementPanels")
        .iter()
        .filter_map(|p| p.get("engagementPanelSectionListRenderer"))
        .find(|p| p["panelIdentifier"] == panel)
        .map(|p| array(p, "/content/macroMarkersListRenderer/contents"))
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let c = c.get("macroMarkersListItemRenderer")?;
            let start = u32::try_from(c.pointer("/onTap/watchEndpoint/startTimeSeconds")?.as_u64()?).ok()?;
            Some(Chapter { title: text(&c["title"])?, start_secs: start })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn auto_chapters_when_the_uploader_wrote_none() {
        let chapter = |title: &str, ms: u64| json!({"chapterRenderer": {"title": {"simpleText": title}, "timeRangeStartMillis": ms}});
        let next = |markers: Value| {
            json!({"playerOverlays": {"playerOverlayRenderer": {"decoratedPlayerBarRenderer": {"decoratedPlayerBarRenderer":
                {"playerBar": {"multiMarkersPlayerBarRenderer": {"markersMap": markers}}}}}}})
        };
        let auto =
            next(json!([{"key": "AUTO_CHAPTERS", "value": {"chapters": [chapter("B", 60_000), chapter("A", 0)]}}]));
        let titles: Vec<String> = chapters(&auto).into_iter().map(|c| c.title).collect();
        assert_eq!(titles, ["A", "B"]);
        let both = next(json!([
            {"key": "AUTO_CHAPTERS", "value": {"chapters": [chapter("auto", 0)]}},
            {"key": "DESCRIPTION_CHAPTERS", "value": {"chapters": [chapter("written", 0)]}}
        ]));
        assert_eq!(chapters(&both)[0].title, "written", "the uploader's chapters come first");
    }

    use std::fmt::Write;
    use std::path::Path;

    use serde_json::json;

    use super::*;

    fn load(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/innertube/{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    fn summary(d: &VideoDetails) -> String {
        let or_dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "-".into());
        let mut out = String::new();
        writeln!(out, "id: {}", d.id).unwrap();
        writeln!(out, "title: {}", d.title).unwrap();
        writeln!(out, "channel: {:?}", d.channel).unwrap();
        writeln!(out, "subscribers: {}", or_dash(&d.subscribers_text)).unwrap();
        writeln!(out, "views: {}", or_dash(&d.views_text)).unwrap();
        writeln!(out, "date: {}", or_dash(&d.date_text)).unwrap();
        writeln!(out, "publish date: {}", or_dash(&d.publish_date)).unwrap();
        writeln!(out, "duration: {:?}", d.duration_secs).unwrap();
        writeln!(out, "likes: {}", or_dash(&d.likes_text)).unwrap();
        writeln!(out, "live: {}", d.live).unwrap();
        let text = &d.description.text;
        writeln!(out, "description: {} chars", text.chars().count()).unwrap();
        writeln!(out, "{:?}", text.chars().take(200).collect::<String>()).unwrap();
        writeln!(out, "links: {}", d.description.links.len()).unwrap();
        for l in &d.description.links {
            let slice: String = text.chars().skip(l.start).take(l.len).collect();
            writeln!(out, "  {}+{} {slice:?} -> {:?}", l.start, l.len, l.target).unwrap();
        }
        writeln!(out, "chapters: {}", d.chapters.len()).unwrap();
        for c in &d.chapters {
            writeln!(out, "  {} {}", c.start_secs, c.title).unwrap();
        }
        writeln!(out, "related: {}", d.related.len()).unwrap();
        for v in d.related.iter().take(3) {
            writeln!(out, "  {} {}", v.id, v.title).unwrap();
        }
        writeln!(out, "thumbnails: {}", d.thumbnails.len()).unwrap();
        for t in &d.thumbnails {
            writeln!(out, "  {}x{} {}", t.width, t.height, t.url).unwrap();
        }
        out
    }

    #[test]
    fn fixture_snapshots() {
        let cases =
            [("next_chapters", Some("player_chapters")), ("next_live", Some("player_live")), ("next_normal", None)];
        for (next, player) in cases {
            let player = player.map(load);
            let d = details(&load(next), player.as_ref()).unwrap();
            insta::assert_snapshot!(next, summary(&d));
        }
    }

    #[test]
    fn fixture_links_match_their_text() {
        let d = details(&load("next_live"), None).unwrap();
        let first = &d.description.links[0];
        let slice: String = d.description.text.chars().skip(first.start).take(first.len).collect();
        assert_eq!(slice, "https://lnk.to/halloween-lofi02");
        assert_eq!(first.target, LinkTarget::Url("https://lnk.to/halloween-lofi02".into()));
    }

    fn with_description(content: &str, runs: &Value) -> Value {
        json!({
            "currentVideoEndpoint": {"watchEndpoint": {"videoId": "self"}},
            "contents": {"twoColumnWatchNextResults": {"results": {"results": {"contents": [
                {"videoSecondaryInfoRenderer": {"attributedDescription": {"content": content, "commandRuns": runs}}}
            ]}}}}
        })
    }

    fn url_run(start: usize, length: usize, url: &str) -> Value {
        json!({"startIndex": start, "length": length, "onTap": {"innertubeCommand": {"urlEndpoint": {"url": url}}}})
    }

    fn links(content: &str, runs: &Value) -> Vec<(String, LinkTarget)> {
        let d = details(&with_description(content, runs), None).unwrap();
        let text = &d.description.text;
        d.description
            .links
            .iter()
            .map(|l| (text.chars().skip(l.start).take(l.len).collect(), l.target.clone()))
            .collect()
    }

    #[test]
    fn link_offsets_are_utf16() {
        // "é" is one unit; "📚" and "𝔸" are two each (surrogate pairs).
        let content = "é📚 𝔸 https://a.example x";
        let start = "é📚 𝔸 ".encode_utf16().count();
        assert_eq!(start, 7);
        let got = links(content, &json!([url_run(start, 17, "https://a.example")]));
        assert_eq!(got, [("https://a.example".to_string(), LinkTarget::Url("https://a.example".into()))]);
    }

    #[test]
    fn bad_link_offsets_are_skipped() {
        let runs = json!([
            // Starts inside the surrogate pair.
            url_run(1, 2, "https://a"),
            // Ends past the text.
            url_run(3, 10, "https://b"),
            // Starts past the text.
            url_run(100, 1, "https://c"),
            // Empty.
            url_run(3, 0, "https://d"),
            // No offsets.
            {"onTap": {"innertubeCommand": {"urlEndpoint": {"url": "https://e"}}}},
            // Overflows.
            url_run(usize::MAX, usize::MAX, "https://f"),
            // Good: "abc".
            url_run(3, 3, "https://g"),
        ]);
        assert_eq!(links("📚 abc", &runs), [("abc".to_string(), LinkTarget::Url("https://g".into()))]);
    }

    #[test]
    fn decodes_percent_sequences() {
        assert_eq!(percent_decode("https%3A%2F%2Fa.example%2Fx"), "https://a.example/x");
        assert_eq!(percent_decode("a+b"), "a+b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("%e2%9c%93"), "✓");
        // Not UTF-8 after decoding: the input is kept.
        assert_eq!(percent_decode("%ff"), "%ff");
    }

    #[test]
    fn redirect_links_give_the_real_url() {
        let redirect = "https://www.youtube.com/redirect?event=video_description&q=https%3A%2F%2Fa.example%2Fp%3Fx%3D1%26y%3D2&v=self";
        assert_eq!(redirect_target(redirect).as_deref(), Some("https://a.example/p?x=1&y=2"));
        assert_eq!(redirect_target("https://www.youtube.com/redirect?event=x"), None);
        assert_eq!(redirect_target("https://evil.example/redirect?q=https%3A%2F%2Fx"), None);
        let got = links("a b", &json!([url_run(0, 1, "https://other.example/?q=1"), url_run(2, 1, redirect)]));
        assert_eq!(got[0].1, LinkTarget::Url("https://other.example/?q=1".into()));
        assert_eq!(got[1].1, LinkTarget::Url("https://a.example/p?x=1&y=2".into()));
    }

    #[test]
    fn watch_and_browse_links() {
        let run = |start: usize, command: Value| json!({"startIndex": start, "length": 1, "onTap": {"innertubeCommand": command}});
        let runs = json!([
            run(0, json!({"watchEndpoint": {"videoId": "self"}})),
            run(1, json!({"watchEndpoint": {"videoId": "self", "startTimeSeconds": 67}})),
            run(2, json!({"watchEndpoint": {"videoId": "other"}})),
            run(3, json!({"browseEndpoint": {"browseId": "UCx", "canonicalBaseUrl": "/@x"}})),
            run(4, json!({"browseEndpoint": {"browseId": "FEhashtag", "canonicalBaseUrl": "/hashtag/rust"}})),
            run(5, json!({"somethingNew": {}})),
        ]);
        let targets: Vec<LinkTarget> = links("abcdef", &runs).into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            targets,
            [
                LinkTarget::Video { id: "self".into(), start_secs: Some(0) },
                LinkTarget::Video { id: "self".into(), start_secs: Some(67) },
                LinkTarget::Video { id: "other".into(), start_secs: None },
                LinkTarget::Channel { id: "UCx".into() },
                LinkTarget::Url("https://www.youtube.com/hashtag/rust".into()),
            ]
        );
    }

    fn empty_next() -> Value {
        json!({"contents": {"twoColumnWatchNextResults": {"results": {"results": {"contents": []}}}}})
    }

    #[test]
    fn player_fills_in_missing_fields() {
        let player = json!({"videoDetails": {
            "videoId": "abc",
            "title": "From player",
            "lengthSeconds": "0",
            "channelId": "UCp",
            "author": "Someone",
            "viewCount": "1234567",
            "shortDescription": "Plain text"
        }});
        let d = details(&empty_next(), Some(&player)).unwrap();
        assert_eq!(d.id, "abc");
        assert_eq!(d.title, "From player");
        assert_eq!(d.duration_secs, None);
        assert_eq!(d.channel, Some(ChannelRef { id: Some("UCp".into()), name: "Someone".into(), handle: None }));
        assert_eq!(d.views_text.as_deref(), Some("1,234,567 views"));
        assert_eq!(d.description.text, "Plain text");
        assert!(d.chapters.is_empty());

        let player = json!({"videoDetails": {"videoId": "abc", "lengthSeconds": "1120"}});
        assert_eq!(details(&empty_next(), Some(&player)).unwrap().duration_secs, Some(1120));
        let player = json!({"videoDetails": {"videoId": "abc", "lengthSeconds": "60", "isLive": true}});
        assert_eq!(details(&empty_next(), Some(&player)).unwrap().duration_secs, None);
    }

    #[test]
    fn formats_player_view_counts() {
        let views = |n: &str| player_views(&json!({"viewCount": n}));
        assert_eq!(views("7").as_deref(), Some("7 views"));
        assert_eq!(views("123").as_deref(), Some("123 views"));
        assert_eq!(views("1234").as_deref(), Some("1,234 views"));
        assert_eq!(views("25037777").as_deref(), Some("25,037,777 views"));
        assert_eq!(views(""), None);
        assert_eq!(views("12a"), None);
    }

    #[test]
    fn chapters_fall_back_to_the_panel_and_are_sorted() {
        let item = |title: &str, secs: u32| {
            json!({"macroMarkersListItemRenderer": {
                "title": {"simpleText": title},
                "onTap": {"watchEndpoint": {"startTimeSeconds": secs}}
            }})
        };
        let quiz = json!({"key": "QUIZ_MARKERS", "value": {"chapters": [
            {"chapterRenderer": {"title": {"simpleText": "Quiz"}, "timeRangeStartMillis": 0}}
        ]}});
        let mut next = empty_next();
        next["currentVideoEndpoint"] = json!({"watchEndpoint": {"videoId": "a"}});
        next["playerOverlays"] = json!({"playerOverlayRenderer": {"decoratedPlayerBarRenderer": {
            "decoratedPlayerBarRenderer": {"playerBar": {"multiMarkersPlayerBarRenderer": {"markersMap": [quiz]}}}
        }}});
        next["engagementPanels"] = json!([
            {"engagementPanelSectionListRenderer": {"panelIdentifier": "comments"}},
            {"engagementPanelSectionListRenderer": {
                "panelIdentifier": "engagement-panel-macro-markers-description-chapters",
                "content": {"macroMarkersListRenderer": {"contents": [item("Later", 67), item("Intro", 0)]}}
            }}
        ]);
        let got: Vec<(u32, String)> =
            details(&next, None).unwrap().chapters.into_iter().map(|c| (c.start_secs, c.title)).collect();
        assert_eq!(got, [(0, "Intro".to_string()), (67, "Later".to_string())]);
    }

    #[test]
    fn a_response_without_a_watch_page_is_an_error() {
        for bad in [json!({}), json!({"contents": {}}), load("player_live"), empty_next()] {
            assert!(matches!(details(&bad, None), Err(SourceError::Parse(_))), "{bad}");
        }
    }
}
