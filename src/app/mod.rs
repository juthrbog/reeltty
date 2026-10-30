//! The app state and its update function (Elm style).
//!
//! All events arrive as `Message` values. `App::update` changes the state and
//! returns `Effect` values. The runtime carries out the effects, for example
//! by starting a task that later sends another `Message`. `update` never
//! blocks and never does I/O.

pub mod list;
pub mod player;
pub mod screens;
pub mod search;
pub mod thumbnails;

use std::sync::Arc;

use crate::keymap::{Action, KeyMap, KeyPress};
use crate::source::filters::SearchQuery;
use crate::source::model::{ChannelPage, ChannelRef, Item, ItemPage, PlaylistPage, SearchPage, VideoDetails};
use crate::source::streams::Streams;
use crate::store::{SavedPlaylist, Subscription};
use crate::terminal::detect::Graphics;
use crate::thumbnails::{Picture, Slot};
use list::ItemList;
use player::{PlayerCommand, PlayerEvent, PlayerState};
use screens::{ChannelScreen, PlaylistScreen, Screen, Target, VideoScreen};
use search::{KeyResult, SearchState};
use thumbnails::{THUMBNAIL_ROWS, Thumbnails};

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
    ChannelLoaded {
        request: u64,
        result: Result<ChannelPage, String>,
    },
    PlaylistLoaded {
        request: u64,
        result: Result<PlaylistPage, String>,
    },
    MoreLoaded {
        request: u64,
        result: Result<ItemPage, String>,
    },
    VideoLoaded {
        request: u64,
        result: Result<VideoDetails, String>,
    },
    /// A change to the local database failed.
    StoreFailed(String),
    /// The reply to `Effect::LoadThumbnail`.
    ThumbnailLoaded {
        url: String,
        slot: Slot,
        result: Result<Arc<Picture>, String>,
    },
    /// The reply to `Effect::ResolveStreams`. `now` is Unix seconds.
    StreamsResolved {
        session: u64,
        now: u64,
        result: Result<Box<Streams>, String>,
    },
    /// Once a second during `Effect::Countdown`. `now` is Unix seconds.
    Tick {
        session: u64,
        now: u64,
    },
    Player {
        session: u64,
        event: PlayerEvent,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Key(KeyPress),
    /// The size of the UI in cells.
    Resize {
        width: u16,
        height: u16,
    },
    /// The size of one cell in pixels.
    CellSize {
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
    LoadChannel {
        request: u64,
        id: String,
    },
    LoadPlaylist {
        request: u64,
        id: String,
    },
    /// The next page of the channel or playlist screen with this request.
    LoadMore {
        request: u64,
        continuation: String,
    },
    LoadVideo {
        request: u64,
        id: String,
    },
    Subscribe(Subscription),
    Unsubscribe {
        channel_id: String,
    },
    SavePlaylist(SavedPlaylist),
    UnsavePlaylist {
        playlist_id: String,
    },
    /// Download `url` and fit it into `slot`.
    LoadThumbnail {
        url: String,
        slot: Slot,
    },
    /// Find the stream URLs of a video, at most `max_height` pixels high.
    ResolveStreams {
        session: u64,
        id: String,
        max_height: u32,
    },
    /// Send `Message::Tick` once a second until the Unix time `until`.
    Countdown {
        session: u64,
        until: u64,
    },
    Player(PlayerCommand),
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

/// Rows of the UI around every list: the tab bar, the border, and the status bar.
const CHROME_ROWS: u16 = 4;
/// Rows above the list on the Search tab (the query line).
pub const SEARCH_HEADER_ROWS: u16 = 1;
/// Rows above the list on a channel or playlist screen.
pub const SCREEN_HEADER_ROWS: u16 = 2;
/// Each list item takes this many rows without thumbnails.
pub const ROWS_PER_ITEM: u16 = 2;

#[derive(Debug, Clone)]
pub struct App {
    pub keymap: KeyMap,
    pub graphics: Graphics,
    pub tab: Tab,
    pub show_help: bool,
    pub search: SearchState,
    /// Screens opened on top of the tabs. The last one is shown.
    pub screens: Vec<Screen>,
    pub subscriptions: Vec<Subscription>,
    pub subscriptions_selection: Selection,
    pub saved_playlists: Vec<SavedPlaylist>,
    pub saved_selection: Selection,
    /// A short message for the status bar, cleared by the next key.
    pub status: Option<String>,
    pub thumbnails: Thumbnails,
    /// The full-screen player, over everything else.
    pub player: Option<PlayerState>,
    pub width: u16,
    pub height: u16,
    /// The pixel size of one cell, or 0 x 0 if the terminal does not say.
    pub cell: (u16, u16),
    /// The number of the last screen request.
    request: u64,
    /// The number of the last video played.
    session: u64,
}

impl App {
    pub fn new(keymap: KeyMap, graphics: Graphics) -> Self {
        Self {
            keymap,
            graphics,
            tab: Tab::Search,
            show_help: false,
            search: SearchState::default(),
            screens: Vec::new(),
            subscriptions: Vec::new(),
            subscriptions_selection: Selection::default(),
            saved_playlists: Vec::new(),
            saved_selection: Selection::default(),
            status: None,
            thumbnails: Thumbnails::new(false),
            player: None,
            width: 80,
            height: 24,
            cell: (0, 0),
            request: 0,
            session: 0,
        }
    }

    /// The rows of one item in a list of videos, channels, and playlists.
    pub fn rows_per_item(&self) -> u16 {
        if self.thumbnails.enabled { THUMBNAIL_ROWS } else { ROWS_PER_ITEM }
    }

    /// The number of list items that fit below `header` rows.
    pub fn rows(&self, header: u16) -> usize {
        usize::from(self.height.saturating_sub(CHROME_ROWS + header) / self.rows_per_item()).max(1)
    }

    /// The number of entries that fit in the Subscriptions and Playlists tabs,
    /// which have no thumbnails.
    pub fn plain_rows(&self) -> usize {
        usize::from(self.height.saturating_sub(CHROME_ROWS) / ROWS_PER_ITEM).max(1)
    }

    /// The number of search results that fit on screen.
    pub fn page(&self) -> usize {
        self.rows(SEARCH_HEADER_ROWS)
    }

    pub fn update(&mut self, msg: Message) -> Vec<Effect> {
        let mut effects = self.handle(msg);
        if self.thumbnails.enabled && self.player.is_none() {
            // The visible items and the next screen of them.
            let (items, selection, rows) = match self.screens.last() {
                Some(Screen::Channel(s)) => (s.list.items.as_slice(), s.list.selection, self.rows(SCREEN_HEADER_ROWS)),
                Some(Screen::Playlist(s)) => (s.list.items.as_slice(), s.list.selection, self.rows(SCREEN_HEADER_ROWS)),
                None if self.tab == Tab::Search => (self.search.results.as_slice(), self.search.selection, self.page()),
                Some(Screen::Video(_)) | None => (&[][..], Selection::default(), 0),
            };
            effects.extend(self.thumbnails.want(items.iter().skip(selection.offset).take(rows * 2)));
        }
        effects
    }

    fn handle(&mut self, msg: Message) -> Vec<Effect> {
        match msg {
            Message::Input(input) => self.on_input(input),
            Message::InputClosed(why) => vec![Effect::Fail(format!("terminal input stopped: {why}"))],
            Message::Terminate => vec![Effect::Quit],
            Message::SearchResult { generation, append, result } => {
                let page = self.page();
                self.search.on_result(generation, append, result, page)
            }
            Message::ChannelLoaded { request, result } => {
                let rows = self.rows(SCREEN_HEADER_ROWS);
                let Some(Screen::Channel(s)) = self.screen_mut(request) else { return Vec::new() };
                match result {
                    Ok(page) => {
                        s.name.clone_from(&page.info.name);
                        s.info = Some(page.info);
                        more(request, s.list.add_page(page.videos, rows))
                    }
                    Err(e) => {
                        s.list.fail(e);
                        Vec::new()
                    }
                }
            }
            Message::PlaylistLoaded { request, result } => {
                let rows = self.rows(SCREEN_HEADER_ROWS);
                let Some(Screen::Playlist(s)) = self.screen_mut(request) else { return Vec::new() };
                match result {
                    Ok(page) => {
                        s.title.clone_from(&page.info.title);
                        s.info = Some(page.info);
                        more(request, s.list.add_page(page.videos, rows))
                    }
                    Err(e) => {
                        s.list.fail(e);
                        Vec::new()
                    }
                }
            }
            Message::MoreLoaded { request, result } => {
                let rows = self.rows(SCREEN_HEADER_ROWS);
                let Some(screen) = self.screen_mut(request) else { return Vec::new() };
                // Later pages of a channel's videos do not name the channel.
                let owner = match screen {
                    Screen::Channel(s) => Some(ChannelRef {
                        id: Some(s.id.clone()),
                        name: s.name.clone(),
                        handle: s.info.as_ref().and_then(|i| i.handle.clone()),
                    }),
                    _ => None,
                };
                let Some(list) = screen.list_mut() else { return Vec::new() };
                match result {
                    Ok(mut page) => {
                        if let Some(owner) = owner {
                            for item in &mut page.items {
                                if let Item::Video(v) = item
                                    && v.channels.is_empty()
                                {
                                    v.channels.push(owner.clone());
                                }
                            }
                        }
                        more(request, list.add_page(page, rows))
                    }
                    Err(e) => {
                        list.fail(e);
                        Vec::new()
                    }
                }
            }
            Message::VideoLoaded { request, result } => {
                let Some(Screen::Video(s)) = self.screen_mut(request) else { return Vec::new() };
                s.loading = false;
                match result {
                    Ok(d) => {
                        s.title.clone_from(&d.title);
                        s.details = Some(d);
                        s.error = None;
                    }
                    Err(e) => s.error = Some(e),
                }
                Vec::new()
            }
            Message::StoreFailed(e) => {
                self.status = Some(format!("Could not save: {e}"));
                Vec::new()
            }
            Message::ThumbnailLoaded { url, slot, result } => {
                self.thumbnails.loaded(&url, slot, result);
                Vec::new()
            }
            Message::StreamsResolved { session, now, result } => self.on_streams(session, now, result),
            Message::Tick { session, now } => self.on_tick(session, now),
            Message::Player { session, event } => self.on_player_event(session, event),
        }
    }

    fn on_input(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Resize { width, height } => {
                (self.width, self.height) = (width, height);
                self.clamp_all();
                self.refit_player()
            }
            Input::CellSize { width, height } => {
                self.cell = (width, height);
                self.thumbnails.set_cell(width, height);
                self.refit_player()
            }
            Input::Key(key) => {
                self.status = None;
                self.on_key(key)
            }
        }
    }

    fn screen_mut(&mut self, request: u64) -> Option<&mut Screen> {
        self.screens.iter_mut().find(|s| s.request() == request)
    }

    fn clamp_all(&mut self) {
        let (search, header, plain) = (self.page(), self.rows(SCREEN_HEADER_ROWS), self.plain_rows());
        self.search.selection.clamp(self.search.results.len(), search);
        self.subscriptions_selection.clamp(self.subscriptions.len(), plain);
        self.saved_selection.clamp(self.saved_playlists.len(), plain);
        for screen in &mut self.screens {
            if let Some(list) = screen.list_mut() {
                list.selection.clamp(list.items.len(), header);
            }
        }
    }

    fn on_key(&mut self, key: KeyPress) -> Vec<Effect> {
        // ctrl-c always quits, even while typing.
        if key == KeyPress::ctrl('c') {
            return vec![Effect::Quit];
        }
        if self.player.is_some() {
            return self.on_player_key(key);
        }
        let action = self.keymap.action(key);
        if action == Some(Action::Refresh) {
            self.thumbnails.forget_failed();
        }
        if self.show_help {
            match action {
                Some(Action::Help | Action::Back) => self.show_help = false,
                Some(Action::Quit) => return vec![Effect::Quit],
                _ => {}
            }
            return Vec::new();
        }
        if !self.screens.is_empty() {
            return self.on_screen_key(action);
        }
        match self.tab {
            Tab::Search => {
                let captured = self.search.captures_keys();
                if !captured
                    && let Some(a @ (Action::Open | Action::Details | Action::Channel | Action::Toggle)) = action
                {
                    return match self.search.selected().cloned() {
                        Some(item) => self.on_item(&item, a),
                        None => Vec::new(),
                    };
                }
                let global = matches!(action, Some(Action::Quit | Action::Help | Action::NextTab | Action::PrevTab));
                if (captured || !global)
                    && let KeyResult::Used(effects) = self.search.on_key(key, action, self.page())
                {
                    return effects;
                }
            }
            Tab::Subscriptions => {
                if let Some(effects) = self.on_subscriptions_key(action) {
                    return effects;
                }
            }
            Tab::Playlists => {
                if let Some(effects) = self.on_saved_key(action) {
                    return effects;
                }
            }
            Tab::History => {}
        }
        match action {
            Some(Action::Quit) => return vec![Effect::Quit],
            Some(Action::Help) => self.show_help = true,
            Some(Action::NextTab) => self.tab = self.tab.offset(1),
            Some(Action::PrevTab) => self.tab = self.tab.offset(-1),
            Some(Action::Search) => return self.start_search_input(),
            _ => {}
        }
        Vec::new()
    }

    /// Goes to the Search tab and opens the query line.
    fn start_search_input(&mut self) -> Vec<Effect> {
        self.screens.clear();
        self.tab = Tab::Search;
        // The key is only read while typing, and the query line is not open yet.
        let key = KeyPress::plain(crate::keymap::KeyCode::Char('/'));
        match self.search.on_key(key, Some(Action::Search), self.page()) {
            KeyResult::Used(effects) => effects,
            KeyResult::NotUsed => Vec::new(),
        }
    }

    /// Keys on the screen at the top of the stack.
    fn on_screen_key(&mut self, action: Option<Action>) -> Vec<Effect> {
        let Some(action) = action else { return Vec::new() };
        match action {
            Action::Quit => return vec![Effect::Quit],
            Action::Help => self.show_help = true,
            Action::Back => {
                self.screens.pop();
            }
            Action::Search => return self.start_search_input(),
            Action::NextTab | Action::PrevTab => {
                self.screens.clear();
                self.tab = self.tab.offset(if action == Action::NextTab { 1 } else { -1 });
            }
            Action::Refresh => return self.reload_top(),
            _ => return self.on_screen_action(action),
        }
        Vec::new()
    }

    fn on_screen_action(&mut self, action: Action) -> Vec<Effect> {
        let rows = self.rows(SCREEN_HEADER_ROWS);
        let Some(top) = self.screens.last_mut() else { return Vec::new() };
        let request = top.request();
        match top {
            Screen::Channel(s) => match action {
                Action::Toggle => {
                    let (id, name) = (s.id.clone(), s.name.clone());
                    let handle = s.info.as_ref().and_then(|i| i.handle.clone());
                    self.toggle_subscription(id, name, handle)
                }
                _ => self.on_screen_list(action, rows, request),
            },
            Screen::Playlist(s) => match action {
                Action::Toggle => {
                    let saved = saved_playlist(s);
                    self.toggle_saved(saved)
                }
                _ => self.on_screen_list(action, rows, request),
            },
            Screen::Video(s) => {
                let max = s.max_scroll.get();
                let half = u16::try_from(rows).unwrap_or(u16::MAX).max(1);
                match action {
                    Action::Up => s.scroll = s.scroll.min(max).saturating_sub(1),
                    Action::Down => s.scroll = s.scroll.saturating_add(1).min(max),
                    Action::HalfPageUp => s.scroll = s.scroll.min(max).saturating_sub(half),
                    Action::HalfPageDown => s.scroll = s.scroll.saturating_add(half).min(max),
                    Action::Top => s.scroll = 0,
                    Action::Bottom if max != u16::MAX => s.scroll = max,
                    Action::Channel | Action::Toggle => {
                        let channel = s.details.as_ref().and_then(|d| d.channel.clone());
                        let Some(c) = channel else { return Vec::new() };
                        if action == Action::Toggle {
                            let Some(id) = c.id else { return Vec::new() };
                            return self.toggle_subscription(id, c.name, c.handle);
                        }
                        return Target::channel(&c).map(|t| self.open(t)).unwrap_or_default();
                    }
                    Action::Open => {
                        let (id, title) = (s.id.clone(), s.title.clone());
                        return self.play(id, title);
                    }
                    _ => {}
                }
                Vec::new()
            }
        }
    }

    /// Navigation and item keys on a channel or playlist screen.
    fn on_screen_list(&mut self, action: Action, rows: usize, request: u64) -> Vec<Effect> {
        let Some(list) = self.screens.last_mut().and_then(Screen::list_mut) else { return Vec::new() };
        if let Some(token) = list.navigate(action, rows) {
            return more(request, token);
        }
        match list.selected().cloned() {
            Some(item) => self.on_item(&item, action),
            None => Vec::new(),
        }
    }

    /// Open, Details, Channel, and Toggle on one item in a list.
    fn on_item(&mut self, item: &Item, action: Action) -> Vec<Effect> {
        match action {
            Action::Open => match item {
                Item::Video(v) => self.play(v.id.clone(), v.title.clone()),
                _ => self.open(Target::open(item)),
            },
            Action::Details if matches!(item, Item::Video(_)) => self.open(Target::open(item)),
            Action::Channel => {
                if let Some(t) = Target::channel_of(item) {
                    return self.open(t);
                }
                self.status = Some("The channel of this item is not known.".into());
                Vec::new()
            }
            Action::Toggle => match item {
                Item::Channel(c) => self.toggle_subscription(c.id.clone(), c.name.clone(), c.handle.clone()),
                Item::Playlist(p) => self.toggle_saved(SavedPlaylist {
                    playlist_id: p.id.clone(),
                    title: p.title.clone(),
                    channel_id: p.channel.as_ref().and_then(|c| c.id.clone()),
                    channel_name: p.channel.as_ref().map(|c| c.name.clone()),
                    videos_text: p.count_text.clone(),
                    saved_at: 0,
                }),
                Item::Video(v) => match v.channels.iter().find(|c| c.id.is_some()) {
                    Some(c) => {
                        self.toggle_subscription(c.id.clone().unwrap_or_default(), c.name.clone(), c.handle.clone())
                    }
                    None => Vec::new(),
                },
            },
            _ => Vec::new(),
        }
    }

    /// Opens a screen on top and asks for its data.
    pub fn open(&mut self, target: Target) -> Vec<Effect> {
        self.request += 1;
        let request = self.request;
        let (screen, effect) = match target {
            Target::Channel { id, name } => (
                Screen::Channel(ChannelScreen { request, id: id.clone(), name, info: None, list: ItemList::loading() }),
                Effect::LoadChannel { request, id },
            ),
            Target::Playlist { id, title } => (
                Screen::Playlist(PlaylistScreen {
                    request,
                    id: id.clone(),
                    title,
                    info: None,
                    list: ItemList::loading(),
                }),
                Effect::LoadPlaylist { request, id },
            ),
            Target::Video { id, title } => (
                Screen::Video(VideoScreen {
                    request,
                    id: id.clone(),
                    title,
                    details: None,
                    loading: true,
                    error: None,
                    scroll: 0,
                    max_scroll: std::cell::Cell::new(u16::MAX),
                }),
                Effect::LoadVideo { request, id },
            ),
        };
        self.screens.push(screen);
        vec![effect]
    }

    /// Loads the top screen again.
    fn reload_top(&mut self) -> Vec<Effect> {
        let Some(top) = self.screens.pop() else { return Vec::new() };
        let target = match top {
            Screen::Channel(s) => Target::Channel { id: s.id, name: s.name },
            Screen::Playlist(s) => Target::Playlist { id: s.id, title: s.title },
            Screen::Video(s) => Target::Video { id: s.id, title: s.title },
        };
        self.open(target)
    }

    pub fn is_subscribed(&self, channel_id: &str) -> bool {
        self.subscriptions.iter().any(|s| s.channel_id == channel_id)
    }

    pub fn is_saved(&self, playlist_id: &str) -> bool {
        self.saved_playlists.iter().any(|p| p.playlist_id == playlist_id)
    }

    fn toggle_subscription(&mut self, channel_id: String, name: String, handle: Option<String>) -> Vec<Effect> {
        if let Some(i) = self.subscriptions.iter().position(|s| s.channel_id == channel_id) {
            let s = self.subscriptions.remove(i);
            self.status = Some(format!("Unsubscribed from {}.", s.name));
            return vec![Effect::Unsubscribe { channel_id }];
        }
        // The runtime sets the time when it saves.
        let s = Subscription { channel_id, name, handle, subscribed_at: 0 };
        self.status = Some(format!("Subscribed to {}.", s.name));
        let at = self.subscriptions.partition_point(|x| x.name.to_lowercase() <= s.name.to_lowercase());
        self.subscriptions.insert(at, s.clone());
        vec![Effect::Subscribe(s)]
    }

    fn toggle_saved(&mut self, p: SavedPlaylist) -> Vec<Effect> {
        if let Some(i) = self.saved_playlists.iter().position(|x| x.playlist_id == p.playlist_id) {
            let removed = self.saved_playlists.remove(i);
            self.status = Some(format!("Removed {} from saved playlists.", removed.title));
            return vec![Effect::UnsavePlaylist { playlist_id: p.playlist_id }];
        }
        self.status = Some(format!("Saved {}.", p.title));
        self.saved_playlists.insert(0, p.clone());
        vec![Effect::SavePlaylist(p)]
    }

    fn on_subscriptions_key(&mut self, action: Option<Action>) -> Option<Vec<Effect>> {
        let action = action?;
        let rows = self.plain_rows();
        if self.subscriptions_selection.apply(action, self.subscriptions.len(), rows) {
            return Some(Vec::new());
        }
        let s = self.subscriptions.get(self.subscriptions_selection.index)?.clone();
        match action {
            Action::Open | Action::Channel => Some(self.open(Target::Channel { id: s.channel_id, name: s.name })),
            Action::Toggle => {
                let effects = self.toggle_subscription(s.channel_id, s.name, s.handle);
                self.subscriptions_selection.clamp(self.subscriptions.len(), rows);
                Some(effects)
            }
            _ => None,
        }
    }

    fn on_saved_key(&mut self, action: Option<Action>) -> Option<Vec<Effect>> {
        let action = action?;
        let rows = self.plain_rows();
        if self.saved_selection.apply(action, self.saved_playlists.len(), rows) {
            return Some(Vec::new());
        }
        let p = self.saved_playlists.get(self.saved_selection.index)?.clone();
        match action {
            Action::Open => Some(self.open(Target::Playlist { id: p.playlist_id, title: p.title })),
            Action::Channel => {
                let id = p.channel_id?;
                Some(self.open(Target::Channel { id, name: p.channel_name.unwrap_or_default() }))
            }
            Action::Toggle => {
                let effects = self.toggle_saved(p);
                self.saved_selection.clamp(self.saved_playlists.len(), rows);
                Some(effects)
            }
            _ => None,
        }
    }
}

fn more(request: u64, token: Option<String>) -> Vec<Effect> {
    token.map(|continuation| Effect::LoadMore { request, continuation }).into_iter().collect()
}

fn saved_playlist(s: &PlaylistScreen) -> SavedPlaylist {
    let info = s.info.as_ref();
    SavedPlaylist {
        playlist_id: s.id.clone(),
        title: info.map_or_else(|| s.title.clone(), |i| i.title.clone()),
        channel_id: info.and_then(|i| i.channel.as_ref()).and_then(|c| c.id.clone()),
        channel_name: info.and_then(|i| i.channel.as_ref()).map(|c| c.name.clone()),
        videos_text: info.and_then(|i| i.videos_text.clone()),
        saved_at: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::filters::{ResultType, SearchFilters, Sort};
    use crate::source::model::{Video, VideoKind};
    use screens::{Screen, Target};

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
            thumbnails: vec![crate::source::model::Thumbnail {
                url: format!("https://i.ytimg.com/vi/{id}/hq.jpg"),
                width: 336,
                height: 188,
            }],
            members_only: false,
            badges: Vec::new(),
            description_snippet: None,
        })
    }

    fn thumbnail_urls(effects: &[Effect]) -> Vec<&str> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::LoadThumbnail { url, .. } => Some(url.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loads_the_thumbnails_of_visible_items() {
        let mut a = app();
        a.thumbnails = Thumbnails::new(true);
        a.update(Message::Input(Input::Resize { width: 80, height: 14 }));
        // Items are three rows high: (14 - 4 - 1) / 3.
        assert_eq!(a.page(), 3);
        assert_eq!(a.plain_rows(), 5);
        let generation = search(&mut a, "rust");
        let effects = a.update(Message::SearchResult { generation, append: false, result: Ok(page_of(10, 0, false)) });
        // The visible items and the next screen of them.
        let urls = thumbnail_urls(&effects);
        assert_eq!(urls.len(), 6);
        assert_eq!(urls[0], "https://i.ytimg.com/vi/0/hq.jpg");
        let mut later = Vec::new();
        for _ in 0..3 {
            later.extend(key(&mut a, "j"));
        }
        // Item 3 is selected, so the list scrolls by one and item 6 comes into range.
        assert_eq!(thumbnail_urls(&later), ["https://i.ytimg.com/vi/6/hq.jpg"]);
        // Video details have no thumbnails, and neither has the player.
        assert!(thumbnail_urls(&key(&mut a, "i")).is_empty());
        key(&mut a, "esc");
        assert!(matches!(key(&mut a, "enter").as_slice(), [Effect::ResolveStreams { .. }]));
        assert!(thumbnail_urls(&key(&mut a, "space")).is_empty());
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
    fn shorts_are_hidden_and_an_all_shorts_page_loads_the_next() {
        let short = |id: &str| match video(id) {
            Item::Video(mut v) => {
                v.kind = VideoKind::Short;
                Item::Video(v)
            }
            other => other,
        };
        let mut a = app();
        let g = search(&mut a, "x");
        let page = SearchPage {
            items: vec![short("s1"), short("s2")],
            continuation: Some("next".into()),
            ..SearchPage::default()
        };
        let effects = a.update(Message::SearchResult { generation: g, append: false, result: Ok(page) });
        assert!(a.search.results.is_empty());
        assert_eq!(effects, vec![Effect::SearchMore { generation: g, continuation: "next".into() }]);

        let page = SearchPage { items: vec![video("v1"), short("s3"), video("v2")], ..SearchPage::default() };
        a.update(Message::SearchResult { generation: g, append: true, result: Ok(page) });
        let ids: Vec<&str> =
            a.search.results.iter().map(|i| if let Item::Video(v) = i { v.id.as_str() } else { "?" }).collect();
        assert_eq!(ids, ["v1", "v2"]);
    }

    #[test]
    fn automatic_paging_stops_after_empty_pages() {
        let mut a = app();
        let g = search(&mut a, "x");
        let empty = |n: usize| SearchPage { continuation: Some(format!("t{n}")), ..SearchPage::default() };
        let mut asked = 0;
        for n in 0..10 {
            let effects = a.update(Message::SearchResult { generation: g, append: n > 0, result: Ok(empty(n)) });
            if effects.is_empty() {
                break;
            }
            asked += 1;
        }
        assert_eq!(asked, 2, "three empty pages in a row stop the loading");
        // Moving down tries again.
        assert_eq!(key(&mut a, "j"), vec![Effect::SearchMore { generation: g, continuation: "t2".into() }]);
    }

    #[test]
    fn a_retry_clears_the_old_error() {
        let mut a = app();
        let g = search(&mut a, "x");
        a.update(Message::SearchResult { generation: g, append: false, result: Ok(page_of(3, 0, true)) });
        // The first page was short, so the next page was asked for at once. It fails.
        a.update(Message::SearchResult { generation: g, append: true, result: Err("timed out".into()) });
        assert_eq!(a.search.error.as_deref(), Some("timed out"));
        assert_eq!(key(&mut a, "j"), vec![Effect::SearchMore { generation: g, continuation: "token3".into() }]);
        assert_eq!(a.search.error, None);
    }

    #[test]
    fn search_key_on_another_tab_opens_the_query_line() {
        let mut a = app();
        key(&mut a, "tab");
        key(&mut a, "/");
        assert_eq!(a.tab, Tab::Search);
        assert_eq!(a.search.input.as_deref(), Some(""));
    }

    #[test]
    fn alt_keys_do_not_type() {
        let mut a = app();
        key(&mut a, "/");
        key(&mut a, "alt-b");
        type_text(&mut a, "x");
        assert_eq!(a.search.input.as_deref(), Some("x"));
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

    fn channel_item(id: &str) -> Item {
        Item::Channel(crate::source::model::Channel {
            id: id.into(),
            name: format!("Chan {id}"),
            handle: Some(format!("@{id}")),
            subscribers_text: None,
            description_snippet: None,
            verified: false,
            thumbnails: Vec::new(),
        })
    }

    fn channel_page(items: Vec<Item>, more: bool) -> ChannelPage {
        ChannelPage {
            info: crate::source::model::ChannelInfo {
                id: "UC1".into(),
                name: "Real Name".into(),
                handle: Some("@real".into()),
                subscribers_text: None,
                videos_text: None,
                description: None,
                verified: false,
                avatar: Vec::new(),
            },
            videos: ItemPage { items, continuation: more.then(|| "next".to_string()) },
        }
    }

    fn results(a: &mut App, items: Vec<Item>) {
        let g = search(a, "x");
        a.update(Message::SearchResult {
            generation: g,
            append: false,
            result: Ok(SearchPage { items, ..SearchPage::default() }),
        });
    }

    #[test]
    fn opening_a_result_pushes_a_screen_and_back_pops_it() {
        let mut a = app();
        results(&mut a, vec![video("v1"), channel_item("UC1")]);
        assert_eq!(key(&mut a, "i"), vec![Effect::LoadVideo { request: 1, id: "v1".into() }]);
        assert!(matches!(a.screens.last(), Some(Screen::Video(_))));
        // Enter on the details plays the video. Back stops it, and the screens stay.
        assert_eq!(key(&mut a, "enter"), [Effect::ResolveStreams { session: 1, id: "v1".into(), max_height: 480 }]);
        assert_eq!(key(&mut a, "esc"), [Effect::Player(PlayerCommand::Stop)]);
        assert!(matches!(a.screens.last(), Some(Screen::Video(_))));
        key(&mut a, "esc");
        assert!(a.screens.is_empty());
        assert!(matches!(key(&mut a, "enter").as_slice(), [Effect::ResolveStreams { session: 2, .. }]));
        key(&mut a, "q");
        key(&mut a, "j");
        assert_eq!(key(&mut a, "enter"), vec![Effect::LoadChannel { request: 2, id: "UC1".into() }]);
    }

    #[test]
    fn channel_page_fills_in_and_pages() {
        let mut a = app();
        a.update(Message::Input(Input::Resize { width: 80, height: 24 }));
        let effects = a.open(Target::Channel { id: "UC1".into(), name: "Old".into() });
        assert_eq!(effects, vec![Effect::LoadChannel { request: 1, id: "UC1".into() }]);
        // A reply to another request changes nothing.
        a.update(Message::ChannelLoaded { request: 9, result: Ok(channel_page(vec![video("x")], false)) });
        let page: Vec<Item> = (0..20).map(|i| video(&i.to_string())).collect();
        assert_eq!(a.update(Message::ChannelLoaded { request: 1, result: Ok(channel_page(page, true)) }), vec![]);
        let Some(Screen::Channel(s)) = a.screens.last() else { panic!("no channel screen") };
        assert_eq!((s.name.as_str(), s.list.items.len()), ("Real Name", 20));
        // Moving near the end loads the next page for this screen.
        let mut asked = Vec::new();
        for _ in 0..19 {
            asked.extend(key(&mut a, "j"));
        }
        assert_eq!(asked, vec![Effect::LoadMore { request: 1, continuation: "next".into() }]);
        a.update(Message::MoreLoaded {
            request: 1,
            result: Ok(ItemPage { items: vec![video("z")], continuation: None }),
        });
        let Some(Screen::Channel(s)) = a.screens.last() else { panic!("no channel screen") };
        assert_eq!(s.list.items.len(), 21);
        // The later page did not name the channel, so the screen's channel fills it in.
        let Some(Item::Video(v)) = s.list.items.last() else { panic!("not a video") };
        assert_eq!(v.channels[0].id.as_deref(), Some("UC1"));
        assert_eq!(v.channels[0].name, "Real Name");
    }

    #[test]
    fn subscribe_toggles_and_persists() {
        let mut a = app();
        a.open(Target::Channel { id: "UC1".into(), name: "Chan".into() });
        a.update(Message::ChannelLoaded { request: 1, result: Ok(channel_page(Vec::new(), false)) });
        let effects = key(&mut a, "s");
        assert!(
            matches!(effects.as_slice(), [Effect::Subscribe(s)] if s.channel_id == "UC1" && s.handle.as_deref() == Some("@real"))
        );
        assert!(a.is_subscribed("UC1"));
        assert_eq!(key(&mut a, "s"), vec![Effect::Unsubscribe { channel_id: "UC1".into() }]);
        assert!(!a.is_subscribed("UC1"));
    }

    #[test]
    fn subscriptions_tab_opens_and_removes() {
        let mut a = app();
        a.subscriptions =
            vec![Subscription { channel_id: "UC1".into(), name: "A".into(), handle: None, subscribed_at: 1 }];
        key(&mut a, "tab");
        assert_eq!(key(&mut a, "enter"), vec![Effect::LoadChannel { request: 1, id: "UC1".into() }]);
        key(&mut a, "esc");
        assert_eq!(key(&mut a, "s"), vec![Effect::Unsubscribe { channel_id: "UC1".into() }]);
        assert!(a.subscriptions.is_empty());
        assert_eq!(key(&mut a, "enter"), vec![], "an empty list opens nothing");
    }

    #[test]
    fn saving_a_playlist_from_search() {
        let mut a = app();
        let playlist = Item::Playlist(crate::source::model::Playlist {
            id: "PL1".into(),
            title: "Mix".into(),
            kind: crate::source::model::PlaylistKind::Playlist,
            channel: None,
            count_text: Some("3 videos".into()),
            thumbnails: Vec::new(),
            first_video_id: None,
        });
        results(&mut a, vec![playlist]);
        let effects = key(&mut a, "s");
        assert!(matches!(effects.as_slice(), [Effect::SavePlaylist(p)] if p.playlist_id == "PL1"));
        key(&mut a, "tab");
        key(&mut a, "tab");
        assert_eq!(a.tab, Tab::Playlists);
        assert_eq!(key(&mut a, "enter"), vec![Effect::LoadPlaylist { request: 1, id: "PL1".into() }]);
    }

    #[test]
    fn channel_key_needs_a_known_channel() {
        let mut a = app();
        results(&mut a, vec![video("v1")]);
        assert_eq!(key(&mut a, "c"), vec![]);
        assert_eq!(a.status.as_deref(), Some("The channel of this item is not known."));
    }

    #[test]
    fn search_key_on_a_screen_goes_back_to_search() {
        let mut a = app();
        a.open(Target::Video { id: "v".into(), title: "t".into() });
        key(&mut a, "/");
        assert!(a.screens.is_empty());
        assert_eq!(a.search.input.as_deref(), Some(""));
    }

    #[test]
    fn video_screen_scrolls_and_reports_errors() {
        let mut a = app();
        a.open(Target::Video { id: "v".into(), title: "t".into() });
        a.update(Message::VideoLoaded { request: 1, result: Err("HTTP 404".into()) });
        let Some(Screen::Video(s)) = a.screens.last() else { panic!() };
        assert_eq!((s.loading, s.error.as_deref()), (false, Some("HTTP 404")));
        key(&mut a, "j");
        key(&mut a, "j");
        key(&mut a, "k");
        let Some(Screen::Video(s)) = a.screens.last() else { panic!() };
        assert_eq!(s.scroll, 1);
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
