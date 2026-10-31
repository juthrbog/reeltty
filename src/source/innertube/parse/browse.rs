//! Channel and playlist pages from `browse`.

use serde_json::Value;

use super::items;
use super::text::{
    array, channel_ref, clean, continuation_token, handle, str_at, strip_bidi_controls, text, text_at, thumbnails,
};
use crate::source::model::{ChannelInfo, ChannelPage, ChannelRef, Item, ItemPage, PlaylistInfo, PlaylistPage};
use crate::source::{Result, SourceError};

const TABS: &str = "/contents/twoColumnBrowseResultsRenderer/tabs";
const HEADER: &str = "/header/pageHeaderRenderer/content/pageHeaderViewModel";
const SIDEBAR: &str = "/sidebar/playlistSidebarRenderer/items";

/// The Videos tab of a channel (or another grid tab, such as Live).
///
/// When the response has no grid, the error names the selected tab, so the
/// caller can ask again with the params of the right tab.
pub fn channel_page(response: &Value) -> Result<ChannelPage> {
    let tabs: Vec<&Value> = array(response, TABS)
        .iter()
        .filter_map(|t| t.get("tabRenderer").or_else(|| t.get("expandableTabRenderer")))
        .collect();
    let selected = tabs.iter().copied().find(|t| t["selected"] == true);
    let videos_tab = || {
        tabs.iter().copied().find(|t| {
            str_at(t, "/endpoint/commandMetadata/webCommandMetadata/url").is_some_and(|u| u.ends_with("/videos"))
        })
    };
    let grid = selected
        .and_then(|t| t.pointer("/content/richGridRenderer"))
        .or_else(|| videos_tab()?.pointer("/content/richGridRenderer"))
        .ok_or_else(|| {
            SourceError::Parse(format!("channel response has no video grid (selected tab: {})", tab_name(selected)))
        })?;

    let info = channel_info(response)?;
    let mut videos = item_page(array(grid, "/contents"));
    // Grid lockups have no channel row, so they name no channel.
    let owner = ChannelRef { id: Some(info.id.clone()), name: info.name.clone(), handle: info.handle.clone() };
    for item in &mut videos.items {
        if let Item::Video(v) = item
            && v.channels.is_empty()
        {
            v.channels.push(owner.clone());
        }
    }
    Ok(ChannelPage { info, videos })
}

/// "Home (/@name/featured)", for error messages.
fn tab_name(tab: Option<&Value>) -> String {
    let Some(tab) = tab else { return "none".into() };
    let title = str_at(tab, "/title").unwrap_or("?");
    match str_at(tab, "/endpoint/commandMetadata/webCommandMetadata/url") {
        Some(url) => format!("{title} ({url})"),
        None => title.to_string(),
    }
}

fn channel_info(response: &Value) -> Result<ChannelInfo> {
    let no_metadata = || SourceError::Parse("channel response has no channel metadata".into());
    let m = response.pointer("/metadata/channelMetadataRenderer").ok_or_else(no_metadata)?;
    let id = str_at(m, "/externalId").ok_or_else(no_metadata)?.to_string();
    let header = response.pointer(HEADER).unwrap_or(&Value::Null);

    // The header rows are "@handle", "16.9M subscribers", "7.9K videos", in
    // no fixed order.
    let (mut handle_text, mut subscribers_text, mut videos_text) = (None, None, None);
    for part in array(header, "/metadata/contentMetadataViewModel/metadataRows")
        .iter()
        .flat_map(|row| array(row, "/metadataParts"))
        .filter_map(|part| text_at(part, "/text"))
    {
        if part.starts_with('@') {
            handle_text.get_or_insert(part);
        } else if part.contains("subscriber") {
            subscribers_text.get_or_insert(part);
        } else if is_video_count(&part) {
            videos_text.get_or_insert(part);
        }
    }
    // The vanity URL is "http://www.youtube.com/@name".
    let handle_text = handle_text.or_else(|| str_at(m, "/vanityChannelUrl")?.rsplit('/').next().and_then(handle));

    let mut avatar = thumbnails(m, "/avatar/thumbnails");
    if avatar.is_empty() {
        avatar = thumbnails(header, "/image/decoratedAvatarViewModel/avatar/avatarViewModel/image/sources");
    }
    Ok(ChannelInfo {
        name: text_at(m, "/title")
            .or_else(|| text_at(header, "/title/dynamicTextViewModel/text"))
            .unwrap_or(id.clone()),
        id,
        handle: handle_text,
        subscribers_text,
        videos_text,
        description: str_at(m, "/description")
            .map(strip_bidi_controls)
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        verified: verified(header),
        avatar,
    })
}

/// The check mark (verified) or the note (official artist) after the title.
fn verified(header: &Value) -> bool {
    array(header, "/title/dynamicTextViewModel/text/attachmentRuns")
        .iter()
        .flat_map(|run| array(run, "/element/type/imageType/image/sources"))
        .filter_map(|s| str_at(s, "/clientResource/imageName"))
        .any(|name| name.starts_with("CHECK_CIRCLE") || name == "AUDIO_BADGE")
}

/// "7.9K videos", "1 video".
fn is_video_count(s: &str) -> bool {
    s.ends_with(" videos") || s.ends_with(" video")
}

/// A playlist (`browse` with `VL...`).
///
/// The section list has a second token after the item section. It loads
/// related playlists, not more videos, so it is not read.
pub fn playlist_page(response: &Value) -> Result<PlaylistPage> {
    let sections = array(response, &format!("{TABS}/0/tabRenderer/content/sectionListRenderer/contents"));
    let lists: Vec<&Vec<Value>> =
        sections.iter().filter_map(|s| s.pointer("/itemSectionRenderer/contents")?.as_array()).collect();
    if lists.is_empty() {
        return Err(SourceError::Parse("playlist response has no item section".into()));
    }
    let videos = item_page(lists.into_iter().flatten());
    Ok(PlaylistPage { info: playlist_info(response)?, videos })
}

fn playlist_info(response: &Value) -> Result<PlaylistInfo> {
    let m = &response["metadata"]["playlistMetadataRenderer"];
    let header = response.pointer(HEADER).unwrap_or(&Value::Null);
    let sidebar = array(response, SIDEBAR);
    let primary = sidebar.iter().find_map(|i| i.get("playlistSidebarPrimaryInfoRenderer")).unwrap_or(&Value::Null);
    let secondary = sidebar.iter().find_map(|i| i.get("playlistSidebarSecondaryInfoRenderer")).unwrap_or(&Value::Null);

    let id = str_at(primary, "/navigationEndpoint/watchEndpoint/playlistId")
        .map(str::to_string)
        .or_else(|| list_param(str_at(response, "/microformat/microformatDataRenderer/urlCanonical")?))
        .or_else(|| list_param(str_at(m, "/androidAppindexingLink")?))
        .ok_or_else(|| SourceError::Parse("playlist response has no playlist ID".into()))?;
    let id = id.strip_prefix("VL").map(str::to_string).unwrap_or(id);

    let owner = secondary.pointer("/videoOwner/videoOwnerRenderer/title/runs/0").and_then(|run| {
        let name = str_at(run, "/text").map(clean).filter(|n| !n.is_empty())?;
        Some(channel_ref(name, run.pointer("/navigationEndpoint/browseEndpoint")))
    });

    // The header rows repeat the sidebar stats, and add "by Owner".
    let header_parts: Vec<String> = array(header, "/metadata/contentMetadataViewModel/metadataRows")
        .iter()
        .flat_map(|row| array(row, "/metadataParts"))
        .filter_map(|part| text_at(part, "/text").or_else(|| text_at(part, "/avatarStack/avatarStackViewModel/text")))
        .collect();
    let owner = owner.or_else(|| {
        let name = header_parts.iter().find_map(|p| p.strip_prefix("by "))?;
        Some(ChannelRef { id: None, name: name.to_string(), handle: None })
    });

    let (mut videos_text, mut views_text, mut updated_text) = (None, None, None);
    for stat in array(primary, "/stats").iter().filter_map(text).chain(header_parts) {
        if is_video_count(&stat) {
            videos_text.get_or_insert(stat);
        } else if stat.ends_with(" views") || stat.ends_with(" view") {
            views_text.get_or_insert(stat);
        } else if stat.starts_with("Last updated") || stat.starts_with("Updated") {
            updated_text.get_or_insert(stat);
        }
    }

    let description = str_at(m, "/description")
        .map(strip_bidi_controls)
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .or_else(|| text_at(primary, "/description"));
    let mut thumbs = thumbnails(header, "/heroImage/contentPreviewImageViewModel/image/sources");
    if thumbs.is_empty() {
        thumbs = thumbnails(primary, "/thumbnailRenderer/playlistVideoThumbnailRenderer/thumbnail/thumbnails");
    }
    Ok(PlaylistInfo {
        id,
        title: text_at(m, "/title")
            .or_else(|| text_at(header, "/title/dynamicTextViewModel/text"))
            .or_else(|| text_at(primary, "/title"))
            .unwrap_or_default(),
        channel: owner,
        videos_text,
        views_text,
        updated_text,
        description,
        thumbnails: thumbs,
    })
}

/// The `list` value of a URL such as `.../playlist?list=PL...`.
fn list_param(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("list=")?;
    Some(rest.split('&').next()?.to_string()).filter(|id| !id.is_empty())
}

/// The next page of a channel grid or a playlist.
///
/// A playlist with no more videos can answer with no actions at all; that is
/// an empty last page.
///
/// Channel grid videos have no channel row, and this response does not name
/// the channel anywhere, so their `channels` is empty. The caller knows the
/// channel from the first page (`ChannelPage::info`) and must fill it in.
pub fn continuation(response: &Value) -> Result<ItemPage> {
    let Some(actions) = response.get("onResponseReceivedActions") else {
        if response.get("responseContext").is_some() && response.get("contents").is_none() {
            return Ok(ItemPage::default());
        }
        return Err(SourceError::Parse("browse continuation has no items".into()));
    };
    let appended: Vec<&Value> =
        actions.as_array().into_iter().flatten().filter_map(|a| a.get("appendContinuationItemsAction")).collect();
    if appended.is_empty() {
        return Err(SourceError::Parse("browse continuation has no items".into()));
    }
    Ok(item_page(appended.into_iter().flat_map(|a| array(a, "/continuationItems"))))
}

/// The items of a list, and the token of its continuation item. Grid items
/// are wrapped in `richItemRenderer`; `items` unwraps them.
fn item_page<'a>(list: impl IntoIterator<Item = &'a Value>) -> ItemPage {
    let mut page = ItemPage::default();
    for item in list {
        match continuation_token(item) {
            Some(token) => {
                page.continuation.get_or_insert(token);
            }
            None => page.items.extend(items(item)),
        }
    }
    page
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::source::model::{Video, VideoKind};

    fn load(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/innertube/{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    fn or_dash(s: Option<&str>) -> &str {
        s.unwrap_or("-")
    }

    fn channel(c: &ChannelRef) -> String {
        format!("{} {} {}", c.name, or_dash(c.id.as_deref()), or_dash(c.handle.as_deref()))
    }

    fn video_line(v: &Video) -> String {
        let mut flags: Vec<String> = Vec::new();
        if v.members_only {
            flags.push("members-only".into());
        }
        flags.extend(v.badges.iter().cloned());
        flags.push(format!("{} thumbs", v.thumbnails.len()));
        let channels = if v.channels.is_empty() {
            "-".to_string()
        } else {
            v.channels.iter().map(channel).collect::<Vec<_>>().join(" + ")
        };
        let duration = match (&v.duration_text, v.duration_secs) {
            (Some(t), Some(s)) => format!("{t} ({s}s)"),
            _ => "-".into(),
        };
        format!(
            "{:?} {} | {} | {channels} | {duration} | {} | {} | {}",
            v.kind,
            v.id,
            v.title,
            or_dash(v.views_text.as_deref()),
            or_dash(v.published_text.as_deref()),
            flags.join(", ")
        )
    }

    fn page_summary(out: &mut String, page: &ItemPage) {
        writeln!(out, "continuation: {}", if page.continuation.is_some() { "yes" } else { "no" }).unwrap();
        writeln!(out, "items: {}", page.items.len()).unwrap();
        for item in &page.items {
            match item {
                Item::Video(v) => writeln!(out, "{}", video_line(v)).unwrap(),
                other => writeln!(out, "{other:?}").unwrap(),
            }
        }
    }

    fn channel_summary(page: &ChannelPage) -> String {
        let i = &page.info;
        let mut out = String::new();
        writeln!(out, "id: {}", i.id).unwrap();
        writeln!(out, "name: {}", i.name).unwrap();
        writeln!(out, "handle: {}", or_dash(i.handle.as_deref())).unwrap();
        writeln!(out, "subscribers: {}", or_dash(i.subscribers_text.as_deref())).unwrap();
        writeln!(out, "videos: {}", or_dash(i.videos_text.as_deref())).unwrap();
        writeln!(out, "description: {}", i.description.as_ref().map_or(0, |d| d.chars().count())).unwrap();
        writeln!(out, "verified: {}", i.verified).unwrap();
        writeln!(out, "avatar: {} thumbs", i.avatar.len()).unwrap();
        page_summary(&mut out, &page.videos);
        out
    }

    fn playlist_summary(page: &PlaylistPage) -> String {
        let i = &page.info;
        let mut out = String::new();
        writeln!(out, "id: {}", i.id).unwrap();
        writeln!(out, "title: {}", i.title).unwrap();
        writeln!(out, "channel: {}", i.channel.as_ref().map_or("-".into(), channel)).unwrap();
        writeln!(out, "videos: {}", or_dash(i.videos_text.as_deref())).unwrap();
        writeln!(out, "views: {}", or_dash(i.views_text.as_deref())).unwrap();
        writeln!(out, "updated: {}", or_dash(i.updated_text.as_deref())).unwrap();
        writeln!(out, "description: {}", or_dash(i.description.as_deref())).unwrap();
        writeln!(out, "thumbnails: {}", i.thumbnails.len()).unwrap();
        page_summary(&mut out, &page.videos);
        out
    }

    fn continuation_summary(page: &ItemPage) -> String {
        let mut out = String::new();
        page_summary(&mut out, page);
        out
    }

    #[test]
    fn channel_snapshots() {
        for name in ["browse_channel_videos", "browse_channel_streams_lofigirl"] {
            insta::assert_snapshot!(name, channel_summary(&channel_page(&load(name)).unwrap()));
        }
    }

    #[test]
    fn playlist_snapshots() {
        for name in ["browse_playlist", "browse_playlist_long"] {
            insta::assert_snapshot!(name, playlist_summary(&playlist_page(&load(name)).unwrap()));
        }
    }

    #[test]
    fn continuation_snapshots() {
        for name in ["browse_channel_videos_cont1", "browse_playlist_long_cont1", "browse_playlist_cont1"] {
            insta::assert_snapshot!(name, continuation_summary(&continuation(&load(name)).unwrap()));
        }
    }

    fn find_video<'a>(page: &'a ItemPage, id: &str) -> &'a Video {
        page.items
            .iter()
            .find_map(|i| match i {
                Item::Video(v) if v.id == id => Some(v),
                _ => None,
            })
            .unwrap()
    }

    fn ltt() -> ChannelRef {
        ChannelRef {
            id: Some("UCXuqSBlHAE6Xw-yeJA0Tunw".into()),
            name: "Linus Tech Tips".into(),
            handle: Some("@LinusTechTips".into()),
        }
    }

    #[test]
    fn channel_grid_video_gets_the_browsed_channel() {
        let page = channel_page(&load("browse_channel_videos")).unwrap();
        assert!(page.videos.items.iter().all(|i| matches!(i, Item::Video(v) if !v.channels.is_empty())));
        assert_eq!(find_video(&page.videos, "7JOW7TTOGfI").channels, [ltt()]);
        // A collab row names the channels, so it is kept, without IDs.
        let collab = find_video(&page.videos, "LttGyjX-QxE");
        assert_eq!(
            collab.channels,
            [ChannelRef { id: None, name: "Linus Tech Tips and Zip Tie Tuning".into(), handle: None }]
        );
    }

    #[test]
    fn channel_grid_continuation_names_no_channel() {
        // Nothing in the response names the channel, so the caller fills it in.
        let response = load("browse_channel_videos_cont1");
        assert!(!response.to_string().contains("UCXuqSBlHAE6Xw-yeJA0Tunw"));
        let page = continuation(&response).unwrap();
        assert_eq!(page.items.len(), 30);
        assert!(page.items.iter().all(|i| matches!(i, Item::Video(v) if v.channels.is_empty())));
    }

    #[test]
    fn members_only_grid_item() {
        let page = channel_page(&load("browse_channel_videos")).unwrap();
        let v = find_video(&page.videos, "KxiUncxSnIo");
        assert!(v.members_only);
        assert_eq!(v.kind, VideoKind::Normal);
        assert_eq!(v.views_text, None);
        assert_eq!(v.published_text.as_deref(), Some("1 day ago"));
        assert_eq!(v.channels, [ltt()]);
        let members = page.videos.items.iter().filter(|i| matches!(i, Item::Video(v) if v.members_only)).count();
        assert_eq!(members, 10);
    }

    #[test]
    fn channel_header_is_classified_by_content() {
        let response = json!({
            "metadata": {"channelMetadataRenderer": {"title": "C", "externalId": "UC1", "description": " "}},
            "header": {"pageHeaderRenderer": {"content": {"pageHeaderViewModel": {"metadata": {"contentMetadataViewModel": {"metadataRows": [
                {"metadataParts": [{"text": {"content": "1 video"}}, {"text": {"content": "1 subscriber"}}]},
                {"metadataParts": [{"text": {"content": "\u{2068}@c\u{2069}"}}]}
            ]}}}}}},
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [
                {"tabRenderer": {"endpoint": {"commandMetadata": {"webCommandMetadata": {"url": "/@c/featured"}}}}},
                {"tabRenderer": {
                    "endpoint": {"commandMetadata": {"webCommandMetadata": {"url": "/@c/videos"}}},
                    "content": {"richGridRenderer": {"contents": []}}
                }}
            ]}}
        });
        // No tab is selected, so the Videos tab is used.
        let page = channel_page(&response).unwrap();
        let i = &page.info;
        assert_eq!(i.handle.as_deref(), Some("@c"));
        assert_eq!(i.subscribers_text.as_deref(), Some("1 subscriber"));
        assert_eq!(i.videos_text.as_deref(), Some("1 video"));
        assert_eq!(i.description, None);
        assert!(!i.verified);
        assert_eq!(page.videos, ItemPage::default());
    }

    #[test]
    fn missing_grid_names_the_selected_tab() {
        let response = json!({
            "metadata": {"channelMetadataRenderer": {"title": "C", "externalId": "UC1"}},
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {
                "title": "Home",
                "selected": true,
                "endpoint": {"commandMetadata": {"webCommandMetadata": {"url": "/@c/featured"}}},
                "content": {"sectionListRenderer": {"contents": []}}
            }}]}}
        });
        let err = channel_page(&response).unwrap_err().to_string();
        assert!(err.contains("selected tab: Home (/@c/featured)"), "{err}");
    }

    #[test]
    fn playlist_section_token_is_ignored() {
        // A short playlist: the only token is the section token.
        let page = playlist_page(&load("browse_playlist")).unwrap();
        assert_eq!(page.videos.items.len(), 10);
        assert_eq!(page.videos.continuation, None);

        // A long playlist: the video token is at the end of the item section.
        let response = load("browse_playlist_long");
        let page = playlist_page(&response).unwrap();
        let sections = &response["contents"]["twoColumnBrowseResultsRenderer"]["tabs"][0]["tabRenderer"]["content"]["sectionListRenderer"]
            ["contents"];
        let section_token = continuation_token(&sections[1]).unwrap();
        let video_token =
            continuation_token(sections[0]["itemSectionRenderer"]["contents"].as_array().unwrap().last().unwrap());
        assert_eq!(page.videos.items.len(), 100);
        assert_eq!(page.videos.continuation, video_token);
        assert_ne!(page.videos.continuation.as_deref(), Some(section_token.as_str()));
    }

    #[test]
    fn playlist_owner_without_handle() {
        let page = playlist_page(&load("browse_playlist_long")).unwrap();
        let owner = page.info.channel.unwrap();
        assert_eq!(owner.name, "Music");
        assert_eq!(owner.id.as_deref(), Some("UC-9-kyTW8ZkZNDHQJ6FgpwQ"));
        assert_eq!(owner.handle, None);
        assert_eq!(page.info.description, None);
    }

    #[test]
    fn playlist_header_is_the_fallback() {
        let response = json!({
            "metadata": {"playlistMetadataRenderer": {"title": "P", "androidAppindexingLink": "android-app://x/playlist?list=PLx&y=1"}},
            "header": {"pageHeaderRenderer": {"content": {"pageHeaderViewModel": {"metadata": {"contentMetadataViewModel": {"metadataRows": [
                {"metadataParts": [{"avatarStack": {"avatarStackViewModel": {"text": {"content": "by Someone"}}}}]},
                {"metadataParts": [{"text": {"content": "Playlist"}}, {"text": {"content": "1 video"}}, {"text": {"content": "No views"}}]}
            ]}}}}}},
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"contents": [
                {"itemSectionRenderer": {"contents": []}}
            ]}}}}]}}
        });
        let i = playlist_page(&response).unwrap().info;
        assert_eq!(i.id, "PLx");
        assert_eq!(i.channel, Some(ChannelRef { id: None, name: "Someone".into(), handle: None }));
        assert_eq!(i.videos_text.as_deref(), Some("1 video"));
        assert_eq!(i.views_text.as_deref(), Some("No views"));
        assert_eq!(i.updated_text, None);
    }

    #[test]
    fn empty_continuation() {
        let page = continuation(&load("browse_playlist_cont1")).unwrap();
        assert_eq!(page, ItemPage::default());
    }

    #[test]
    fn continuation_reads_both_token_types() {
        let response = json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {"continuationItems": [
            {"richItemRenderer": {"content": {"videoRenderer": {"videoId": "a"}}}},
            {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "grid"}}}}
        ]}}]});
        let page = continuation(&response).unwrap();
        assert_eq!((page.items.len(), page.continuation.as_deref()), (1, Some("grid")));

        let response = json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {"continuationItems": [
            {"videoRenderer": {"videoId": "a"}},
            {"continuationItemViewModel": {"continuationCommand": {"innertubeCommand": {"continuationCommand": {"token": "list"}}}}}
        ]}}]});
        let page = continuation(&response).unwrap();
        assert_eq!((page.items.len(), page.continuation.as_deref()), (1, Some("list")));
    }

    #[test]
    fn unrecognisable_responses_are_errors() {
        for bad in [json!({}), json!([1, 2]), json!({"onResponseReceivedActions": [{"somethingNew": {}}]})] {
            assert!(matches!(channel_page(&bad), Err(SourceError::Parse(_))), "{bad}");
            assert!(matches!(playlist_page(&bad), Err(SourceError::Parse(_))), "{bad}");
            assert!(matches!(continuation(&bad), Err(SourceError::Parse(_))), "{bad}");
        }
        // First pages are not continuations, and search is not browse.
        assert!(continuation(&load("browse_channel_videos")).is_err());
        assert!(continuation(&load("browse_playlist")).is_err());
        assert!(channel_page(&load("browse_playlist")).is_err());
        assert!(channel_page(&load("search_rust_programming")).is_err());
        assert!(playlist_page(&load("search_rust_programming")).is_err());
        assert!(playlist_page(&load("browse_channel_videos_cont1")).is_err());
    }
}
