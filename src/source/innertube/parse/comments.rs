//! Comments from `next`: the first token in the watch page, then pages of
//! comments and replies from continuation responses.
//!
//! The thread items hold only keys. The text, the author, and the counts are
//! entities in `frameworkUpdates`, joined on `entityKey`.

use std::collections::HashMap;

use serde_json::Value;

use super::text::{array, channel_ref, clean, continuation_token, str_at, text_at};
use super::video::{CONTENTS, description};
use crate::source::model::{Comment, CommentPage, CommentSort};
use crate::source::{Result, SourceError};

const SECTION: &str = "comment-item-section";
const PANEL: &str = "engagement-panel-comments-section";
const PANEL_ITEMS: &str = "/content/sectionListRenderer/contents/0/itemSectionRenderer/contents";
/// The "Show more replies" button at the end of a replies page.
const BUTTON_TOKEN: &str = "/continuationItemRenderer/button/buttonRenderer/command/continuationCommand/token";
const HEARTED: &str = "TOOLBAR_HEART_STATE_HEARTED";

/// The comments section of a watch page.
fn section(next: &Value) -> Option<&Value> {
    array(next, CONTENTS)
        .iter()
        .filter_map(|c| c.get("itemSectionRenderer"))
        .find(|s| s["sectionIdentifier"] == SECTION)
}

/// The comments panel of a watch page.
fn panel(next: &Value) -> Option<&Value> {
    array(next, "/engagementPanels")
        .iter()
        .filter_map(|p| p.get("engagementPanelSectionListRenderer"))
        .find(|p| p["panelIdentifier"] == PANEL)
}

/// The token for the first page of comments in a `next` response. `None`
/// when the video has no comments, or they are turned off.
pub fn token(next: &Value) -> Option<String> {
    let in_section = section(next).and_then(|s| array(s, "/contents").iter().find_map(continuation_token));
    in_section.or_else(|| array(panel(next)?, PANEL_ITEMS).iter().find_map(continuation_token))
}

/// The short comment count in a `next` response, "10M".
pub fn count_text(next: &Value) -> Option<String> {
    text_at(panel(next)?, "/header/engagementPanelTitleHeaderRenderer/contextualInfo")
}

/// Why a `next` response has no comments token, such as "Comments are turned
/// off.".
pub fn message(next: &Value) -> Option<String> {
    let items =
        section(next).map(|s| array(s, "/contents")).into_iter().chain(panel(next).map(|p| array(p, PANEL_ITEMS)));
    items.flatten().find_map(|i| text_at(i.get("messageRenderer")?, "/text"))
}

/// A page of comments or replies, from any comments token. `video_id` turns
/// links to the same video into timestamps.
pub fn page(response: &Value, video_id: &str) -> Result<CommentPage> {
    let entities: HashMap<&str, &Value> = array(response, "/frameworkUpdates/entityBatchUpdate/mutations")
        .iter()
        .filter_map(|m| Some((m["entityKey"].as_str()?, &m["payload"])))
        .collect();
    let mut commands = array(response, "/onResponseReceivedEndpoints");
    if commands.is_empty() {
        commands = array(response, "/onResponseReceivedCommands");
    }
    let lists: Vec<&[Value]> = commands
        .iter()
        .filter_map(|c| {
            c.pointer("/reloadContinuationItemsCommand/continuationItems")
                .or_else(|| c.pointer("/appendContinuationItemsAction/continuationItems"))
        })
        .filter_map(|items| items.as_array().map(Vec::as_slice))
        .collect();
    if lists.is_empty() {
        return Err(SourceError::Parse("comments response has no items".into()));
    }
    let mut page = CommentPage::default();
    for item in lists.into_iter().flatten() {
        if let Some(h) = item.get("commentsHeaderRenderer") {
            page.count_text = text_at(h, "/countText");
            page.sorts = sorts(h);
        } else if let Some(t) = item.get("commentThreadRenderer") {
            page.comments.extend(thread(t, &entities, video_id));
        } else if let Some(vm) = item.get("commentViewModel") {
            page.comments.extend(comment(vm, &entities, video_id));
        } else if let Some(token) = more_token(item) {
            page.continuation.get_or_insert(token);
        } else {
            tracing::debug!(keys = ?keys(item), "skipping unknown comments item");
        }
    }
    Ok(page)
}

/// The token of a continuation item: an automatic one, or the "Show more
/// replies" button.
fn more_token(item: &Value) -> Option<String> {
    continuation_token(item).or_else(|| str_at(item, BUTTON_TOKEN).map(str::to_string))
}

fn sorts(header: &Value) -> Vec<CommentSort> {
    array(header, "/sortMenu/sortFilterSubMenuRenderer/subMenuItems")
        .iter()
        .filter_map(|i| {
            Some(CommentSort {
                label: str_at(i, "/title").map(clean).filter(|l| !l.is_empty())?,
                selected: i["selected"].as_bool() == Some(true),
                continuation: str_at(i, "/serviceEndpoint/continuationCommand/token")?.to_string(),
            })
        })
        .collect()
}

/// A top-level comment and the token of its replies.
fn thread(t: &Value, entities: &HashMap<&str, &Value>, video_id: &str) -> Option<Comment> {
    let Some(vm) = t.pointer("/commentViewModel/commentViewModel") else {
        // The old `commentRenderer` form, which YouTube no longer sends.
        tracing::debug!(keys = ?keys(t), "skipping comment thread without a view model");
        return None;
    };
    let mut c = comment(vm, entities, video_id)?;
    let replies = &t["replies"]["commentRepliesRenderer"];
    // The newer form puts the token among `subThreads`.
    c.replies = array(replies, "/contents").iter().chain(array(replies, "/subThreads")).find_map(more_token);
    Some(c)
}

/// A comment from its view model and the entities it names.
fn comment(vm: &Value, entities: &HashMap<&str, &Value>, video_id: &str) -> Option<Comment> {
    let entity = str_at(vm, "/commentKey").and_then(|k| entities.get(k)).and_then(|p| p.get("commentEntityPayload"));
    let Some(e) = entity else {
        tracing::debug!("skipping comment without an entity");
        return None;
    };
    let (p, a, t) = (&e["properties"], &e["author"], &e["toolbar"]);
    let Some(name) = str_at(a, "/displayName").map(clean).filter(|n| !n.is_empty()) else {
        tracing::debug!("skipping comment without an author");
        return None;
    };
    let mut author = channel_ref(name, a.pointer("/channelCommand/innertubeCommand/browseEndpoint"));
    if author.id.is_none() {
        author.id = str_at(a, "/channelId").map(str::to_string);
    }
    let published = str_at(p, "/publishedTime").map(clean).filter(|s| !s.is_empty());
    let edited = published.as_deref().is_some_and(|s| s.ends_with(" (edited)"));
    let count = |path: &str| str_at(t, path).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let state = str_at(vm, "/toolbarStateKey").and_then(|k| entities.get(k));
    Some(Comment {
        id: str_at(p, "/commentId").or_else(|| str_at(vm, "/commentId"))?.to_string(),
        author,
        text: description(&p["content"], video_id).unwrap_or_default(),
        published_text: published.map(|s| s.strip_suffix(" (edited)").map_or(s.clone(), str::to_string)),
        edited,
        // " " means no likes, and "" no replies.
        likes_text: count("/likeCountNotliked"),
        reply_count_text: count("/replyCount"),
        replies: None,
        pinned: vm.get("pinnedText").is_some(),
        hearted: state.is_some_and(|s| s["engagementToolbarStateEntityPayload"]["heartState"] == HEARTED),
        by_uploader: a["isCreator"].as_bool() == Some(true),
        verified: a["isVerified"].as_bool() == Some(true) || a["isArtist"].as_bool() == Some(true),
        reply_level: p["replyLevel"].as_u64().and_then(|n| u8::try_from(n).ok()).unwrap_or(0),
    })
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::source::model::{ChannelRef, LinkTarget};

    fn load(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/innertube/{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    fn summary(p: &CommentPage) -> String {
        let or_dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "-".into());
        let short = |t: &Option<String>| t.as_ref().map_or("-".into(), |t| format!("{}…", &t[..12]));
        let mut out = String::new();
        writeln!(out, "count: {}", or_dash(&p.count_text)).unwrap();
        for s in &p.sorts {
            writeln!(out, "sort: {} selected={} {}", s.label, s.selected, short(&Some(s.continuation.clone())))
                .unwrap();
        }
        writeln!(out, "next: {}", short(&p.continuation)).unwrap();
        writeln!(out, "note: {}", or_dash(&p.note)).unwrap();
        for c in &p.comments {
            writeln!(out, "- {} level={}", c.id, c.reply_level).unwrap();
            writeln!(out, "  author: {:?}", c.author).unwrap();
            writeln!(out, "  published: {} edited={}", or_dash(&c.published_text), c.edited).unwrap();
            writeln!(
                out,
                "  likes: {} replies: {} token: {}",
                or_dash(&c.likes_text),
                or_dash(&c.reply_count_text),
                short(&c.replies)
            )
            .unwrap();
            writeln!(
                out,
                "  pinned={} hearted={} by_uploader={} verified={}",
                c.pinned, c.hearted, c.by_uploader, c.verified
            )
            .unwrap();
            writeln!(out, "  text: {:?}", c.text.text).unwrap();
            for l in &c.text.links {
                let slice: String = c.text.text.chars().skip(l.start).take(l.len).collect();
                writeln!(out, "  link: {slice:?} -> {:?}", l.target).unwrap();
            }
        }
        out
    }

    #[test]
    fn fixture_snapshots() {
        for name in ["next_comments", "next_comments_cont1", "next_comment_replies"] {
            insta::assert_snapshot!(name, summary(&page(&load(name), "jNQXAC9IVRw").unwrap()));
        }
    }

    #[test]
    fn finds_the_first_token_in_the_watch_page() {
        for name in ["next_normal", "next_chapters"] {
            let next = load(name);
            assert!(token(&next).is_some(), "{name}");
            assert!(count_text(&next).is_some(), "{name}");
        }
        assert_eq!(count_text(&load("next_normal")).as_deref(), Some("10M"));
        // A live stream has live chat, not comments.
        assert_eq!(token(&load("next_live")), None);
        // Only the panel has it.
        let mut next = load("next_normal");
        next["contents"] = json!({});
        assert!(token(&next).is_some());
    }

    #[test]
    fn reads_why_comments_are_off() {
        let next = json!({"contents": {"twoColumnWatchNextResults": {"results": {"results": {"contents": [
            {"itemSectionRenderer": {"sectionIdentifier": "comment-item-section", "contents": [
                {"messageRenderer": {"text": {"runs": [{"text": "Comments are turned off."}]}}}
            ]}}
        ]}}}}});
        assert_eq!(token(&next), None);
        assert_eq!(message(&next).as_deref(), Some("Comments are turned off."));
        assert_eq!(message(&load("next_live")), None);
    }

    /// A page with one item and the entities of one comment.
    fn one(item: &Value, entity: &Value, state: &Value) -> Value {
        json!({
            "onResponseReceivedEndpoints": [{"appendContinuationItemsAction": {"continuationItems": [item]}}],
            "frameworkUpdates": {"entityBatchUpdate": {"mutations": [
                {"entityKey": "c", "payload": {"commentEntityPayload": entity}},
                {"entityKey": "s", "payload": {"engagementToolbarStateEntityPayload": state}}
            ]}}
        })
    }

    fn entity(published: &str, likes: &str, replies: &str) -> Value {
        json!({
            "properties": {"commentId": "id1", "content": {"content": "Hi"}, "publishedTime": published, "replyLevel": 0},
            "author": {"channelId": "UC1", "displayName": "\u{2068}@someone\u{2069}"},
            "toolbar": {"likeCountNotliked": likes, "replyCount": replies}
        })
    }

    fn view_model() -> Value {
        json!({"commentKey": "c", "toolbarStateKey": "s"})
    }

    fn only(response: &Value) -> Comment {
        let mut p = page(response, "v").unwrap();
        assert_eq!(p.comments.len(), 1, "{p:?}");
        p.comments.remove(0)
    }

    #[test]
    fn reads_a_bare_comment() {
        let c = only(&one(&json!({"commentViewModel": view_model()}), &entity("2 days ago", " ", ""), &json!({})));
        assert_eq!(c.id, "id1");
        assert_eq!(c.author, ChannelRef { id: Some("UC1".into()), name: "@someone".into(), handle: None });
        assert_eq!(c.text.text, "Hi");
        assert_eq!(c.published_text.as_deref(), Some("2 days ago"));
        assert!(!c.edited);
        // " " and "" mean none.
        assert_eq!((c.likes_text, c.reply_count_text), (None, None));
        assert!(!c.pinned && !c.hearted && !c.by_uploader && !c.verified);
        assert_eq!(c.replies, None);
    }

    #[test]
    fn reads_edits_hearts_pins_and_the_replies_token() {
        let mut vm = view_model();
        vm["pinnedText"] = json!("Pinned by @x");
        let thread = json!({"commentThreadRenderer": {
            "commentViewModel": {"commentViewModel": vm},
            "replies": {"commentRepliesRenderer": {"subThreads": [
                {"commentThreadRenderer": {}},
                {"continuationItemRenderer": {"button": {"buttonRenderer": {"command": {"continuationCommand": {"token": "r"}}}}}}
            ]}}
        }});
        let state = json!({"heartState": "TOOLBAR_HEART_STATE_HEARTED"});
        let c = only(&one(&thread, &entity("7 years ago (edited)", "4.8M", "986"), &state));
        assert_eq!(c.published_text.as_deref(), Some("7 years ago"));
        assert!(c.edited);
        assert_eq!(c.likes_text.as_deref(), Some("4.8M"));
        assert_eq!(c.reply_count_text.as_deref(), Some("986"));
        assert!(c.pinned && c.hearted);
        assert_eq!(c.replies.as_deref(), Some("r"));
        let state = json!({"heartState": "TOOLBAR_HEART_STATE_UNHEARTED"});
        assert!(!only(&one(&thread, &entity("now", "1", "1"), &state)).hearted);
    }

    #[test]
    fn skips_comments_without_entities_and_the_old_form() {
        let response = json!({
            "onResponseReceivedCommands": [{"appendContinuationItemsAction": {"continuationItems": [
                {"commentViewModel": {"commentKey": "missing"}},
                {"commentThreadRenderer": {"comment": {"commentRenderer": {"commentId": "old"}}}},
                {"somethingNew": {}},
                {"continuationItemRenderer": {"button": {"buttonRenderer": {"command": {"continuationCommand": {"token": "more"}}}}}}
            ]}}]
        });
        // `onResponseReceivedCommands` works too.
        let p = page(&response, "v").unwrap();
        assert!(p.comments.is_empty());
        assert_eq!(p.continuation.as_deref(), Some("more"));
    }

    #[test]
    fn a_response_without_items_is_an_error() {
        for bad in [json!({}), json!({"onResponseReceivedEndpoints": [{"somethingElse": {}}]})] {
            assert!(matches!(page(&bad, "v"), Err(SourceError::Parse(_))), "{bad}");
        }
        // An empty list is a page with nothing in it.
        let empty =
            json!({"onResponseReceivedEndpoints": [{"appendContinuationItemsAction": {"continuationItems": []}}]});
        assert_eq!(page(&empty, "v").unwrap(), CommentPage::default());
    }

    #[test]
    fn a_link_to_the_same_video_is_a_timestamp() {
        let mut e = entity("now", "1", "");
        e["properties"]["content"] = json!({"content": "at 0:19", "commandRuns": [
            {"startIndex": 3, "length": 4, "onTap": {"innertubeCommand": {"watchEndpoint": {"videoId": "v", "startTimeSeconds": 19}}}}
        ]});
        let c = only(&one(&json!({"commentViewModel": view_model()}), &e, &json!({})));
        assert_eq!(c.text.links[0].target, LinkTarget::Video { id: "v".into(), start_secs: Some(19) });
    }
}
