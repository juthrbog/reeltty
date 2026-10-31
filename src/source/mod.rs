//! Where metadata comes from. The `Source` trait hides it.
//!
//! - `innertube`: the main source, the private API that the YouTube website uses.
//! - `ytdlp`: the fallback. It is slower, but yt-dlp gets fixes fastest.
//!
//! `Fallback` tries one source, then the other.

pub mod filters;
pub mod innertube;
pub mod model;
pub mod streams;
pub mod ytdlp;

use futures::future::BoxFuture;

pub use filters::SearchQuery;
pub use model::{ChannelPage, CommentPage, ItemPage, PlaylistPage, SearchPage, VideoDetails};

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("network: {0}")]
    Network(String),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("unexpected response: {0}")]
    Parse(String),
    /// The source cannot do this at all, for example the next page in yt-dlp.
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Program(String),
}

pub type Result<T> = std::result::Result<T, SourceError>;

pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;

    /// The first page of results.
    fn search<'a>(&'a self, query: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>>;

    /// The page after the one that returned `continuation`.
    fn search_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<SearchPage>>;

    /// A channel and the first page of its videos. `id` is `UC...`.
    fn channel<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ChannelPage>> {
        let _ = id;
        Box::pin(async { Err(SourceError::Unsupported(format!("{} cannot open channels", self.name()))) })
    }

    /// A playlist and its first page of videos. `id` is `PL...`, without `VL`.
    fn playlist<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<PlaylistPage>> {
        let _ = id;
        Box::pin(async { Err(SourceError::Unsupported(format!("{} cannot open playlists", self.name()))) })
    }

    /// The next page of a channel's videos or of a playlist.
    fn browse_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<ItemPage>> {
        let _ = continuation;
        Box::pin(async { Err(SourceError::Unsupported(format!("{} cannot load more", self.name()))) })
    }

    /// The details of one video.
    fn video<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VideoDetails>> {
        let _ = id;
        Box::pin(async { Err(SourceError::Unsupported(format!("{} cannot show video details", self.name()))) })
    }

    /// The first page of a video's comments, in YouTube's "Top" order. A
    /// video with no comments gives a page with only a `note`.
    fn comments<'a>(&'a self, video_id: &'a str) -> BoxFuture<'a, Result<CommentPage>> {
        let _ = video_id;
        Box::pin(async { Err(SourceError::Unsupported(format!("{} cannot show comments", self.name()))) })
    }

    /// The page for any comments token: the next page, another order,
    /// replies, or more replies. `video_id` is the video of the comments.
    fn comments_more<'a>(&'a self, video_id: &'a str, continuation: &'a str) -> BoxFuture<'a, Result<CommentPage>> {
        let _ = (video_id, continuation);
        Box::pin(async { Err(SourceError::Unsupported(format!("{} cannot show comments", self.name()))) })
    }
}

/// Uses `primary`. If it fails, uses `secondary` and says so in `SearchPage::note`.
pub struct Fallback<A, B> {
    pub primary: A,
    pub secondary: B,
}

impl<A: Source, B: Source> Source for Fallback<A, B> {
    fn name(&self) -> &'static str {
        self.primary.name()
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>> {
        Box::pin(async move {
            match self.primary.search(query).await {
                Ok(page) => Ok(page),
                Err(first) => {
                    tracing::warn!("{} search failed: {first}", self.primary.name());
                    let mut page = self.secondary.search(query).await.map_err(|second| {
                        tracing::warn!("{} search failed: {second}", self.secondary.name());
                        // The secondary's error already names the program.
                        SourceError::Program(format!("{} failed: {first}. {second}", self.primary.name()))
                    })?;
                    let why = format!(
                        "Results from {}, because {} failed: {first}",
                        self.secondary.name(),
                        self.primary.name()
                    );
                    page.note = Some(match page.note.take() {
                        Some(n) => format!("{why}. {n}"),
                        None => why,
                    });
                    Ok(page)
                }
            }
        })
    }

    fn search_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<SearchPage>> {
        // A continuation token always comes from the primary source, because
        // the fallback gives none.
        self.primary.search_more(continuation)
    }

    fn channel<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ChannelPage>> {
        Box::pin(self.either("channel", move |s| s.channel(id)))
    }

    fn playlist<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<PlaylistPage>> {
        Box::pin(self.either("playlist", move |s| s.playlist(id)))
    }

    fn browse_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<ItemPage>> {
        // As with search, only the primary source gives continuation tokens.
        self.primary.browse_more(continuation)
    }

    fn video<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VideoDetails>> {
        Box::pin(self.either("video", move |s| s.video(id)))
    }

    fn comments<'a>(&'a self, video_id: &'a str) -> BoxFuture<'a, Result<CommentPage>> {
        // yt-dlp reads comments only in a slow full extraction.
        self.primary.comments(video_id)
    }

    fn comments_more<'a>(&'a self, video_id: &'a str, continuation: &'a str) -> BoxFuture<'a, Result<CommentPage>> {
        self.primary.comments_more(video_id, continuation)
    }
}

impl<A: Source, B: Source> Fallback<A, B> {
    /// Runs `call` on the primary source, and on the secondary if that fails.
    /// `what` names the call in the log.
    async fn either<'a, T>(
        &'a self,
        what: &str,
        call: impl Fn(&'a dyn Source) -> BoxFuture<'a, Result<T>>,
    ) -> Result<T> {
        match call(&self.primary).await {
            Ok(value) => Ok(value),
            Err(first) => {
                tracing::warn!("{} {what} failed: {first}", self.primary.name());
                call(&self.secondary).await.map_err(|second| {
                    tracing::warn!("{} {what} failed: {second}", self.secondary.name());
                    SourceError::Program(format!("{} failed: {first}. {second}", self.primary.name()))
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::filters::SearchFilters;
    use crate::source::model::{ChannelInfo, Description, PlaylistInfo};

    struct Fixed(&'static str, bool, Option<&'static str>);

    impl Source for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn search<'a>(&'a self, _: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>> {
            Box::pin(async move {
                if self.1 {
                    Ok(SearchPage { note: self.2.map(str::to_string), ..SearchPage::default() })
                } else {
                    Err(SourceError::Network("down".into()))
                }
            })
        }
        fn search_more<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<SearchPage>> {
            Box::pin(async { Err(SourceError::Unsupported("no".into())) })
        }
        fn channel<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ChannelPage>> {
            let info = ChannelInfo {
                id: id.into(),
                name: self.0.into(),
                handle: None,
                subscribers_text: None,
                videos_text: None,
                description: None,
                verified: false,
                avatar: Vec::new(),
            };
            self.answer(ChannelPage { info, videos: ItemPage::default() })
        }
        fn playlist<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<PlaylistPage>> {
            let info = PlaylistInfo {
                id: id.into(),
                title: self.0.into(),
                channel: None,
                videos_text: None,
                views_text: None,
                updated_text: None,
                description: None,
                thumbnails: Vec::new(),
            };
            self.answer(PlaylistPage { info, videos: ItemPage::default() })
        }
        fn browse_more<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<ItemPage>> {
            self.answer(ItemPage { items: Vec::new(), continuation: Some(self.0.into()) })
        }
        fn comments<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<CommentPage>> {
            self.answer(CommentPage { note: Some(self.0.into()), ..CommentPage::default() })
        }
        fn comments_more<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<CommentPage>> {
            self.answer(CommentPage { continuation: Some(self.0.into()), ..CommentPage::default() })
        }
        fn video<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VideoDetails>> {
            self.answer(VideoDetails {
                id: id.into(),
                title: self.0.into(),
                channel: None,
                subscribers_text: None,
                views_text: None,
                date_text: None,
                publish_date: None,
                duration_secs: None,
                likes_text: None,
                live: false,
                description: Description::default(),
                chapters: Vec::new(),
                thumbnails: Vec::new(),
                related: Vec::new(),
                comments: None,
                comments_count_text: None,
            })
        }
    }

    impl Fixed {
        /// `value` if this source works, else a network error.
        fn answer<T: Send + 'static>(&self, value: T) -> BoxFuture<'static, Result<T>> {
            let works = self.1;
            Box::pin(async move { if works { Ok(value) } else { Err(SourceError::Network("down".into())) } })
        }
    }

    fn query() -> SearchQuery {
        SearchQuery { text: "x".into(), filters: SearchFilters::default() }
    }

    #[tokio::test]
    async fn uses_primary_when_it_works() {
        let f = Fallback { primary: Fixed("a", true, None), secondary: Fixed("b", true, None) };
        assert_eq!(f.search(&query()).await.unwrap().note, None);
    }

    #[tokio::test]
    async fn falls_back_with_a_note() {
        let f = Fallback { primary: Fixed("a", false, None), secondary: Fixed("b", true, None) };
        let page = f.search(&query()).await.unwrap();
        assert_eq!(page.note.as_deref(), Some("Results from b, because a failed: network: down"));
        // The secondary's own note is kept.
        let f = Fallback { primary: Fixed("a", false, None), secondary: Fixed("b", true, Some("filters ignored")) };
        let page = f.search(&query()).await.unwrap();
        assert_eq!(page.note.as_deref(), Some("Results from b, because a failed: network: down. filters ignored"));
    }

    #[tokio::test]
    async fn both_failing_is_an_error() {
        let f = Fallback { primary: Fixed("a", false, None), secondary: Fixed("b", false, None) };
        let err = f.search(&query()).await.unwrap_err();
        assert_eq!(err.to_string(), "a failed: network: down. network: down");
    }

    fn pair(a: bool, b: bool) -> Fallback<Fixed, Fixed> {
        Fallback { primary: Fixed("a", a, None), secondary: Fixed("b", b, None) }
    }

    #[tokio::test]
    async fn channel_playlist_and_video_fall_back() {
        for (a, b, from) in [(true, true, "a"), (true, false, "a"), (false, true, "b")] {
            let f = pair(a, b);
            let channel = f.channel("UCx").await.unwrap();
            assert_eq!((channel.info.id.as_str(), channel.info.name.as_str()), ("UCx", from));
            let playlist = f.playlist("PLx").await.unwrap();
            assert_eq!((playlist.info.id.as_str(), playlist.info.title.as_str()), ("PLx", from));
            let video = f.video("v").await.unwrap();
            assert_eq!((video.id.as_str(), video.title.as_str()), ("v", from));
        }
    }

    #[tokio::test]
    async fn channel_playlist_and_video_fail_when_both_fail() {
        let f = pair(false, false);
        let want = "a failed: network: down. network: down";
        assert_eq!(f.channel("UCx").await.unwrap_err().to_string(), want);
        assert_eq!(f.playlist("PLx").await.unwrap_err().to_string(), want);
        assert_eq!(f.video("v").await.unwrap_err().to_string(), want);
    }

    #[tokio::test]
    async fn browse_more_uses_only_the_primary() {
        let page = pair(true, true).browse_more("t").await.unwrap();
        assert_eq!(page.continuation.as_deref(), Some("a"));
        assert!(matches!(pair(false, true).browse_more("t").await, Err(SourceError::Network(_))));
    }

    #[tokio::test]
    async fn comments_use_only_the_primary() {
        assert_eq!(pair(true, true).comments("v").await.unwrap().note.as_deref(), Some("a"));
        assert_eq!(pair(true, true).comments_more("v", "t").await.unwrap().continuation.as_deref(), Some("a"));
        assert!(matches!(pair(false, true).comments("v").await, Err(SourceError::Network(_))));
        assert!(matches!(pair(false, true).comments_more("v", "t").await, Err(SourceError::Network(_))));
    }
}
