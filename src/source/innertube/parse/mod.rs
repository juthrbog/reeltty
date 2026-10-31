//! Turns InnerTube JSON into the model types.
//!
//! YouTube changes its renderers without notice. Unknown items are skipped
//! and logged at debug level. Only a page with no result list at all is an
//! error.

mod browse;
mod comments;
mod renderers;
mod text;
mod video;

use serde_json::Value;

use crate::source::model::{
    ChannelPage, CommentPage, Item, ItemPage, PlaylistPage, SearchPage, Spelling, VideoDetails,
};
use crate::source::{Result, SourceError};
use text::{array, continuation_token, str_at, text_at};

const RESULT_LIST: &str = "/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents";

/// The first page of a `search` response.
pub fn search_page(response: &Value) -> Result<SearchPage> {
    let sections = response
        .pointer(&format!("/contents{RESULT_LIST}"))
        .and_then(Value::as_array)
        .ok_or_else(|| SourceError::Parse("search response has no result list".into()))?;
    Ok(page(sections, response))
}

/// A `search` continuation response. This is also the answer to a chip
/// token, which reloads the whole result list.
pub fn search_continuation(response: &Value) -> Result<SearchPage> {
    let commands = array(response, "/onResponseReceivedCommands");
    let appended = commands.iter().find_map(|c| c.pointer("/appendContinuationItemsAction/continuationItems"));
    let reloaded = || {
        commands.iter().find_map(|c| {
            let reload = c.get("reloadContinuationItemsCommand")?;
            if reload["slot"] != "RELOAD_CONTINUATION_SLOT_BODY" {
                return None;
            }
            reload.pointer(&format!("/continuationItems/0{RESULT_LIST}"))
        })
    };
    let sections = appended
        .or_else(reloaded)
        .and_then(Value::as_array)
        .ok_or_else(|| SourceError::Parse("search continuation has no items".into()))?;
    Ok(page(sections, response))
}

/// Reads the sections of a result list. Items are in `itemSectionRenderer`s;
/// the next-page token is a sibling of them.
fn page(sections: &[Value], response: &Value) -> SearchPage {
    let mut page = SearchPage { estimated_results: estimated_results(response), ..SearchPage::default() };
    for section in sections {
        if let Some(contents) = section.pointer("/itemSectionRenderer/contents").and_then(Value::as_array) {
            for item in contents {
                match spelling(item) {
                    Some(s) => page.spelling = Some(s),
                    None => page.items.extend(items(item)),
                }
            }
        } else if let Some(token) = continuation_token(section) {
            page.continuation.get_or_insert(token);
        } else {
            tracing::debug!(keys = ?keys(section), "skipping unknown search section");
        }
    }
    page
}

/// `estimatedResults` is a string, "6192373".
fn estimated_results(response: &Value) -> Option<u64> {
    let v = response.get("estimatedResults")?;
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

fn spelling(item: &Value) -> Option<Spelling> {
    let corrected = |r: &Value| {
        str_at(r, "/correctedQueryEndpoint/searchEndpoint/query")
            .map(str::to_string)
            .or_else(|| text_at(r, "/correctedQuery"))
            .map(|q| q.trim().to_string())
    };
    if let Some(r) = item.get("showingResultsForRenderer") {
        let original = text_at(r, "/originalQuery")
            .or_else(|| str_at(r, "/originalQueryEndpoint/searchEndpoint/query").map(str::to_string))?;
        return Some(Spelling::ShowingResultsFor { corrected: corrected(r)?, original });
    }
    let r = item.get("didYouMeanRenderer")?;
    Some(Spelling::DidYouMean { corrected: corrected(r)? })
}

/// One item object, such as `{"videoRenderer": {...}}`. Containers (shelves)
/// give several items. Unknown keys give none.
pub fn items(item: &Value) -> Vec<Item> {
    let Some(object) = item.as_object() else { return Vec::new() };
    for (key, value) in object {
        if let Some(found) = dispatch(key, value) {
            return found;
        }
    }
    tracing::debug!(keys = ?keys(item), "skipping unknown item");
    Vec::new()
}

/// The items for one renderer key. `None` when the key is unknown.
fn dispatch(key: &str, value: &Value) -> Option<Vec<Item>> {
    let one = |item: Option<Item>| {
        if item.is_none() {
            tracing::debug!(key, "skipping item with missing fields");
        }
        item.into_iter().collect()
    };
    let each = |list: &[Value]| list.iter().flat_map(items).collect();
    let found = match key {
        "videoRenderer" => one(renderers::video_renderer(value).map(Item::Video)),
        "lockupViewModel" => one(renderers::lockup(value)),
        "shortsLockupViewModel" => one(renderers::shorts_lockup(value).map(Item::Video)),
        "channelRenderer" | "gridChannelRenderer" => one(renderers::channel_renderer(value).map(Item::Channel)),
        "richItemRenderer" => items(&value["content"]),
        "gridShelfViewModel" => each(array(value, "/contents")),
        "reelShelfRenderer" => each(array(value, "/items")),
        "shelfRenderer" => {
            let vertical = array(value, "/content/verticalListRenderer/items");
            each(if vertical.is_empty() { array(value, "/content/horizontalListRenderer/items") } else { vertical })
        }
        // Read elsewhere: tokens and spelling messages.
        "continuationItemRenderer"
        | "continuationItemViewModel"
        | "showingResultsForRenderer"
        | "didYouMeanRenderer" => Vec::new(),
        _ => return None,
    };
    Some(found)
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default()
}

/// A channel `browse` response for the Videos tab.
pub fn channel_page(response: &Value) -> Result<ChannelPage> {
    browse::channel_page(response)
}

/// A playlist `browse` response (`VL...`).
pub fn playlist_page(response: &Value) -> Result<PlaylistPage> {
    browse::playlist_page(response)
}

/// A `browse` continuation response, for a channel grid or a playlist.
pub fn browse_continuation(response: &Value) -> Result<ItemPage> {
    browse::continuation(response)
}

/// The `next` response, and the `player` response if it was read, for one video.
pub fn video_details(next: &Value, player: Option<&Value>) -> Result<VideoDetails> {
    video::details(next, player)
}

/// The token for the first page of comments in a video's `next` response.
pub fn comments_token(next: &Value) -> Option<String> {
    comments::token(next)
}

/// Why a video's `next` response has no comments token.
pub fn comments_message(next: &Value) -> Option<String> {
    comments::message(next)
}

/// A `next` continuation response with comments or replies.
pub fn comment_page(response: &Value, video_id: &str) -> Result<CommentPage> {
    comments::page(response, video_id)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;
    use crate::source::model::{Channel, ChannelRef, Video, VideoKind};

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/innertube")
    }

    fn load(name: &str) -> Value {
        let path = fixture_dir().join(format!("{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    /// Continuation and chip responses go to `search_continuation`.
    fn parse_fixture(name: &str) -> SearchPage {
        let v = load(name);
        if name.contains("cont") || name.contains("chip") { search_continuation(&v) } else { search_page(&v) }.unwrap()
    }

    fn search_fixtures() -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(fixture_dir())
            .unwrap()
            .filter_map(|e| {
                let name = e.unwrap().file_name().into_string().unwrap();
                name.strip_prefix("search_").is_some().then(|| name.trim_end_matches(".json").to_string())
            })
            .collect();
        names.sort();
        names
    }

    fn channels(list: &[ChannelRef]) -> String {
        let one = |c: &ChannelRef| {
            let mut s = c.name.clone();
            for extra in [&c.id, &c.handle].into_iter().flatten() {
                write!(s, " {extra}").unwrap();
            }
            s
        };
        if list.is_empty() { "-".into() } else { list.iter().map(one).collect::<Vec<_>>().join(" + ") }
    }

    fn video_line(v: &Video) -> String {
        let mut flags: Vec<String> = Vec::new();
        if v.members_only {
            flags.push("members-only".into());
        }
        flags.extend(v.badges.iter().cloned());
        if v.description_snippet.is_some() {
            flags.push("snippet".into());
        }
        flags.push(format!("{} thumbs", v.thumbnails.len()));
        let duration = match (&v.duration_text, v.duration_secs) {
            (Some(t), Some(s)) => format!("{t} ({s}s)"),
            _ => "-".into(),
        };
        let or_dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "-".into());
        format!(
            "{:?} {} | {} | {} | {duration} | {} | {} | {}",
            v.kind,
            v.id,
            v.title,
            channels(&v.channels),
            or_dash(&v.views_text),
            or_dash(&v.published_text),
            flags.join(", ")
        )
    }

    fn summary(page: &SearchPage) -> String {
        let mut out = String::new();
        writeln!(out, "continuation: {}", if page.continuation.is_some() { "yes" } else { "no" }).unwrap();
        writeln!(out, "estimated results: {:?}", page.estimated_results).unwrap();
        writeln!(out, "spelling: {:?}", page.spelling).unwrap();
        writeln!(out, "items: {}", page.items.len()).unwrap();
        for item in &page.items {
            let line = match item {
                Item::Video(v) => video_line(v),
                Item::Channel(c) => format!(
                    "Channel {} | {} | {} | {} | {}{}{} thumbs",
                    c.id,
                    c.name,
                    c.handle.as_deref().unwrap_or("-"),
                    c.subscribers_text.as_deref().unwrap_or("-"),
                    if c.verified { "verified, " } else { "" },
                    if c.description_snippet.is_some() { "snippet, " } else { "" },
                    c.thumbnails.len()
                ),
                Item::Playlist(p) => format!(
                    "{:?} {} | {} | {} | {} | first {} | {} thumbs",
                    p.kind,
                    p.id,
                    p.title,
                    channels(p.channel.as_slice()),
                    p.count_text.as_deref().unwrap_or("-"),
                    p.first_video_id.as_deref().unwrap_or("-"),
                    p.thumbnails.len()
                ),
            };
            writeln!(out, "{line}").unwrap();
        }
        out
    }

    #[test]
    fn search_fixture_snapshots() {
        let names = search_fixtures();
        assert!(names.len() >= 10, "{names:?}");
        for name in names {
            insta::assert_snapshot!(name.clone(), summary(&parse_fixture(&name)));
        }
    }

    #[test]
    fn fixture_items_follow_the_model_rules() {
        let bidi = |s: &str| s.chars().any(|c| matches!(c, '\u{200E}' | '\u{200F}' | '\u{2066}'..='\u{2069}'));
        for name in search_fixtures() {
            let page = parse_fixture(&name);
            for item in &page.items {
                let (thumbs, refs, texts): (_, Vec<&ChannelRef>, Vec<&str>) = match item {
                    Item::Video(v) => (&v.thumbnails, v.channels.iter().collect(), vec![&v.id, &v.title]),
                    Item::Channel(c) => (&c.thumbnails, vec![], vec![&c.id, &c.name]),
                    Item::Playlist(p) => (&p.thumbnails, p.channel.iter().collect(), vec![&p.id, &p.title]),
                };
                assert!(thumbs.iter().all(|t| t.url.starts_with("https://")), "{name}: {item:?}");
                assert!(texts.iter().all(|t| !t.is_empty() && !bidi(t)), "{name}: {item:?}");
                for c in refs {
                    assert!(!bidi(&c.name), "{name}: {c:?}");
                    assert!(c.handle.as_ref().is_none_or(|h| h.starts_with('@')), "{name}: {c:?}");
                }
            }
        }
    }

    fn only_video(item: &Value) -> Video {
        match items(item).as_slice() {
            [Item::Video(v)] => v.clone(),
            other => panic!("expected one video, got {other:?}"),
        }
    }

    fn only_channel(item: &Value) -> Channel {
        match items(item).as_slice() {
            [Item::Channel(c)] => c.clone(),
            other => panic!("expected one channel, got {other:?}"),
        }
    }

    #[test]
    fn channel_fields_are_classified_by_content() {
        let swapped = json!({"channelRenderer": {
            "channelId": "UC1",
            "title": {"simpleText": "Jon Gjengset"},
            "subscriberCountText": {"simpleText": "@jonhoo"},
            "videoCountText": {"simpleText": "111K subscribers"},
            "thumbnail": {"thumbnails": [{"url": "//yt3.ggpht.com/x", "width": 88, "height": 88}]},
            "ownerBadges": [{"metadataBadgeRenderer": {"style": "BADGE_STYLE_TYPE_VERIFIED_ARTIST"}}]
        }});
        let c = only_channel(&swapped);
        assert_eq!(c.handle.as_deref(), Some("@jonhoo"));
        assert_eq!(c.subscribers_text.as_deref(), Some("111K subscribers"));
        assert!(c.verified);
        assert_eq!(c.thumbnails[0].url, "https://yt3.ggpht.com/x");

        // The grid renderer uses the fields as named, and has the handle only in the URL.
        let grid = json!({"gridChannelRenderer": {
            "channelId": "UC2",
            "title": {"simpleText": "ShortCircuit"},
            "subscriberCountText": {"simpleText": "2.55M subscribers"},
            "videoCountText": {"runs": [{"text": "1,370"}, {"text": " videos"}]},
            "navigationEndpoint": {"browseEndpoint": {"browseId": "UC2", "canonicalBaseUrl": "/@ShortCircuit"}}
        }});
        let c = only_channel(&grid);
        assert_eq!(c.handle.as_deref(), Some("@ShortCircuit"));
        assert_eq!(c.subscribers_text.as_deref(), Some("2.55M subscribers"));
        assert!(!c.verified);
    }

    fn collab_item(name: &str, id: &str, subtitle: &str) -> Value {
        json!({"listItemViewModel": {
            "title": {"content": name},
            "subtitle": {"content": subtitle},
            "rendererContext": {"commandContext": {"onTap": {"innertubeCommand": {"browseEndpoint": {"browseId": id}}}}}
        }})
    }

    fn collab_dialog(list: &[Value]) -> Value {
        json!({"showDialogCommand": {"panelLoadingStrategy": {"inlineContent": {"dialogViewModel": {
            "customContent": {"listViewModel": {"listItems": list}}
        }}}}})
    }

    #[test]
    fn collab_byline_gives_all_channels_without_bidi_marks() {
        let dialog = collab_dialog(&[
            collab_item(
                "Гриненко про",
                "UCa",
                "\u{200E}\u{2068}@GrinenkoPro\u{2069} • \u{2068}1.29K subscribers\u{2069}",
            ),
            collab_item(
                "Veged and Code",
                "UCb",
                "\u{200E}\u{2068}@VegedAndCode\u{2069} • \u{2068}1.13K subscribers\u{2069}",
            ),
        ]);
        let v = only_video(&json!({"videoRenderer": {
            "videoId": "R6jVO6ebGcw",
            "title": {"runs": [{"text": "Rust"}]},
            "longBylineText": {"runs": [{"text": "Гриненко про and Veged and Code", "navigationEndpoint": dialog}]}
        }}));
        let expect = |id: &str, name: &str, handle: &str| ChannelRef {
            id: Some(id.into()),
            name: name.into(),
            handle: Some(handle.into()),
        };
        assert_eq!(
            v.channels,
            [expect("UCa", "Гриненко про", "@GrinenkoPro"), expect("UCb", "Veged and Code", "@VegedAndCode")]
        );
    }

    #[test]
    fn live_and_past_stream_videos() {
        let live = only_video(&json!({"videoRenderer": {
            "videoId": "KYBnhxajXkc",
            "title": {"runs": [{"text": "Live now"}]},
            "viewCountText": {"runs": [{"text": "32"}, {"text": " watching"}]},
            "badges": [
                {"metadataBadgeRenderer": {"style": "BADGE_STYLE_TYPE_LIVE_NOW", "label": "LIVE"}},
                {"metadataBadgeRenderer": {"style": "BADGE_STYLE_TYPE_SIMPLE", "label": "New"}}
            ],
            "thumbnailOverlays": [{"thumbnailOverlayTimeStatusRenderer": {"text": {"simpleText": "LIVE"}, "style": "LIVE"}}]
        }}));
        assert_eq!(live.kind, VideoKind::Live);
        assert_eq!(live.views_text.as_deref(), Some("32 watching"));
        assert_eq!(live.badges, ["New"]);
        assert_eq!((live.duration_text, live.duration_secs), (None, None));

        let past = only_video(&json!({"videoRenderer": {
            "videoId": "8Eya3TEvwHU",
            "publishedTimeText": {"simpleText": "Streamed 2 months ago"},
            "lengthText": {"simpleText": "1:25:26"}
        }}));
        assert_eq!(past.kind, VideoKind::PastStream);
        assert_eq!(past.duration_secs, Some(5126));
    }

    fn lockup_rows(rows: &Value, image: &Value, badge: &Value) -> Value {
        json!({"lockupViewModel": {
            "contentId": "KxiUncxSnIo",
            "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
            "contentImage": {"thumbnailViewModel": {
                "image": {"sources": [{"url": "https://i.ytimg.com/vi/x.jpg", "width": 360, "height": 202}]},
                "overlays": [{"thumbnailBottomOverlayViewModel": {"badges": [{"thumbnailBadgeViewModel": badge}]}}]
            }},
            "metadata": {"lockupMetadataViewModel": {
                "title": {"content": "A video"},
                "image": image,
                "metadata": {"contentMetadataViewModel": {"metadataRows": rows}}
            }}
        }})
    }

    #[test]
    fn members_only_lockup() {
        let rows = json!([
            {"metadataParts": [{"text": {"content": "1 day ago"}, "accessibilityLabel": "1 day ago"}]},
            {"badges": [
                {"badgeViewModel": {"badgeText": "Members only", "badgeStyle": "BADGE_MEMBERS_ONLY"}},
                {"badgeViewModel": {"badgeText": "CC", "badgeStyle": "BADGE_DEFAULT"}}
            ]}
        ]);
        let image = json!({"decoratedAvatarViewModel": {
            "a11yLabel": "Go to channel Linus Tech Tips",
            "rendererContext": {"commandContext": {"onTap": {"innertubeCommand": {
                "browseEndpoint": {"browseId": "UCX", "canonicalBaseUrl": "/@LinusTechTips"}
            }}}}
        }});
        let badge = json!({"text": "15:16", "badgeStyle": "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT"});
        let v = only_video(&lockup_rows(&rows, &image, &badge));
        assert!(v.members_only);
        assert_eq!(v.badges, ["CC"]);
        assert_eq!(v.kind, VideoKind::Normal);
        assert_eq!(v.views_text, None);
        assert_eq!(v.published_text.as_deref(), Some("1 day ago"));
        assert_eq!(v.duration_secs, Some(916));
        assert_eq!(
            v.channels,
            [ChannelRef {
                id: Some("UCX".into()),
                name: "Linus Tech Tips".into(),
                handle: Some("@LinusTechTips".into())
            }]
        );
    }

    #[test]
    fn related_lockup_uses_the_long_labels() {
        let rows = json!([
            {"metadataParts": [{"text": {"content": "Policing Uncut"}}]},
            {"metadataParts": [
                {"text": {"content": "909K"}, "accessibilityLabel": "909 thousand views"},
                {"text": {"content": "1mo ago"}, "accessibilityLabel": "1 month ago"}
            ]}
        ]);
        let badge = json!({"text": "13:52", "badgeStyle": "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT"});
        let v = only_video(&lockup_rows(&rows, &Value::Null, &badge));
        assert_eq!(v.views_text.as_deref(), Some("909 thousand views"));
        assert_eq!(v.published_text.as_deref(), Some("1 month ago"));
        assert_eq!(v.channels, [ChannelRef { id: None, name: "Policing Uncut".into(), handle: None }]);

        let rows = json!([
            {"metadataParts": [{"text": {"content": "Lofi Girl"}}]},
            {"metadataParts": [{"text": {"content": "994 watching"}, "accessibilityLabel": "994 watching"}]}
        ]);
        let badge = json!({"text": "LIVE", "badgeStyle": "THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE"});
        let v = only_video(&lockup_rows(&rows, &Value::Null, &badge));
        assert_eq!(v.kind, VideoKind::Live);
        assert_eq!(v.views_text.as_deref(), Some("994 watching"));
        assert_eq!(v.duration_text, None);
    }

    #[test]
    fn collab_lockup_uses_the_avatar_stack() {
        let rows = json!([
            {"metadataParts": [{"text": {"content": "Linus Tech Tips and Zip Tie Tuning"}}]},
            {"metadataParts": [{"text": {"content": "2.1M views"}}, {"text": {"content": "3 weeks ago"}}]}
        ]);
        let dialog = collab_dialog(&[
            collab_item(
                "Linus Tech Tips",
                "UCa",
                "\u{200E}\u{2068}@LinusTechTips\u{2069} • \u{2068}16.9M subscribers\u{2069}",
            ),
            collab_item(
                "Zip Tie Tuning",
                "UCb",
                "\u{200E}\u{2068}@ZipTieTuning\u{2069} • \u{2068}761K subscribers\u{2069}",
            ),
        ]);
        let image = json!({"avatarStackViewModel": {"rendererContext": {"commandContext": {"onTap": {"innertubeCommand": dialog}}}}});
        let v = only_video(&lockup_rows(&rows, &image, &json!({})));
        let names: Vec<_> = v.channels.iter().map(|c| c.handle.as_deref().unwrap()).collect();
        assert_eq!(names, ["@LinusTechTips", "@ZipTieTuning"]);
        assert_eq!(v.views_text.as_deref(), Some("2.1M views"));
    }

    #[test]
    fn shelves_and_wrappers_are_unwrapped() {
        let short = |id: &str| {
            json!({"shortsLockupViewModel": {
                "entityId": format!("shorts-shelf-item-{id}"),
                "overlayMetadata": {"primaryText": {"content": "A short"}, "secondaryText": {"content": "84K views"}}
            }})
        };
        let video = |id: &str| json!({"videoRenderer": {"videoId": id, "title": {"runs": [{"text": "t"}]}}});
        let cases = [
            json!({"richItemRenderer": {"content": video("a")}}),
            json!({"gridShelfViewModel": {"contents": [short("a"), short("b")]}}),
            json!({"reelShelfRenderer": {"items": [short("a")]}}),
            json!({"shelfRenderer": {"content": {"verticalListRenderer": {"items": [video("a"), video("b")]}}}}),
            json!({"shelfRenderer": {"content": {"horizontalListRenderer": {"items": [video("a")]}}}}),
        ];
        let counts: Vec<usize> = cases.iter().map(|c| items(c).len()).collect();
        assert_eq!(counts, [1, 2, 1, 2, 1]);
        let s = only_video(&short("H6ML7P19ySw"));
        assert_eq!(
            (s.id.as_str(), s.kind, s.views_text.as_deref()),
            ("H6ML7P19ySw", VideoKind::Short, Some("84K views"))
        );
    }

    #[test]
    fn shopping_badges_are_dropped() {
        let v = only_video(&json!({"videoRenderer": {"videoId": "a", "badges": [
            {"metadataBadgeRenderer": {"label": "8 products"}},
            {"metadataBadgeRenderer": {"label": "1 product"}},
            {"metadataBadgeRenderer": {"label": "4K"}},
            {"metadataBadgeRenderer": {"label": "Top 10 products"}},
            {"metadataBadgeRenderer": {"label": "Fundraiser"}}
        ]}}));
        assert_eq!(v.badges, ["4K", "Top 10 products"]);
    }

    #[test]
    fn short_badge_is_kept() {
        let v = only_video(&json!({"shortsLockupViewModel": {
            "entityId": "shorts-shelf-item-x",
            "badge": {"badgeViewModel": {"badgeText": "New"}}
        }}));
        assert_eq!(v.badges, ["New"]);
    }

    #[test]
    fn playlist_labels_are_not_channels() {
        let lockup = |parts: Value| {
            json!({"lockupViewModel": {"contentType": "LOCKUP_CONTENT_TYPE_PLAYLIST", "contentId": "PLx",
                "metadata": {"lockupMetadataViewModel": {"title": {"content": "P"},
                    "metadata": {"contentMetadataViewModel": {"metadataRows": [{"metadataParts": parts}]}}}}}})
        };
        let channel = |v: &Value| match items(v).as_slice() {
            [Item::Playlist(p)] => p.channel.clone(),
            other => panic!("expected one playlist, got {other:?}"),
        };
        // Channel home: only the label.
        assert_eq!(channel(&lockup(json!([{"text": {"content": "Playlist"}}]))), None);
        // Search: the channel, then the label.
        let search = lockup(json!([
            {"text": {"content": "Some Channel", "commandRuns": [{"onTap": {"innertubeCommand": {"browseEndpoint": {"browseId": "UCx"}}}}]}},
            {"text": {"content": "Playlist"}}
        ]));
        let c = channel(&search).unwrap();
        assert_eq!((c.name.as_str(), c.id.as_deref()), ("Some Channel", Some("UCx")));
    }

    #[test]
    fn unknown_and_broken_items_are_skipped() {
        assert!(items(&json!({"adSlotRenderer": {"x": 1}})).is_empty());
        assert!(items(&json!({"videoRenderer": {"title": {"simpleText": "no id"}}})).is_empty());
        assert!(
            items(&json!({"lockupViewModel": {"contentType": "LOCKUP_CONTENT_TYPE_PODCAST", "contentId": "x"}}))
                .is_empty()
        );
        assert!(items(&json!("not an object")).is_empty());

        let response = json!({"contents": {"twoColumnSearchResultsRenderer": {"primaryContents": {"sectionListRenderer": {"contents": [
            {"itemSectionRenderer": {"contents": [
                {"adSlotRenderer": {}},
                {"videoRenderer": {"videoId": "a"}},
                {"videoRenderer": {}}
            ]}},
            {"somethingNew": {}},
            {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "next"}}}}
        ]}}}}, "estimatedResults": "42"});
        let page = search_page(&response).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.continuation.as_deref(), Some("next"));
        assert_eq!(page.estimated_results, Some(42));
    }

    #[test]
    fn continuation_view_model_ends_a_page_too() {
        let response = json!({"onResponseReceivedCommands": [{"appendContinuationItemsAction": {"continuationItems": [
            {"itemSectionRenderer": {"contents": [{"videoRenderer": {"videoId": "a"}}]}},
            {"continuationItemViewModel": {"continuationCommand": {"innertubeCommand": {"continuationCommand": {"token": "vm"}}}}}
        ]}}]});
        let page = search_continuation(&response).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.continuation.as_deref(), Some("vm"));
        assert_eq!(page.estimated_results, None);
    }

    #[test]
    fn unrecognisable_pages_are_errors() {
        for bad in [json!({}), json!({"responseContext": {}}), json!([1, 2])] {
            assert!(matches!(search_page(&bad), Err(SourceError::Parse(_))), "{bad}");
            assert!(matches!(search_continuation(&bad), Err(SourceError::Parse(_))), "{bad}");
        }
        // The first-page shape is not a continuation, and the other way round.
        assert!(search_continuation(&load("search_rust_programming")).is_err());
        assert!(search_page(&load("search_rust_programming_cont1")).is_err());
    }

    #[test]
    fn reads_spelling_messages() {
        let page = parse_fixture("search_misspelled");
        assert_eq!(
            page.spelling,
            Some(Spelling::ShowingResultsFor {
                corrected: "rust programing language".into(),
                original: "rust programing langauge".into()
            })
        );
        let page = parse_fixture("search_misspelled_nospell_video_views");
        assert_eq!(page.spelling, Some(Spelling::DidYouMean { corrected: "rust programing language".into() }));
    }
}
