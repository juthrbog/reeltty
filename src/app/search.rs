//! The Search tab: the query line, the results, and the filter menu.

use super::{Effect, Selection};
use crate::keymap::{Action, KeyCode, KeyPress};
use crate::source::filters::{Duration, ResultType, SearchFilters, SearchQuery, Sort, UploadDate};
use crate::source::model::{Item, SearchPage, Spelling, VideoKind};

/// Load the next page when the selection is this close to the end.
const PRELOAD_ROWS: usize = 5;
/// Pages in a row that add no visible results (for example only Shorts)
/// before automatic loading stops. Moving down loads again.
const MAX_EMPTY_PAGES: u8 = 3;

#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// The text being typed. `Some` while the query line has focus.
    pub input: Option<String>,
    /// The filters for the next search.
    pub filters: SearchFilters,
    /// The filter menu, when it is open.
    pub filter_menu: Option<FilterMenu>,
    /// The last search that was sent.
    pub query: Option<SearchQuery>,
    pub results: Vec<Item>,
    pub selection: Selection,
    pub continuation: Option<String>,
    pub estimated_results: Option<u64>,
    pub spelling: Option<Spelling>,
    pub note: Option<String>,
    pub error: Option<String>,
    pub loading: Loading,
    /// Counts searches, so that a late reply to an old search is dropped.
    pub generation: u64,
    /// Pages in a row that added no visible results.
    empty_pages: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Loading {
    #[default]
    Idle,
    FirstPage,
    MorePages,
}

/// What `SearchState::on_key` did with a key.
pub enum KeyResult {
    /// The key was used. These are the effects.
    Used(Vec<Effect>),
    /// The key was not for the search tab.
    NotUsed,
}

impl SearchState {
    /// True while the query line or the filter menu takes all keys.
    pub fn captures_keys(&self) -> bool {
        self.input.is_some() || self.filter_menu.is_some()
    }

    pub fn on_key(&mut self, key: KeyPress, action: Option<Action>, page: usize) -> KeyResult {
        if self.input.is_some() {
            return KeyResult::Used(self.on_input_key(key));
        }
        if let Some(menu) = &mut self.filter_menu {
            match action {
                Some(Action::Up) => menu.row = menu.row.saturating_sub(1),
                Some(Action::Down) => menu.row = (menu.row + 1).min(FilterRow::ALL.len() - 1),
                Some(Action::Open) => menu.cycle(&mut self.filters),
                Some(Action::Back | Action::Filters) => {
                    let before = menu.before;
                    self.filter_menu = None;
                    if self.filters != before {
                        return KeyResult::Used(self.research());
                    }
                }
                _ => {}
            }
            return KeyResult::Used(Vec::new());
        }
        let Some(action) = action else { return KeyResult::NotUsed };
        if matches!(action, Action::Up | Action::Down | Action::Bottom | Action::HalfPageDown) {
            self.empty_pages = 0;
        }
        match action {
            Action::Search => {
                self.input = Some(self.query.as_ref().map(|q| q.text.clone()).unwrap_or_default());
                KeyResult::Used(Vec::new())
            }
            Action::Filters => {
                self.filter_menu = Some(FilterMenu { row: 0, before: self.filters });
                KeyResult::Used(Vec::new())
            }
            Action::Refresh => KeyResult::Used(self.research()),
            _ if self.selection.apply(action, self.results.len(), page) => KeyResult::Used(self.maybe_load_more()),
            _ => KeyResult::NotUsed,
        }
    }

    fn on_input_key(&mut self, key: KeyPress) -> Vec<Effect> {
        let Some(input) = &mut self.input else { return Vec::new() };
        match (key.code, key.ctrl, key.alt) {
            (KeyCode::Enter, false, false) => {
                let text = input.trim().to_string();
                self.input = None;
                if text.is_empty() {
                    return Vec::new();
                }
                return self.start(SearchQuery { text, filters: self.filters });
            }
            (KeyCode::Esc, false, false) => self.input = None,
            (KeyCode::Backspace, false, false) => {
                input.pop();
            }
            (KeyCode::Char('u'), true, false) => input.clear(),
            (KeyCode::Char('w'), true, false) => {
                let keep = input.trim_end().rfind(' ').map_or(0, |i| i + 1);
                input.truncate(keep);
            }
            (KeyCode::Char(c), false, false) => input.push(c),
            _ => {}
        }
        Vec::new()
    }

    /// Runs the last query again with the current filters.
    fn research(&mut self) -> Vec<Effect> {
        match &self.query {
            Some(q) => {
                let text = q.text.clone();
                self.start(SearchQuery { text, filters: self.filters })
            }
            None => Vec::new(),
        }
    }

    fn start(&mut self, query: SearchQuery) -> Vec<Effect> {
        self.generation += 1;
        self.query = Some(query.clone());
        self.results.clear();
        self.selection = Selection::default();
        self.continuation = None;
        self.estimated_results = None;
        self.spelling = None;
        self.note = None;
        self.error = None;
        self.loading = Loading::FirstPage;
        self.empty_pages = 0;
        vec![Effect::Search { generation: self.generation, query }]
    }

    fn maybe_load_more(&mut self) -> Vec<Effect> {
        if self.loading != Loading::Idle || self.selection.index + PRELOAD_ROWS < self.results.len() {
            return Vec::new();
        }
        let Some(token) = self.continuation.clone() else { return Vec::new() };
        self.loading = Loading::MorePages;
        self.error = None;
        vec![Effect::SearchMore { generation: self.generation, continuation: token }]
    }

    /// A reply to `Effect::Search` (`append` false) or `Effect::SearchMore` (`append` true).
    pub fn on_result(
        &mut self,
        generation: u64,
        append: bool,
        result: Result<SearchPage, String>,
        page: usize,
    ) -> Vec<Effect> {
        if generation != self.generation {
            return Vec::new();
        }
        self.loading = Loading::Idle;
        match result {
            Ok(p) => {
                if !append {
                    self.estimated_results = p.estimated_results;
                    self.spelling = p.spelling;
                }
                if p.note.is_some() {
                    self.note = p.note;
                }
                // Shorts are hidden in search, as in the subscriptions feed.
                let before = self.results.len();
                self.results.extend(p.items.into_iter().filter(|i| !is_short(i)));
                self.empty_pages = if self.results.len() == before { self.empty_pages.saturating_add(1) } else { 0 };
                self.continuation = p.continuation;
                self.error = None;
                self.selection.clamp(self.results.len(), page);
                // A short page may leave the selection near the end already.
                if self.empty_pages >= MAX_EMPTY_PAGES {
                    return Vec::new();
                }
                self.maybe_load_more()
            }
            Err(e) => {
                self.error = Some(e);
                // Keep the token, so that moving down tries the next page again.
                Vec::new()
            }
        }
    }

    pub fn selected(&self) -> Option<&Item> {
        self.results.get(self.selection.index)
    }
}

fn is_short(item: &Item) -> bool {
    matches!(item, Item::Video(v) if v.kind == VideoKind::Short)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterRow {
    Sort,
    UploadDate,
    Type,
    Duration,
}

impl FilterRow {
    pub const ALL: [Self; 4] = [Self::Sort, Self::UploadDate, Self::Type, Self::Duration];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Sort => "Sort by",
            Self::UploadDate => "Upload date",
            Self::Type => "Type",
            Self::Duration => "Duration",
        }
    }

    pub fn value(self, f: &SearchFilters) -> &'static str {
        match self {
            Self::Sort => match f.sort {
                Sort::Relevance => "Relevance",
                Sort::ViewCount => "View count",
            },
            Self::UploadDate => match f.upload_date {
                None => "Any time",
                Some(UploadDate::LastHour) => "Last hour",
                Some(UploadDate::Today) => "Today",
                Some(UploadDate::ThisWeek) => "This week",
                Some(UploadDate::ThisMonth) => "This month",
                Some(UploadDate::ThisYear) => "This year",
            },
            Self::Type => match f.result_type {
                None => "Any",
                Some(ResultType::Video) => "Videos",
                Some(ResultType::Channel) => "Channels",
                Some(ResultType::Playlist) => "Playlists",
            },
            Self::Duration => match f.duration {
                None => "Any",
                Some(Duration::Under3Min) => "Under 3 minutes",
                Some(Duration::From3To20Min) => "3 to 20 minutes",
                Some(Duration::Over20Min) => "Over 20 minutes",
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterMenu {
    pub row: usize,
    /// The filters when the menu opened. Closing searches again if they changed.
    pub before: SearchFilters,
}

impl FilterMenu {
    /// Moves the selected row to its next value, and back to the start after the last one.
    fn cycle(&self, f: &mut SearchFilters) {
        fn next<T: Copy + PartialEq>(all: &[T], v: Option<T>) -> Option<T> {
            match v.and_then(|v| all.iter().position(|x| *x == v)) {
                None => all.first().copied(),
                Some(i) => all.get(i + 1).copied(),
            }
        }
        match FilterRow::ALL[self.row] {
            FilterRow::Sort => {
                f.sort = match f.sort {
                    Sort::Relevance => Sort::ViewCount,
                    Sort::ViewCount => Sort::Relevance,
                }
            }
            FilterRow::UploadDate => {
                use UploadDate::{LastHour, ThisMonth, ThisWeek, ThisYear, Today};
                f.upload_date = next(&[LastHour, Today, ThisWeek, ThisMonth, ThisYear], f.upload_date);
            }
            FilterRow::Type => {
                use ResultType::{Channel, Playlist, Video};
                f.result_type = next(&[Video, Channel, Playlist], f.result_type);
            }
            FilterRow::Duration => {
                use Duration::{From3To20Min, Over20Min, Under3Min};
                f.duration = next(&[Under3Min, From3To20Min, Over20Min], f.duration);
            }
        }
    }
}
