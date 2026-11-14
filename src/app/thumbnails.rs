//! The thumbnails that the app wants, has, or could not get.
//!
//! The app asks only for the thumbnails of the visible items and of the next
//! screen of items. It keeps a limited number in memory. Each loaded
//! thumbnail gets a new image ID, which the frontend uses to send it to the
//! terminal and the UI uses to draw it.

use std::collections::HashMap;
use std::sync::Arc;

use super::Effect;
use crate::source::model::Item;
use crate::terminal::kitty::MAX_ID;
use crate::thumbnails::{Picture, Slot, pick};

/// List items are this many rows high when thumbnails are on.
pub const THUMBNAIL_ROWS: u16 = 3;
/// Thumbnails kept in memory. Each one is some tens of KB.
const MAX_ENTRIES: usize = 300;
/// The first image ID. Lower IDs are free for other uses, such as video.
const FIRST_ID: u32 = 256;
/// Most terminals report no pixel size in tests and some setups.
const DEFAULT_CELL: (u16, u16) = (10, 20);

#[derive(Debug, Clone)]
pub enum State {
    Loading,
    /// `picture` is what shows: `base`, with how much of the video was
    /// watched (`watched`, in thousandths) drawn on it. `color` is the main
    /// color of `base`, if it has one.
    Ready {
        id: u32,
        picture: Arc<Picture>,
        base: Arc<Picture>,
        watched: Option<u16>,
        color: Option<(u8, u8, u8)>,
    },
    Failed,
}

#[derive(Debug, Clone)]
struct Entry {
    state: State,
    /// The value of `clock` when the item was last visible.
    used: u64,
    /// Only videos asked for this thumbnail. A playlist or a mix can use the
    /// picture of its first video, and must not show that video's watched
    /// part.
    video: bool,
}

#[derive(Debug, Clone)]
pub struct Thumbnails {
    /// The configuration file allows them and the terminal can show them.
    pub enabled: bool,
    slot: Slot,
    entries: HashMap<String, Entry>,
    next_id: u32,
    clock: u64,
}

impl Thumbnails {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            slot: Slot::new(THUMBNAIL_ROWS, DEFAULT_CELL),
            entries: HashMap::new(),
            next_id: FIRST_ID,
            clock: 0,
        }
    }

    /// Where each thumbnail goes on screen.
    pub fn slot(&self) -> Slot {
        self.slot
    }

    /// Sets the pixel size of one cell. If it changed, all thumbnails load again at the new size.
    pub fn set_cell(&mut self, width: u16, height: u16) {
        if width == 0 || height == 0 {
            return;
        }
        let slot = Slot::new(THUMBNAIL_ROWS, (width, height));
        if slot != self.slot {
            self.slot = slot;
            self.entries.clear();
        }
    }

    /// The URL of the thumbnail for `item` at the current size.
    pub fn url<'a>(&self, item: &'a Item) -> Option<&'a str> {
        let thumbnails = match item {
            Item::Video(v) => &v.thumbnails,
            Item::Channel(c) => &c.thumbnails,
            Item::Playlist(p) => &p.thumbnails,
        };
        pick(thumbnails, self.slot.width)
    }

    /// The image ID of the thumbnail for `item`, if it is loaded.
    pub fn image(&self, item: &Item) -> Option<u32> {
        match self.entries.get(self.url(item)?)?.state {
            State::Ready { id, .. } => Some(id),
            State::Loading | State::Failed => None,
        }
    }

    /// The loaded thumbnails, for the frontend.
    pub fn ready(&self) -> impl Iterator<Item = (u32, &Picture)> {
        self.entries.values().filter_map(|e| match &e.state {
            State::Ready { id, picture, .. } => Some((*id, picture.as_ref())),
            State::Loading | State::Failed => None,
        })
    }

    /// Asks for the thumbnails of `visible` that are not loaded or loading.
    pub fn want<'a>(&mut self, visible: impl IntoIterator<Item = &'a Item>) -> Vec<Effect> {
        if !self.enabled {
            return Vec::new();
        }
        self.clock += 1;
        let mut effects = Vec::new();
        for item in visible {
            let Some(url) = self.url(item) else { continue };
            let video = matches!(item, Item::Video(_));
            if let Some(e) = self.entries.get_mut(url) {
                e.used = self.clock;
                if e.video && !video {
                    e.video = false;
                    // The plain picture, under a new ID, if it showed a watched part.
                    if let State::Ready { id, picture, base, watched: shown @ Some(_), .. } = &mut e.state {
                        *picture = Arc::clone(base);
                        *shown = None;
                        *id = self.next_id;
                        self.next_id = if *id >= MAX_ID { FIRST_ID } else { *id + 1 };
                    }
                }
                continue;
            }
            self.entries.insert(url.to_string(), Entry { state: State::Loading, used: self.clock, video });
            effects.push(Effect::LoadThumbnail { url: url.to_string(), slot: self.slot });
        }
        self.evict();
        effects
    }

    /// The reply to `Effect::LoadThumbnail`.
    pub fn loaded(&mut self, url: &str, slot: Slot, result: Result<Arc<Picture>, String>) {
        // A reply for an old size is no use. The thumbnail loads again when it is visible.
        if slot != self.slot {
            return;
        }
        let Some(entry) = self.entries.get_mut(url) else { return };
        if !matches!(entry.state, State::Loading) {
            return;
        }
        entry.state = match result {
            Ok(picture) => {
                let id = self.next_id;
                self.next_id = if id >= MAX_ID { FIRST_ID } else { id + 1 };
                let color = crate::thumbnails::main_color(&picture.rgba);
                State::Ready { id, base: Arc::clone(&picture), picture, watched: None, color }
            }
            Err(e) => {
                tracing::debug!("thumbnail {url}: {e}");
                State::Failed
            }
        };
    }

    /// The main color of a loaded thumbnail of the video `id`.
    pub fn video_color(&self, id: &str) -> Option<(u8, u8, u8)> {
        self.entries.iter().find_map(|(url, e)| match e.state {
            State::Ready { color, .. } if video_id(url) == Some(id) => color,
            _ => None,
        })
    }

    /// Draws on each loaded video thumbnail how much of the video was
    /// watched, in thousandths, from `watched`. A thumbnail whose value
    /// changed gets a new image ID, so that the frontend sends it again.
    pub fn show_watched(&mut self, watched: impl Fn(&str) -> Option<u16>) {
        for (url, entry) in &mut self.entries {
            let State::Ready { id, picture, base, watched: shown, .. } = &mut entry.state else { continue };
            let now = if entry.video { video_id(url).and_then(&watched) } else { None };
            if now == *shown {
                continue;
            }
            *picture = match now {
                Some(permille) => Arc::new(crate::thumbnails::with_watched(base, permille)),
                None => Arc::clone(base),
            };
            *shown = now;
            *id = self.next_id;
            self.next_id = if *id >= MAX_ID { FIRST_ID } else { *id + 1 };
        }
    }

    /// Lets the failed thumbnails load again, for example after a network error.
    pub fn forget_failed(&mut self) {
        self.entries.retain(|_, e| !matches!(e.state, State::Failed));
    }

    /// Forgets the thumbnails that were not visible for the longest time.
    /// Two kinds always stay: the ones the last `want` asked for (they are on
    /// screen or come next), and the ones that are loading, so that their
    /// replies have a place to go. So the count can go above `MAX_ENTRIES`
    /// for a while.
    fn evict(&mut self) {
        let over = self.entries.len().saturating_sub(MAX_ENTRIES);
        if over == 0 {
            return;
        }
        let mut old: Vec<(u64, String)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.used < self.clock && !matches!(e.state, State::Loading))
            .map(|(url, e)| (e.used, url.clone()))
            .collect();
        old.sort_unstable();
        for (_, url) in old.into_iter().take(over) {
            self.entries.remove(&url);
        }
    }
}

/// The video ID in a YouTube thumbnail URL, such as
/// `https://i.ytimg.com/vi/<id>/hqdefault.jpg`.
fn video_id(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("/vi/").or_else(|| url.split_once("/vi_webp/"))?;
    rest.split_once('/').map(|(id, _)| id).filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::model::{Thumbnail, Video, VideoKind};

    fn video(n: usize) -> Item {
        Item::Video(Video {
            id: format!("v{n}"),
            title: String::new(),
            kind: VideoKind::Normal,
            channels: Vec::new(),
            duration_text: None,
            duration_secs: None,
            views_text: None,
            published_text: None,
            thumbnails: vec![Thumbnail { url: format!("https://i.ytimg.com/vi/v{n}/a.jpg"), width: 336, height: 188 }],
            members_only: false,
            badges: Vec::new(),
            description_snippet: None,
        })
    }

    fn picture() -> Arc<Picture> {
        Arc::new(Picture { width: 1, height: 1, cols: 1, rows: 1, rgba: vec![0; 4] })
    }

    #[test]
    fn asks_once_and_gives_new_ids() {
        let mut t = Thumbnails::new(true);
        let items = [video(1), video(2)];
        let effects = t.want(&items);
        assert_eq!(effects.len(), 2);
        assert!(t.want(&items).is_empty(), "loading thumbnails are not asked for again");
        let slot = t.slot();
        t.loaded("https://i.ytimg.com/vi/v1/a.jpg", slot, Ok(picture()));
        t.loaded("https://i.ytimg.com/vi/v2/a.jpg", slot, Err("404".into()));
        assert_eq!(t.image(&items[0]), Some(FIRST_ID));
        assert_eq!(t.image(&items[1]), None);
        assert_eq!(t.ready().count(), 1);
        assert!(t.want(&items).is_empty(), "failed thumbnails are not asked for again");
    }

    #[test]
    fn finds_video_ids_in_urls() {
        assert_eq!(video_id("https://i.ytimg.com/vi/abc_-12/hqdefault.jpg?sqp=x"), Some("abc_-12"));
        assert_eq!(video_id("https://i.ytimg.com/vi_webp/abc/mqdefault.webp"), Some("abc"));
        assert_eq!(video_id("https://yt3.ggpht.com/abc=s88"), None);
        assert_eq!(video_id("https://i.ytimg.com/vi//a.jpg"), None);
        assert_eq!(video_id("https://i.ytimg.com/vi/abc"), None);
    }

    #[test]
    fn watched_thumbnails_get_a_bar_and_a_new_id() {
        let mut t = Thumbnails::new(true);
        let items = [video(1), video(2)];
        t.want(&items);
        let slot = t.slot();
        let grey = Arc::new(Picture { width: 4, height: 4, cols: 1, rows: 1, rgba: [90, 90, 90, 255].repeat(16) });
        t.loaded("https://i.ytimg.com/vi/v1/a.jpg", slot, Ok(Arc::clone(&grey)));
        t.loaded("https://i.ytimg.com/vi/v2/a.jpg", slot, Ok(Arc::clone(&grey)));
        let (first, second) = (t.image(&items[0]).unwrap(), t.image(&items[1]).unwrap());
        let shown = |t: &Thumbnails, id: u32| t.ready().find(|(i, _)| *i == id).unwrap().1.clone();
        // v1 was watched to the end. v2 was not watched.
        let watched = |id: &str| (id == "v1").then_some(1000);
        t.show_watched(watched);
        let new = t.image(&items[0]).unwrap();
        assert_ne!(new, first, "a new image, so the terminal gets it again");
        assert_eq!(shown(&t, new).rgba[60..64], [255, 0, 0, 255], "the bottom row is red");
        assert_eq!(shown(&t, new).rgba[0..4], [90, 90, 90, 255], "the top row is not");
        assert_eq!(t.image(&items[1]), Some(second), "the other one stays");
        assert_eq!(shown(&t, second), *grey);
        // The same values again change nothing.
        t.show_watched(watched);
        assert_eq!(t.image(&items[0]), Some(new));
        // Out of the history: the plain picture, with another new ID.
        t.show_watched(|_| None);
        let plain = t.image(&items[0]).unwrap();
        assert_ne!(plain, new);
        assert_eq!(shown(&t, plain), *grey);
    }

    #[test]
    fn playlists_with_a_watched_first_video_get_no_bar() {
        use crate::source::model::Playlist;
        let mut t = Thumbnails::new(true);
        let Item::Video(v) = video(1) else { unreachable!() };
        // A playlist that uses the picture of its first video, v1.
        let playlist = Item::Playlist(Playlist {
            id: "PL1".into(),
            title: String::new(),
            kind: crate::source::model::PlaylistKind::Playlist,
            channel: None,
            count_text: None,
            thumbnails: v.thumbnails.clone(),
            first_video_id: None,
        });
        t.want([&playlist]);
        let slot = t.slot();
        let grey = Arc::new(Picture { width: 4, height: 4, cols: 1, rows: 1, rgba: [90, 90, 90, 255].repeat(16) });
        t.loaded("https://i.ytimg.com/vi/v1/a.jpg", slot, Ok(Arc::clone(&grey)));
        let id = t.image(&playlist).unwrap();
        t.show_watched(|_| Some(1000));
        assert_eq!(t.image(&playlist), Some(id), "no bar on a playlist");
        // A video alone gets the bar. When a playlist with the same picture
        // shows too, the bar goes.
        let mut t = Thumbnails::new(true);
        let v1 = Item::Video(v);
        t.want([&v1]);
        t.loaded("https://i.ytimg.com/vi/v1/a.jpg", slot, Ok(Arc::clone(&grey)));
        t.show_watched(|_| Some(1000));
        let barred = t.image(&v1).unwrap();
        assert_ne!(t.ready().find(|(i, _)| *i == barred).unwrap().1, &*grey);
        t.want([&v1, &playlist]);
        let plain = t.image(&v1).unwrap();
        assert_ne!(plain, barred);
        assert_eq!(t.ready().find(|(i, _)| *i == plain).unwrap().1, &*grey);
        t.show_watched(|_| Some(1000));
        assert_eq!(t.image(&v1), Some(plain), "it stays plain");
    }

    #[test]
    fn ignores_replies_for_an_old_size() {
        let mut t = Thumbnails::new(true);
        let items = [video(1)];
        t.want(&items);
        let old = t.slot();
        t.set_cell(8, 16);
        t.loaded("https://i.ytimg.com/vi/v1/a.jpg", old, Ok(picture()));
        assert_eq!(t.image(&items[0]), None);
        assert_eq!(t.want(&items).len(), 1, "the new size loads again");
        t.set_cell(8, 16);
        assert!(t.want(&items).is_empty(), "the same size keeps the entries");
    }

    #[test]
    fn evicts_the_least_recently_visible() {
        let mut t = Thumbnails::new(true);
        let items: Vec<Item> = (0..MAX_ENTRIES + 10).map(video).collect();
        let slot = t.slot();
        for item in &items {
            t.want([item]);
            t.loaded(t.url(item).unwrap(), slot, Ok(picture()));
        }
        assert_eq!(t.entries.len(), MAX_ENTRIES);
        assert_eq!(t.image(&items[0]), None);
        assert!(t.image(&items[MAX_ENTRIES + 9]).is_some());
    }

    #[test]
    fn never_evicts_wanted_thumbnails() {
        let mut t = Thumbnails::new(true);
        let slot = t.slot();
        let visible: Vec<Item> = (0..20).map(video).collect();
        t.want(&visible);
        for item in &visible {
            t.loaded(t.url(item).unwrap(), slot, Ok(picture()));
        }
        // Many slow downloads for items that were scrolled past, then back
        // to the first items. Their thumbnails may load once more.
        let passed: Vec<Item> = (100..100 + MAX_ENTRIES).map(video).collect();
        t.want(&passed);
        for e in t.want(&visible) {
            let Effect::LoadThumbnail { url, .. } = e else { panic!("{e:?}") };
            t.loaded(&url, slot, Ok(picture()));
        }
        // The downloads are still pending, but the visible thumbnails stay
        // and are not asked for again.
        for _ in 0..3 {
            assert!(t.want(&visible).is_empty(), "the visible thumbnails stay");
        }
        assert!(visible.iter().all(|i| t.image(i).is_some()));
    }

    #[test]
    fn failed_thumbnails_load_again_after_forget_failed() {
        let mut t = Thumbnails::new(true);
        let items = [video(1)];
        t.want(&items);
        t.loaded(t.url(&items[0]).unwrap(), t.slot(), Err("timeout".into()));
        assert!(t.want(&items).is_empty());
        t.forget_failed();
        assert_eq!(t.want(&items).len(), 1);
    }

    #[test]
    fn does_nothing_when_off() {
        let mut t = Thumbnails::new(false);
        assert!(t.want(&[video(1)]).is_empty());
    }
}
