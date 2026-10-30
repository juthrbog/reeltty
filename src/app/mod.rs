//! The app state and its update function (Elm style).
//!
//! All events arrive as `Message` values. `App::update` changes the state and
//! returns `Effect` values. The runtime carries out the effects, for example
//! by starting a task that later sends another `Message`. `update` never
//! blocks and never does I/O.

pub mod search;

use crate::keymap::{Action, KeyMap, KeyPress};
use crate::source::filters::SearchQuery;
use crate::source::model::{Item, SearchPage};
use crate::terminal::detect::Graphics;
use search::{KeyResult, SearchState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Input(Input),
    /// The input source stopped. The text says why.
    InputClosed(String),
    /// SIGTERM or SIGHUP.
    Terminate,
    /// The reply to `Effect::Search` (`append` false) or `Effect::SearchMore` (`append` true).
    SearchResult {
        generation: u64,
        append: bool,
        result: Result<SearchPage, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Key(KeyPress),
    /// The size of the UI in cells.
    Resize {
        width: u16,
        height: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Quit,
    /// Quit with an error.
    Fail(String),
    Search {
        generation: u64,
        query: SearchQuery,
    },
    SearchMore {
        generation: u64,
        continuation: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Subscriptions,
    Playlists,
    History,
}

impl Tab {
    pub const ALL: [Self; 4] = [Self::Search, Self::Subscriptions, Self::Playlists, Self::History];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Subscriptions => "Subscriptions",
            Self::Playlists => "Playlists",
            Self::History => "History",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).expect("tab in ALL")
    }

    fn offset(self, by: isize) -> Self {
        let n = Self::ALL.len().cast_signed();
        Self::ALL[(self.index().cast_signed() + by).rem_euclid(n).cast_unsigned()]
    }
}

/// The selected row in a list, and the first visible row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub index: usize,
    pub offset: usize,
}

impl Selection {
    /// Moves the selection for a navigation action. `len` is the list length,
    /// `page` is the number of visible rows. Returns `false` for other actions.
    pub fn apply(&mut self, action: Action, len: usize, page: usize) -> bool {
        let last = len.saturating_sub(1);
        let half = (page / 2).max(1);
        self.index = match action {
            Action::Up => self.index.saturating_sub(1),
            Action::Down => (self.index + 1).min(last),
            Action::Top => 0,
            Action::Bottom => last,
            Action::HalfPageUp => self.index.saturating_sub(half),
            Action::HalfPageDown => (self.index + half).min(last),
            _ => return false,
        };
        self.clamp(len, page);
        true
    }

    /// Keeps the selection in the list and the selected row on screen.
    pub fn clamp(&mut self, len: usize, page: usize) {
        let page = page.max(1);
        self.index = self.index.min(len.saturating_sub(1));
        if self.index < self.offset {
            self.offset = self.index;
        } else if self.index >= self.offset + page {
            self.offset = self.index + 1 - page;
        }
        self.offset = self.offset.min(len.saturating_sub(page));
    }
}

/// Rows of the UI outside a list: the tab bar, the border, the query line, and the status bar.
const CHROME_ROWS: u16 = 5;
/// Each list item takes this many rows.
pub const ROWS_PER_ITEM: u16 = 2;

#[derive(Debug, Clone)]
pub struct App {
    pub keymap: KeyMap,
    pub graphics: Graphics,
    pub tab: Tab,
    pub show_help: bool,
    pub search: SearchState,
    /// A short message for the status bar, cleared by the next key.
    pub status: Option<String>,
    pub height: u16,
}

impl App {
    pub fn new(keymap: KeyMap, graphics: Graphics) -> Self {
        Self {
            keymap,
            graphics,
            tab: Tab::Search,
            show_help: false,
            search: SearchState::default(),
            status: None,
            height: 24,
        }
    }

    /// The number of list items that fit on screen.
    pub fn page(&self) -> usize {
        usize::from(self.height.saturating_sub(CHROME_ROWS) / ROWS_PER_ITEM).max(1)
    }

    pub fn update(&mut self, msg: Message) -> Vec<Effect> {
        match msg {
            Message::Input(Input::Resize { height, .. }) => {
                self.height = height;
                let (len, page) = (self.search.results.len(), self.page());
                self.search.selection.clamp(len, page);
                Vec::new()
            }
            Message::Input(Input::Key(key)) => {
                self.status = None;
                self.on_key(key)
            }
            Message::InputClosed(why) => vec![Effect::Fail(format!("terminal input stopped: {why}"))],
            Message::Terminate => vec![Effect::Quit],
            Message::SearchResult { generation, append, result } => {
                let page = self.page();
                self.search.on_result(generation, append, result, page)
            }
        }
    }

    fn on_key(&mut self, key: KeyPress) -> Vec<Effect> {
        // ctrl-c always quits, even while typing.
        if key == KeyPress::ctrl('c') {
            return vec![Effect::Quit];
        }
        let action = self.keymap.action(key);
        if self.show_help {
            match action {
                Some(Action::Help | Action::Back) => self.show_help = false,
                Some(Action::Quit) => return vec![Effect::Quit],
                _ => {}
            }
            return Vec::new();
        }
        if self.tab == Tab::Search {
            let captured = self.search.captures_keys();
            let global = matches!(action, Some(Action::Quit | Action::Help | Action::NextTab | Action::PrevTab));
            if !captured && action == Some(Action::Open) {
                self.open_selected();
                return Vec::new();
            }
            if (captured || !global)
                && let KeyResult::Used(effects) = self.search.on_key(key, action, self.page())
            {
                return effects;
            }
        }
        match action {
            Some(Action::Quit) => return vec![Effect::Quit],
            Some(Action::Help) => self.show_help = true,
            Some(Action::NextTab) => self.tab = self.tab.offset(1),
            Some(Action::PrevTab) => self.tab = self.tab.offset(-1),
            _ => {}
        }
        Vec::new()
    }

    fn open_selected(&mut self) {
        let what = match self.search.selected() {
            Some(Item::Video(_)) => "Playing videos",
            Some(Item::Channel(_)) => "Channel pages",
            Some(Item::Playlist(_)) => "Playlist pages",
            None => return,
        };
        self.status = Some(format!("{what} come in a later version."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::filters::{ResultType, SearchFilters, Sort};
    use crate::source::model::{Video, VideoKind};

    fn app() -> App {
        App::new(KeyMap::default(), Graphics::KittyShm)
    }

    fn key(app: &mut App, s: &str) -> Vec<Effect> {
        app.update(Message::Input(Input::Key(s.parse().unwrap())))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            let k = KeyPress::plain(crate::keymap::KeyCode::Char(c));
            app.update(Message::Input(Input::Key(k)));
        }
    }

    pub(crate) fn video(id: &str) -> Item {
        Item::Video(Video {
            id: id.into(),
            title: format!("Video {id}"),
            kind: VideoKind::Normal,
            channels: Vec::new(),
            duration_text: None,
            duration_secs: None,
            views_text: None,
            published_text: None,
            thumbnails: Vec::new(),
            members_only: false,
            badges: Vec::new(),
            description_snippet: None,
        })
    }

    fn page_of(n: usize, from: usize, more: bool) -> SearchPage {
        SearchPage {
            items: (from..from + n).map(|i| video(&i.to_string())).collect(),
            continuation: more.then(|| format!("token{}", from + n)),
            ..SearchPage::default()
        }
    }

    fn search(app: &mut App, text: &str) -> u64 {
        key(app, "/");
        key(app, "ctrl-u");
        type_text(app, text);
        match key(app, "enter").as_slice() {
            [Effect::Search { generation, query }] => {
                assert_eq!(query.text, text);
                *generation
            }
            other => panic!("expected a search, got {other:?}"),
        }
    }

    #[test]
    fn quit_keys() {
        assert_eq!(key(&mut app(), "q"), vec![Effect::Quit]);
        assert_eq!(key(&mut app(), "ctrl-c"), vec![Effect::Quit]);
    }

    #[test]
    fn lost_input_or_a_signal_ends_the_app() {
        let mut a = app();
        assert_eq!(
            a.update(Message::InputClosed("EIO".into())),
            vec![Effect::Fail("terminal input stopped: EIO".into())]
        );
        assert_eq!(a.update(Message::Terminate), vec![Effect::Quit]);
    }

    #[test]
    fn tabs_wrap_around() {
        let mut a = app();
        key(&mut a, "shift-tab");
        assert_eq!(a.tab, Tab::History);
        key(&mut a, "tab");
        key(&mut a, "tab");
        assert_eq!(a.tab, Tab::Subscriptions);
    }

    #[test]
    fn help_takes_keys_while_open() {
        let mut a = app();
        key(&mut a, "?");
        assert!(a.show_help);
        key(&mut a, "tab");
        assert_eq!(a.tab, Tab::Search);
        key(&mut a, "esc");
        assert!(!a.show_help);
        key(&mut a, "?");
        assert_eq!(key(&mut a, "q"), vec![Effect::Quit]);
    }

    #[test]
    fn typing_a_query_takes_all_keys() {
        let mut a = app();
        key(&mut a, "/");
        type_text(&mut a, "q j?");
        assert_eq!(a.search.input.as_deref(), Some("q j?"));
        assert_eq!(a.tab, Tab::Search);
        assert!(!a.show_help);
        key(&mut a, "ctrl-w");
        assert_eq!(a.search.input.as_deref(), Some("q "));
        key(&mut a, "backspace");
        key(&mut a, "backspace");
        assert_eq!(a.search.input.as_deref(), Some(""));
        assert_eq!(key(&mut a, "enter"), vec![], "an empty query sends nothing");
        assert_eq!(a.search.input, None);
        key(&mut a, "/");
        assert_eq!(key(&mut a, "ctrl-c"), vec![Effect::Quit]);
    }

    #[test]
    fn results_and_paging() {
        let mut a = app();
        a.update(Message::Input(Input::Resize { width: 80, height: 25 }));
        assert_eq!(a.page(), 10);
        let g = search(&mut a, "rust");
        assert_eq!(a.search.loading, search::Loading::FirstPage);
        let none = a.update(Message::SearchResult { generation: g, append: false, result: Ok(page_of(20, 0, true)) });
        assert_eq!(none, vec![]);
        assert_eq!(a.search.results.len(), 20);

        // Moving near the end asks for the next page once.
        for _ in 0..14 {
            assert_eq!(key(&mut a, "j"), vec![]);
        }
        assert_eq!(key(&mut a, "j"), vec![Effect::SearchMore { generation: g, continuation: "token20".into() }]);
        assert_eq!(key(&mut a, "j"), vec![], "one request at a time");
        assert_eq!(a.search.selection, Selection { index: 16, offset: 7 });

        a.update(Message::SearchResult { generation: g, append: true, result: Ok(page_of(20, 20, false)) });
        assert_eq!(a.search.results.len(), 40);
        key(&mut a, "G");
        assert_eq!(a.search.selection.index, 39);
        assert_eq!(key(&mut a, "j"), vec![], "no token, no request");
    }

    #[test]
    fn a_late_reply_to_an_old_search_is_dropped() {
        let mut a = app();
        let old = search(&mut a, "one");
        let new = search(&mut a, "two");
        assert_ne!(old, new);
        a.update(Message::SearchResult { generation: old, append: false, result: Ok(page_of(3, 0, false)) });
        assert!(a.search.results.is_empty());
        assert_eq!(a.search.loading, search::Loading::FirstPage);
    }

    #[test]
    fn errors_are_kept_for_the_ui() {
        let mut a = app();
        let g = search(&mut a, "x");
        a.update(Message::SearchResult { generation: g, append: false, result: Err("network: down".into()) });
        assert_eq!(a.search.error.as_deref(), Some("network: down"));
        assert_eq!(a.search.loading, search::Loading::Idle);
    }

    #[test]
    fn filter_menu_searches_again_when_closed_with_changes() {
        let mut a = app();
        let g = search(&mut a, "rust");
        a.update(Message::SearchResult { generation: g, append: false, result: Ok(page_of(3, 0, false)) });

        key(&mut a, "f");
        assert!(a.search.filter_menu.is_some());
        assert_eq!(key(&mut a, "q"), vec![], "q does not quit while the menu is open");
        key(&mut a, "enter"); // sort: view count
        key(&mut a, "j");
        key(&mut a, "j");
        key(&mut a, "enter"); // type: videos
        let effects = key(&mut a, "esc");
        let expected =
            SearchFilters { sort: Sort::ViewCount, result_type: Some(ResultType::Video), ..SearchFilters::default() };
        assert_eq!(
            effects,
            vec![Effect::Search { generation: g + 1, query: SearchQuery { text: "rust".into(), filters: expected } }]
        );

        // No change, no search.
        key(&mut a, "f");
        assert_eq!(key(&mut a, "f"), vec![]);
    }

    #[test]
    fn filter_values_cycle_back_to_any() {
        let mut a = app();
        key(&mut a, "f");
        key(&mut a, "j"); // upload date
        for _ in 0..6 {
            key(&mut a, "enter");
        }
        assert_eq!(a.search.filters.upload_date, None);
    }

    #[test]
    fn open_shows_a_message_for_now() {
        let mut a = app();
        let g = search(&mut a, "x");
        a.update(Message::SearchResult { generation: g, append: false, result: Ok(page_of(1, 0, false)) });
        key(&mut a, "enter");
        assert_eq!(a.status.as_deref(), Some("Playing videos come in a later version."));
        key(&mut a, "j");
        assert_eq!(a.status, None);
    }

    #[test]
    fn selection_stays_in_bounds_and_on_screen() {
        let mut s = Selection::default();
        assert!(s.apply(Action::Up, 10, 6));
        assert_eq!(s, Selection { index: 0, offset: 0 });
        s.apply(Action::HalfPageDown, 10, 6);
        assert_eq!(s, Selection { index: 3, offset: 0 });
        s.apply(Action::Bottom, 10, 6);
        assert_eq!(s, Selection { index: 9, offset: 4 });
        s.apply(Action::Down, 10, 6);
        assert_eq!(s.index, 9);
        s.apply(Action::HalfPageUp, 10, 6);
        assert_eq!(s, Selection { index: 6, offset: 4 });
        s.apply(Action::Top, 10, 6);
        assert_eq!(s, Selection { index: 0, offset: 0 });
        s.apply(Action::Bottom, 0, 6);
        assert_eq!(s, Selection { index: 0, offset: 0 });
        assert!(!s.apply(Action::Open, 10, 6));
    }
}
