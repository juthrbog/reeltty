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
