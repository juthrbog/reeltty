//! What the sources return: videos, channels, and playlists in lists.
//!
//! Text fields keep the text that YouTube shows ("1.2M views", "3 years ago"),
//! because exact numbers are often not in the response.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Video(Video),
    Channel(Channel),
    Playlist(Playlist),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRef {
    /// `UC...`. Missing for some collaboration bylines.
    pub id: Option<String>,
    pub name: String,
    /// `@name`, without a leading `/`.
    pub handle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// Always an absolute `https:` URL.
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoKind {
    Normal,
    Short,
    /// Live now.
    Live,
    /// A live stream that ended.
    PastStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub kind: VideoKind,
    /// Empty when unknown (for example Shorts). Two or more for collaborations.
    pub channels: Vec<ChannelRef>,
    /// "13:59:10".
    pub duration_text: Option<String>,
    pub duration_secs: Option<u32>,
    /// "1.2M views", "32 watching".
    pub views_text: Option<String>,
    /// "3 years ago", "Streamed 1 day ago".
    pub published_text: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
    /// Guests cannot play these.
    pub members_only: bool,
    /// "4K", "CC", "New", ...
    pub badges: Vec<String>,
    pub description_snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    /// "111K subscribers".
    pub subscribers_text: Option<String>,
    pub description_snippet: Option<String>,
    pub verified: bool,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistKind {
    Playlist,
    Course,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    /// `PL...`, without the `VL` browse prefix.
    pub id: String,
    pub title: String,
    pub kind: PlaylistKind,
    pub channel: Option<ChannelRef>,
    /// "44 videos", "9 lessons".
    pub count_text: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
    pub first_video_id: Option<String>,
}

/// One page of search results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchPage {
    pub items: Vec<Item>,
    /// The token for the next page. `None` at the end.
    pub continuation: Option<String>,
    pub estimated_results: Option<u64>,
    /// YouTube changed or would change the spelling of the query.
    pub spelling: Option<Spelling>,
    /// A message for the user about this page, for example that a fallback source answered.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spelling {
    /// YouTube searched for `corrected` in place of `original`.
    ShowingResultsFor { corrected: String, original: String },
    /// Auto-correct was off. YouTube suggests `corrected`.
    DidYouMean { corrected: String },
}

/// A page of a list that is not a search: a channel's videos or a playlist.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemPage {
    pub items: Vec<Item>,
    /// The token for the next page. `None` at the end.
    pub continuation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    /// `UC...`.
    pub id: String,
    pub name: String,
    /// `@name`.
    pub handle: Option<String>,
    /// "16.9M subscribers".
    pub subscribers_text: Option<String>,
    /// "7.9K videos".
    pub videos_text: Option<String>,
    pub description: Option<String>,
    pub verified: bool,
    pub avatar: Vec<Thumbnail>,
}

/// The first page of a channel: who it is, and its newest videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPage {
    pub info: ChannelInfo,
    pub videos: ItemPage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistInfo {
    /// `PL...`, without the `VL` browse prefix.
    pub id: String,
    pub title: String,
    pub channel: Option<ChannelRef>,
    /// "13 videos".
    pub videos_text: Option<String>,
    /// "102,310 views".
    pub views_text: Option<String>,
    /// "Last updated on Jun 17, 2014".
    pub updated_text: Option<String>,
    pub description: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
}

/// The first page of a playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistPage {
    pub info: PlaylistInfo,
    pub videos: ItemPage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub title: String,
    pub start_secs: u32,
}

/// A link in a description. `start` and `len` count `char`s in `Description::text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub len: usize,
    pub target: LinkTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// The real URL, not YouTube's redirect.
    Url(String),
    /// A video, or a time in a video (a timestamp links to the same video).
    Video {
        id: String,
        start_secs: Option<u32>,
    },
    Channel {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Description {
    pub text: String,
    pub links: Vec<Link>,
}

/// The details of one video, for the details screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDetails {
    pub id: String,
    pub title: String,
    pub channel: Option<ChannelRef>,
    /// "8.73M subscribers".
    pub subscribers_text: Option<String>,
    /// "25,037,755 views", "10,132 watching now".
    pub views_text: Option<String>,
    /// "Oct 5, 2017", "Started streaming on Aug 19, 2026".
    pub date_text: Option<String>,
    /// ISO 8601, for example "2017-10-05T08:11:25-07:00".
    pub publish_date: Option<String>,
    /// `None` for a live stream.
    pub duration_secs: Option<u32>,
    /// "568K".
    pub likes_text: Option<String>,
    pub live: bool,
    pub description: Description,
    /// In order of start time. Empty when the video has none.
    pub chapters: Vec<Chapter>,
    pub thumbnails: Vec<Thumbnail>,
    /// The videos that YouTube shows next to this one, without Shorts.
    pub related: Vec<Video>,
}
