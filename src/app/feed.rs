//! The subscriptions feed: the newest videos of the subscribed channels.
//!
//! Each channel's videos tab is loaded at most every 30 minutes (DESIGN.md
//! Q15), and kept in the database, so the feed shows at once at start. The
//! videos tab gives only relative dates ("3 days ago"), so each video gets an
//! approximate time: the time the channel was loaded, less that age.

use std::collections::{HashMap, HashSet};

use super::Selection;
use crate::source::model::{ChannelRef, Item, Video, VideoKind};
pub use crate::store::FeedVideo;

/// A channel is loaded again after this many seconds.
pub const MAX_AGE_SECS: i64 = 30 * 60;

/// Which videos the feed shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeedFilter {
    pub shorts: bool,
    pub live: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FeedState {
    /// All videos, newest first.
    videos: Vec<FeedVideo>,
    /// When each channel was loaded, in Unix seconds.
    pub fetched: HashMap<String, i64>,
    /// What the tab shows: the videos of subscribed channels, after the filter.
    pub items: Vec<Item>,
    pub selection: Selection,
    /// Channels still loading.
    pub loading: usize,
    /// The last channel that failed, and why.
    pub error: Option<String>,
    pub filter: FeedFilter,
}

impl FeedState {
    /// The feed from the database.
    pub fn new(videos: Vec<FeedVideo>, fetched: HashMap<String, i64>, filter: FeedFilter) -> Self {
        let mut feed = Self { videos, fetched, filter, ..Self::default() };
        sort(&mut feed.videos);
        feed
    }

    /// Replaces the videos of one channel. `videos` are newest first, as the
    /// channel's videos tab gives them. Returns them with their times, for
    /// the database, or `None` when no video has a date (the yt-dlp
    /// fallback gives none): then the old videos stay.
    ///
    /// Videos without a date before the first dated one are upcoming
    /// premieres and streams, which cannot play yet: they are left out. A
    /// video without a date after a dated one gets the time of that one.
    pub fn set_channel(&mut self, channel: &ChannelRef, fetched_at: i64, videos: Vec<Video>) -> Option<Vec<FeedVideo>> {
        let channel_id = channel.id.clone().unwrap_or_default();
        let mut last = None;
        let mut new = Vec::new();
        for mut video in videos {
            let at = video.published_text.as_deref().and_then(age_secs).map(|age| fetched_at - age);
            let Some(at) = at.or(last) else { continue };
            let at = last.map_or(at, |l: i64| at.min(l));
            last = Some(at);
            // A collaboration names its channels without IDs. The feed knows
            // which channel it came from.
            if !video.channels.iter().any(|c| c.id.is_some()) {
                match video.channels.first_mut() {
                    Some(c) => c.id = Some(channel_id.clone()),
                    None => video.channels.push(channel.clone()),
                }
            }
            new.push(FeedVideo { channel_id: channel_id.clone(), published_at: at, video });
        }
        if new.is_empty() {
            return None;
        }
        self.fetched.insert(channel_id.clone(), fetched_at);
        self.videos.retain(|v| v.channel_id != channel_id);
        self.videos.extend(new.iter().cloned());
        sort(&mut self.videos);
        Some(new)
    }

    /// Forgets a channel, for example after an unsubscribe.
    pub fn forget_channel(&mut self, channel_id: &str) {
        self.fetched.remove(channel_id);
        self.videos.retain(|v| v.channel_id != channel_id);
    }

    /// Builds the list that the tab shows, for the channels in `subscribed`.
    pub fn rebuild(&mut self, subscribed: &[&str], rows: usize) {
        let selected = self.items.get(self.selection.index).map(item_id);
        let filter = self.filter;
        // A collaboration is on the videos tab of each of its channels.
        let mut seen = HashSet::new();
        self.items = self
            .videos
            .iter()
            .filter(|v| subscribed.contains(&v.channel_id.as_str()))
            .filter(|v| shows(&v.video, filter))
            .filter(|v| seen.insert(v.video.id.clone()))
            .map(|v| Item::Video(v.video.clone()))
            .collect();
        // Keep the selection on the same video when new ones come in above it.
        if let Some(i) = selected.and_then(|id| self.items.iter().position(|it| item_id(it) == id)) {
            self.selection.index = i;
        }
        self.selection.clamp(self.items.len(), rows);
    }
}

/// The channels to load: the ones not loaded in the last 30 minutes, or all
/// with `force`. Each channel comes with the time it was loaded, if it was.
pub fn stale(channels: &[(String, Option<i64>)], now: i64, force: bool) -> Vec<String> {
    channels
        .iter()
        .filter(|(_, at)| force || at.is_none_or(|at| now - at >= MAX_AGE_SECS))
        .map(|(id, _)| id.clone())
        .collect()
}

fn item_id(item: &Item) -> String {
    match item {
        Item::Video(v) => v.id.clone(),
        Item::Channel(c) => c.id.clone(),
        Item::Playlist(p) => p.id.clone(),
    }
}

/// Newest first. Videos with the same time keep their order.
fn sort(videos: &mut [FeedVideo]) {
    videos.sort_by_key(|v| std::cmp::Reverse(v.published_at));
}

/// Whether the feed shows a video. Guests cannot play members-only videos.
fn shows(v: &Video, filter: FeedFilter) -> bool {
    !v.members_only
        && match v.kind {
            VideoKind::Short => filter.shorts,
            VideoKind::Live => filter.live,
            VideoKind::Normal | VideoKind::PastStream => true,
        }
}

/// The age in seconds of an English relative date: "20 hours ago",
/// "Streamed 2 days ago", "1 month ago".
pub fn age_secs(text: &str) -> Option<i64> {
    let text = text.trim();
    let text = text.strip_prefix("Streamed ").or_else(|| text.strip_prefix("Premiered ")).unwrap_or(text);
    let mut words = text.strip_suffix(" ago")?.split_whitespace();
    let n: i64 = words.next()?.parse().ok()?;
    let unit = words.next()?.trim_end_matches('s');
    let secs = match unit {
        "second" => 1,
        "minute" => 60,
        "hour" => 3600,
        "day" => 86_400,
        "week" => 7 * 86_400,
        "month" => 30 * 86_400,
        "year" => 365 * 86_400,
        _ => return None,
    };
    Some(n * secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str, ago: Option<&str>) -> Video {
        Video {
            id: id.into(),
            title: format!("Video {id}"),
            kind: VideoKind::Normal,
            channels: Vec::new(),
            duration_text: None,
            duration_secs: None,
            views_text: None,
            published_text: ago.map(Into::into),
            thumbnails: Vec::new(),
            members_only: false,
            badges: Vec::new(),
            description_snippet: None,
        }
    }

    fn chan(id: &str) -> ChannelRef {
        ChannelRef { id: Some(id.into()), name: format!("Channel {id}"), handle: None }
    }

    fn ids(f: &FeedState) -> Vec<String> {
        f.items.iter().map(item_id).collect()
    }

    #[test]
    fn reads_relative_dates() {
        assert_eq!(age_secs("20 hours ago"), Some(72_000));
        assert_eq!(age_secs("1 day ago"), Some(86_400));
        assert_eq!(age_secs("Streamed 2 weeks ago"), Some(14 * 86_400));
        assert_eq!(age_secs("3 months ago"), Some(90 * 86_400));
        assert_eq!(age_secs("1 year ago"), Some(365 * 86_400));
        assert_eq!(age_secs("Scheduled for 10/31/26"), None);
        assert_eq!(age_secs("ages ago"), None);
    }

    #[test]
    fn mixes_channels_by_time() {
        let mut f = FeedState::default();
        f.set_channel(&chan("A"), 1_000_000, vec![video("a1", Some("1 hour ago")), video("a2", Some("3 days ago"))]);
        f.set_channel(&chan("B"), 1_000_000, vec![video("b1", Some("5 hours ago")), video("b2", None)]);
        f.rebuild(&["A", "B"], 10);
        assert_eq!(ids(&f), ["a1", "b1", "b2", "a2"], "a video without a date takes the time of the one before it");
        // Unsubscribed channels do not show.
        f.rebuild(&["B"], 10);
        assert_eq!(ids(&f), ["b1", "b2"]);
        // A new load replaces the channel's videos.
        f.set_channel(&chan("B"), 1_000_100, vec![video("b3", Some("1 minute ago"))]);
        f.rebuild(&["A", "B"], 10);
        assert_eq!(ids(&f), ["b3", "a1", "a2"]);
        f.forget_channel("A");
        f.rebuild(&["A", "B"], 10);
        assert_eq!(ids(&f), ["b3"]);
    }

    #[test]
    fn hides_shorts_live_and_members_only() {
        let mut f = FeedState::default();
        let mut short = video("s", Some("1 hour ago"));
        short.kind = VideoKind::Short;
        let mut live = video("l", Some("1 hour ago"));
        live.kind = VideoKind::Live;
        let mut members = video("m", Some("1 hour ago"));
        members.members_only = true;
        let mut past = video("p", Some("Streamed 1 day ago"));
        past.kind = VideoKind::PastStream;
        f.set_channel(&chan("A"), 1_000_000, vec![short, live, members, past]);
        f.rebuild(&["A"], 10);
        assert_eq!(ids(&f), ["p"]);
        f.filter = FeedFilter { shorts: true, live: true };
        f.rebuild(&["A"], 10);
        assert_eq!(ids(&f), ["s", "l", "p"]);
    }

    #[test]
    fn the_selection_stays_on_its_video() {
        let mut f = FeedState::default();
        f.set_channel(&chan("A"), 1_000_000, vec![video("a1", Some("1 day ago")), video("a2", Some("2 days ago"))]);
        f.rebuild(&["A", "B"], 10);
        f.selection.index = 1;
        f.set_channel(&chan("B"), 1_000_000, vec![video("b1", Some("1 hour ago"))]);
        f.rebuild(&["A", "B"], 10);
        assert_eq!(item_id(&f.items[f.selection.index]), "a2");
    }

    #[test]
    fn loads_only_stale_channels() {
        let channels =
            [("A".to_string(), Some(1000)), ("B".to_string(), Some(1000 - MAX_AGE_SECS)), ("C".to_string(), None)];
        assert_eq!(stale(&channels, 1000, false), ["B", "C"]);
        assert_eq!(stale(&channels, 1000, true), ["A", "B", "C"]);
    }

    #[test]
    fn upcoming_and_undated_videos() {
        let mut f = FeedState::default();
        // A premiere first, with no date: left out.
        let new = f.set_channel(&chan("A"), 1_000_000, vec![video("p", None), video("a1", Some("3 days ago"))]);
        assert_eq!(new.unwrap().len(), 1);
        f.rebuild(&["A"], 10);
        assert_eq!(ids(&f), ["a1"]);
        // No dates at all, as from yt-dlp: the old videos stay.
        assert_eq!(f.set_channel(&chan("A"), 2_000_000, vec![video("x", None)]), None);
        assert_eq!(f.fetched.get("A"), Some(&1_000_000));
        f.rebuild(&["A"], 10);
        assert_eq!(ids(&f), ["a1"]);
    }

    #[test]
    fn collaborations_show_once_and_know_their_channel() {
        let mut f = FeedState::default();
        let mut collab = video("c", Some("1 day ago"));
        collab.channels = vec![ChannelRef { id: None, name: "A and B".into(), handle: None }];
        f.set_channel(&chan("A"), 1_000_000, vec![collab.clone()]);
        f.set_channel(&chan("B"), 1_000_000, vec![collab]);
        f.rebuild(&["A", "B"], 10);
        assert_eq!(ids(&f), ["c"]);
        let Item::Video(v) = &f.items[0] else { panic!() };
        assert!(v.channels[0].id.is_some(), "the keys for the channel work");
    }
}
