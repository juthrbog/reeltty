//! The yt-dlp source: the fallback for metadata.

use std::path::PathBuf;

use futures::future::BoxFuture;

use crate::source::{Result, SearchPage, SearchQuery, Source};

pub struct YtDlp {
    pub program: PathBuf,
}

impl Source for YtDlp {
    fn name(&self) -> &'static str {
        "yt-dlp"
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>> {
        let _ = query;
        todo!("YtDlp::search")
    }

    fn search_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<SearchPage>> {
        let _ = continuation;
        todo!("YtDlp::search_more")
    }
}
