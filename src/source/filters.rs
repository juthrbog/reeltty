//! Search filters. Only options that the server honours are here: sort by
//! upload date and by rating are ignored by YouTube, so they are left out.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Relevance,
    ViewCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadDate {
    LastHour,
    Today,
    ThisWeek,
    ThisMonth,
    ThisYear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultType {
    Video,
    Channel,
    Playlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duration {
    Under3Min,
    From3To20Min,
    Over20Min,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchFilters {
    pub sort: Sort,
    pub upload_date: Option<UploadDate>,
    pub result_type: Option<ResultType>,
    pub duration: Option<Duration>,
}

impl SearchFilters {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub filters: SearchFilters,
}
