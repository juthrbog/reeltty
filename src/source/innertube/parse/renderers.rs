//! One parser for each item renderer. Each gives `None` when a field that
//! the model needs (an ID) is missing.

use serde_json::Value;

use super::text::{
    Kind, array, channel_ref, classify, clean, collab_channels, duration_secs, handle, str_at, text, text_at,
    thumbnails,
};
use crate::source::model::{Channel, ChannelRef, Item, Playlist, PlaylistKind, Video, VideoKind};

const TAP: &str = "/rendererContext/commandContext/onTap/innertubeCommand";

fn video_kind(short: bool, live: bool, views: Option<&str>, published: Option<&str>) -> VideoKind {
    if short {
        VideoKind::Short
    } else if live || views.is_some_and(|v| classify(v) == Kind::Watching) {
        VideoKind::Live
    } else if published.is_some_and(|p| p.starts_with("Streamed")) {
        VideoKind::PastStream
    } else {
        VideoKind::Normal
    }
}

/// `videoRenderer`, the search result video.
pub fn video_renderer(v: &Value) -> Option<Video> {
    let id = str_at(v, "/videoId")?.to_string();
    let channels = v.pointer("/longBylineText/runs/0").or_else(|| v.pointer("/ownerText/runs/0")).map(byline_channels);

    let overlays = array(v, "/thumbnailOverlays");
    let overlay_time = overlays.iter().find_map(|o| text_at(o, "/thumbnailOverlayTimeStatusRenderer/text"));
    let short = v.pointer("/navigationEndpoint/reelWatchEndpoint").is_some()
        || overlays.iter().any(|o| str_at(o, "/thumbnailOverlayTimeStatusRenderer/style") == Some("SHORTS"));
    let duration_text = text_at(v, "/lengthText").or(overlay_time).filter(|t| duration_secs(t).is_some());

    let (mut live, mut members_only, mut badges) = (false, false, Vec::new());
    for badge in array(v, "/badges") {
        let badge = &badge["metadataBadgeRenderer"];
        let label = str_at(badge, "/label").map(clean).filter(|l| !l.is_empty());
        match badge["style"].as_str() {
            Some("BADGE_STYLE_TYPE_LIVE_NOW") => live = true,
            Some("BADGE_STYLE_TYPE_MEMBERS_ONLY") => members_only = true,
            _ if label.as_deref() == Some("Members only") => members_only = true,
            _ => badges.extend(label),
        }
    }

    let views_text = text_at(v, "/viewCountText");
    let published_text = text_at(v, "/publishedTimeText");
    Some(Video {
        id,
        title: text_at(v, "/title").unwrap_or_default(),
        kind: video_kind(short, live, views_text.as_deref(), published_text.as_deref()),
        channels: channels.unwrap_or_default(),
        duration_secs: duration_text.as_deref().and_then(duration_secs),
        duration_text,
        views_text,
        published_text,
        thumbnails: thumbnails(v, "/thumbnail/thumbnails"),
        members_only,
        badges,
        description_snippet: text_at(v, "/detailedMetadataSnippets/0/snippetText")
            .or_else(|| text_at(v, "/descriptionSnippet")),
    })
}

/// The channels of a byline run: one channel, or the collaborators when the
/// run opens a dialog.
fn byline_channels(run: &Value) -> Vec<ChannelRef> {
    let endpoint = &run["navigationEndpoint"];
    let name = str_at(run, "/text").map(clean).filter(|n| !n.is_empty());
    if let Some(browse) = endpoint.get("browseEndpoint") {
        return name.map(|n| channel_ref(n, Some(browse))).into_iter().collect();
    }
    let collab = collab_channels(endpoint);
    if !collab.is_empty() {
        return collab;
    }
    name.map(|n| ChannelRef { id: None, name: n, handle: None }).into_iter().collect()
}

/// `lockupViewModel`: a video, a Short, a playlist, or a course.
pub fn lockup(l: &Value) -> Option<Item> {
    match l["contentType"].as_str() {
        Some("LOCKUP_CONTENT_TYPE_VIDEO") => lockup_video(l, false).map(Item::Video),
        Some("LOCKUP_CONTENT_TYPE_SHORT") => lockup_video(l, true).map(Item::Video),
        Some("LOCKUP_CONTENT_TYPE_PLAYLIST") => lockup_playlist(l, PlaylistKind::Playlist).map(Item::Playlist),
        Some("LOCKUP_CONTENT_TYPE_COURSE") => lockup_playlist(l, PlaylistKind::Course).map(Item::Playlist),
        other => {
            tracing::debug!(content_type = other, "skipping lockup");
            None
        }
    }
}

/// What the metadata rows of a lockup say. Rows have no fixed layout, so each
/// part is classified by its text.
#[derive(Default)]
struct Rows<'a> {
    views: Option<String>,
    watching: bool,
    published: Option<String>,
    /// The first unclassified text in row 0, with its `browseEndpoint`.
    byline: Option<(String, Option<&'a Value>)>,
    members_only: bool,
    badges: Vec<String>,
}

fn rows(m: &Value) -> Rows<'_> {
    let mut r = Rows::default();
    for (i, row) in array(m, "/metadata/contentMetadataViewModel/metadataRows").iter().enumerate() {
        for part in array(row, "/metadataParts") {
            let content = str_at(part, "/text/content").map(clean).filter(|s| !s.is_empty());
            // Related videos show short text ("909K"); the label is the long form.
            let long = str_at(part, "/accessibilityLabel").map(clean).filter(|s| !s.is_empty()).or(content.clone());
            let Some(long) = long else { continue };
            match classify(&long) {
                Kind::Views => {
                    r.views.get_or_insert(long);
                }
                Kind::Watching => {
                    r.watching = true;
                    r.views.get_or_insert(long);
                }
                Kind::Published => {
                    r.published.get_or_insert(long);
                }
                Kind::Other if i == 0 && r.byline.is_none() => {
                    let browse = part.pointer("/text/commandRuns/0/onTap/innertubeCommand/browseEndpoint");
                    r.byline = content.map(|c| (c, browse));
                }
                Kind::Other => {}
            }
        }
        for badge in array(row, "/badges") {
            let badge = &badge["badgeViewModel"];
            if badge["badgeStyle"] == "BADGE_MEMBERS_ONLY" {
                r.members_only = true;
            } else if let Some(t) = str_at(badge, "/badgeText").map(clean).filter(|t| !t.is_empty()) {
                r.badges.push(t);
            }
        }
    }
    r
}

fn lockup_video(l: &Value, short: bool) -> Option<Video> {
    let id = str_at(l, "/contentId").or_else(|| str_at(l, &format!("{TAP}/watchEndpoint/videoId")))?.to_string();
    let m = &l["metadata"]["lockupMetadataViewModel"];
    let thumb = &l["contentImage"]["thumbnailViewModel"];

    let (mut duration_text, mut live) = (None, false);
    for overlay in array(thumb, "/overlays") {
        for badge in array(overlay, "/thumbnailBottomOverlayViewModel/badges") {
            let badge = &badge["thumbnailBadgeViewModel"];
            match badge["badgeStyle"].as_str() {
                Some("THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE") => live = true,
                Some("THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT") if duration_text.is_none() => {
                    duration_text = str_at(badge, "/text").map(clean).filter(|t| duration_secs(t).is_some());
                }
                _ => {}
            }
        }
    }

    let rows = rows(m);
    let image = &m["image"];
    let collab = image.pointer(&format!("/avatarStackViewModel{TAP}")).map(collab_channels).unwrap_or_default();
    let channels = if collab.is_empty() {
        let avatar = &image["decoratedAvatarViewModel"];
        let avatar_browse = avatar.pointer(&format!("{TAP}/browseEndpoint"));
        // Channel tabs have no channel row. The avatar label still names it.
        let avatar_name = || str_at(avatar, "/a11yLabel")?.strip_prefix("Go to channel ").map(clean);
        let (name, browse) = match rows.byline {
            Some((name, browse)) => (Some(name), browse.or(avatar_browse)),
            None => (avatar_name(), avatar_browse),
        };
        name.map(|n| channel_ref(n, browse)).into_iter().collect()
    } else {
        collab
    };

    Some(Video {
        id,
        title: text_at(m, "/title").unwrap_or_default(),
        kind: video_kind(short, live || rows.watching, rows.views.as_deref(), rows.published.as_deref()),
        channels,
        duration_secs: duration_text.as_deref().and_then(duration_secs),
        duration_text,
        views_text: rows.views,
        published_text: rows.published,
        thumbnails: thumbnails(thumb, "/image/sources"),
        members_only: rows.members_only,
        badges: rows.badges,
        description_snippet: None,
    })
}

fn lockup_playlist(l: &Value, kind: PlaylistKind) -> Option<Playlist> {
    let watch = l.pointer(&format!("{TAP}/watchEndpoint")).unwrap_or(&Value::Null);
    // Home lockups can have short content IDs, so the watch endpoint comes first.
    let id = str_at(watch, "/playlistId").or_else(|| str_at(l, "/contentId"))?;
    let id = id.strip_prefix("VL").unwrap_or(id).to_string();
    let m = &l["metadata"]["lockupMetadataViewModel"];
    let ct = &l["contentImage"]["collectionThumbnailViewModel"]["primaryThumbnail"]["thumbnailViewModel"];

    let count_text = array(ct, "/overlays")
        .iter()
        .flat_map(|o| array(o, "/thumbnailOverlayBadgeViewModel/thumbnailBadges"))
        .find_map(|b| str_at(b, "/thumbnailBadgeViewModel/text"))
        .map(clean);
    let byline = m.pointer("/metadata/contentMetadataViewModel/metadataRows/0/metadataParts/0/text");
    let channel = byline.and_then(|b| {
        let name = text(b)?;
        Some(channel_ref(name, b.pointer("/commandRuns/0/onTap/innertubeCommand/browseEndpoint")))
    });

    Some(Playlist {
        id,
        title: text_at(m, "/title").unwrap_or_default(),
        kind,
        channel,
        count_text,
        thumbnails: thumbnails(ct, "/image/sources"),
        first_video_id: str_at(watch, "/videoId").map(str::to_string),
    })
}

/// `shortsLockupViewModel`. It has no channel, duration, or date.
pub fn shorts_lockup(s: &Value) -> Option<Video> {
    let id = str_at(s, "/onTap/innertubeCommand/reelWatchEndpoint/videoId")
        .or_else(|| str_at(s, "/entityId")?.strip_prefix("shorts-shelf-item-"))?
        .to_string();
    Some(Video {
        id,
        title: text_at(s, "/overlayMetadata/primaryText").unwrap_or_default(),
        kind: VideoKind::Short,
        channels: Vec::new(),
        duration_text: None,
        duration_secs: None,
        views_text: text_at(s, "/overlayMetadata/secondaryText"),
        published_text: None,
        thumbnails: thumbnails(s, "/thumbnailViewModel/thumbnailViewModel/image/sources"),
        members_only: false,
        badges: Vec::new(),
        description_snippet: None,
    })
}

/// `channelRenderer` (search) and `gridChannelRenderer` (featured channels).
///
/// In search results `subscriberCountText` holds the handle and
/// `videoCountText` holds the subscriber count, so the texts are classified
/// by what they say, not by the field name.
pub fn channel_renderer(c: &Value) -> Option<Channel> {
    let id = str_at(c, "/channelId")?.to_string();
    let texts: Vec<String> = ["/subscriberCountText", "/videoCountText"].iter().filter_map(|p| text_at(c, p)).collect();
    let handle = texts
        .iter()
        .find_map(|t| t.starts_with('@').then(|| t.clone()))
        .or_else(|| str_at(c, "/navigationEndpoint/browseEndpoint/canonicalBaseUrl").and_then(handle));
    let verified = array(c, "/ownerBadges")
        .iter()
        .any(|b| str_at(b, "/metadataBadgeRenderer/style").is_some_and(|s| s.starts_with("BADGE_STYLE_TYPE_VERIFIED")));
    Some(Channel {
        name: text_at(c, "/title").unwrap_or_else(|| id.clone()),
        id,
        handle,
        subscribers_text: texts.into_iter().find(|t| t.contains("subscriber")),
        description_snippet: text_at(c, "/descriptionSnippet"),
        verified,
        thumbnails: thumbnails(c, "/thumbnail/thumbnails"),
    })
}
