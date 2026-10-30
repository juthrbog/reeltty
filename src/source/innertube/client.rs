//! HTTP for InnerTube: the client context, the visitor ID, and errors.

use futures::future::BoxFuture;

use crate::source::{Result, SearchPage, SearchQuery, Source};

pub struct InnerTube {}

impl InnerTube {
    pub fn new() -> Result<Self> {
        todo!("InnerTube::new")
    }
}

impl Source for InnerTube {
    fn name(&self) -> &'static str {
        "InnerTube"
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>> {
        let _ = query;
        todo!("InnerTube::search")
    }

    fn search_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<SearchPage>> {
        let _ = continuation;
        todo!("InnerTube::search_more")
    }
}
