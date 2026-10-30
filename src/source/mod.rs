//! Where metadata comes from. The `Source` trait hides it.
//!
//! - `innertube`: the main source, the private API that the YouTube website uses.
//! - `ytdlp`: the fallback. It is slower, but yt-dlp gets fixes fastest.
//!
//! `Fallback` tries one source, then the other.

pub mod filters;
pub mod innertube;
pub mod model;
pub mod ytdlp;

use futures::future::BoxFuture;

pub use filters::SearchQuery;
pub use model::SearchPage;

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
                        SourceError::Program(format!(
                            "{} failed: {first}. {} failed: {second}",
                            self.primary.name(),
                            self.secondary.name()
                        ))
                    })?;
                    page.note = Some(format!(
                        "Results from {}, because {} failed: {first}",
                        self.secondary.name(),
                        self.primary.name()
                    ));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::filters::SearchFilters;

    struct Fixed(&'static str, bool);

    impl Source for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn search<'a>(&'a self, _: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>> {
            Box::pin(async move {
                if self.1 { Ok(SearchPage::default()) } else { Err(SourceError::Network("down".into())) }
            })
        }
        fn search_more<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<SearchPage>> {
            Box::pin(async { Err(SourceError::Unsupported("no".into())) })
        }
    }

    fn query() -> SearchQuery {
        SearchQuery { text: "x".into(), filters: SearchFilters::default() }
    }

    #[tokio::test]
    async fn uses_primary_when_it_works() {
        let f = Fallback { primary: Fixed("a", true), secondary: Fixed("b", true) };
        assert_eq!(f.search(&query()).await.unwrap().note, None);
    }

    #[tokio::test]
    async fn falls_back_with_a_note() {
        let f = Fallback { primary: Fixed("a", false), secondary: Fixed("b", true) };
        let page = f.search(&query()).await.unwrap();
        assert_eq!(page.note.as_deref(), Some("Results from b, because a failed: network: down"));
    }

    #[tokio::test]
    async fn both_failing_is_an_error() {
        let f = Fallback { primary: Fixed("a", false), secondary: Fixed("b", false) };
        let err = f.search(&query()).await.unwrap_err();
        assert_eq!(err.to_string(), "a failed: network: down. b failed: network: down");
    }
}
