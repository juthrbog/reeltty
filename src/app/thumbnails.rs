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
    Ready { id: u32, picture: Arc<Picture> },
    Failed,
}

#[derive(Debug, Clone)]
struct Entry {
    state: State,
    /// The value of `clock` when the item was last visible.
    used: u64,
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
            State::Ready { id, picture } => Some((*id, picture.as_ref())),
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
            if let Some(e) = self.entries.get_mut(url) {
                e.used = self.clock;
                continue;
            }
            self.entries.insert(url.to_string(), Entry { state: State::Loading, used: self.clock });
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
                State::Ready { id, picture }
            }
            Err(e) => {
                tracing::debug!("thumbnail {url}: {e}");
                State::Failed
            }
        };
    }

    /// Forgets the thumbnails that were not visible for the longest time.
    /// A thumbnail that is loading stays, so that its reply has a place to go.
    fn evict(&mut self) {
        let over = self.entries.len().saturating_sub(MAX_ENTRIES);
        if over == 0 {
            return;
        }
        let mut old: Vec<(u64, String)> = self
            .entries
            .iter()
            .filter(|(_, e)| !matches!(e.state, State::Loading))
            .map(|(url, e)| (e.used, url.clone()))
            .collect();
        old.sort_unstable();
        for (_, url) in old.into_iter().take(over) {
            self.entries.remove(&url);
        }
    }
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
    fn does_nothing_when_off() {
        let mut t = Thumbnails::new(false);
        assert!(t.want(&[video(1)]).is_empty());
    }
}
