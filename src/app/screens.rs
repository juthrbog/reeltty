//! Screens that open on top of the tabs: a channel, a playlist, or a video's details.

use std::cell::Cell;

use super::comments::CommentsState;
use super::list::ItemList;
use crate::source::model::{ChannelInfo, ChannelRef, Item, PlaylistInfo, VideoDetails};

#[derive(Debug, Clone)]
pub enum Screen {
    Channel(ChannelScreen),
    Playlist(PlaylistScreen),
    Video(VideoScreen),
}

impl Screen {
    /// The request that loads this screen. Replies with another number are not for it.
    pub fn request(&self) -> u64 {
        match self {
            Self::Channel(s) => s.request,
            Self::Playlist(s) => s.request,
            Self::Video(s) => s.request,
        }
    }

    pub fn list_mut(&mut self) -> Option<&mut ItemList> {
        match self {
            Self::Channel(s) => Some(&mut s.list),
            Self::Playlist(s) => Some(&mut s.list),
            Self::Video(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelScreen {
    pub request: u64,
    pub id: String,
    /// Known before the page loads, for the title.
    pub name: String,
    pub info: Option<ChannelInfo>,
    pub list: ItemList,
}

#[derive(Debug, Clone)]
pub struct PlaylistScreen {
    pub request: u64,
    pub id: String,
    pub title: String,
    pub info: Option<PlaylistInfo>,
    pub list: ItemList,
}

#[derive(Debug, Clone)]
pub struct VideoScreen {
    pub request: u64,
    pub id: String,
    pub title: String,
    pub details: Option<VideoDetails>,
    pub loading: bool,
    pub error: Option<String>,
    /// The first visible line of the details text.
    pub scroll: u16,
    /// The largest useful `scroll`. Drawing sets it, because only drawing
    /// knows how the text wraps. `u16::MAX` until the first draw.
    pub max_scroll: Cell<u16>,
    pub pane: VideoPane,
    /// Loaded when the comments first show, and kept when the details show again.
    pub comments: Option<Box<CommentsState>>,
}

/// What the video screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoPane {
    #[default]
    Details,
    Comments,
}

/// Something that can be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Channel { id: String, name: String },
    Playlist { id: String, title: String },
    Video { id: String, title: String },
}

impl Target {
    /// What `Open` opens for an item.
    pub fn open(item: &Item) -> Self {
        match item {
            Item::Video(v) => Self::Video { id: v.id.clone(), title: v.title.clone() },
            Item::Channel(c) => Self::Channel { id: c.id.clone(), name: c.name.clone() },
            Item::Playlist(p) => Self::Playlist { id: p.id.clone(), title: p.title.clone() },
        }
    }

    /// The channel of an item, if its ID is known.
    pub fn channel_of(item: &Item) -> Option<Self> {
        match item {
            Item::Video(v) => v.channels.iter().find_map(Self::channel),
            Item::Channel(c) => Some(Self::Channel { id: c.id.clone(), name: c.name.clone() }),
            Item::Playlist(p) => p.channel.as_ref().and_then(Self::channel),
        }
    }

    pub fn channel(c: &ChannelRef) -> Option<Self> {
        Some(Self::Channel { id: c.id.clone()?, name: c.name.clone() })
    }
}
