//! The app state and its update function (Elm style).
//!
//! All events arrive as `Message` values. `App::update` changes the state and
//! returns `Effect` values. The runtime carries out the effects, for example
//! by starting a task that later sends another `Message`. `update` never
//! blocks and never does I/O.

pub mod comments;
pub mod feed;
pub mod list;
pub mod player;
pub mod screens;
pub mod search;
pub mod thumbnails;

use std::sync::Arc;

use crate::keymap::{Action, KeyMap, KeyPress};
use crate::source::filters::SearchQuery;
use crate::source::model::{
    ChannelPage, ChannelRef, CommentPage, Item, ItemPage, PlaylistPage, SearchPage, Video, VideoDetails,
};
use crate::source::streams::Streams;
use crate::sponsorblock::Segment;
use crate::store::{FeedVideo, HISTORY_LIMIT, HistoryEntry, SavedPlaylist, Subscription};
use crate::terminal::detect::Graphics;
use crate::thumbnails::{Picture, Slot};
use comments::{CommentRow, CommentsFrom, CommentsLoad, CommentsSlot, CommentsState};
use feed::FeedState;
use list::ItemList;
use player::{PlayerCommand, PlayerEvent, PlayerState};
use screens::{ChannelScreen, PlaylistScreen, Screen, Target, VideoPane, VideoScreen};
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
    /// The reply to `Effect::LoadComments`.
    CommentsLoaded {
        request: u64,
        generation: u64,
        slot: CommentsSlot,
        result: Result<CommentPage, String>,
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
    /// The reply to `Effect::LoadFeed`: how many channels will load.
    FeedLoading {
        count: usize,
    },
    /// One channel of `Effect::LoadFeed`. `fetched_at` is Unix seconds.
    FeedChannelLoaded {
        channel_id: String,
        fetched_at: i64,
        result: Result<Vec<Video>, String>,
    },
    /// The reply to `Effect::LoadRelated`.
    RelatedLoaded {
        session: u64,
        result: Result<Vec<Video>, String>,
    },
    /// The reply to `Effect::LoadSegments`.
    SegmentsLoaded {
        session: u64,
        result: Result<Vec<Segment>, String>,
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
    /// A page of comments for the video screen with this request.
    LoadComments {
        request: u64,
        generation: u64,
        video_id: String,
        slot: CommentsSlot,
        from: CommentsFrom,
    },
    Subscribe(Subscription),
    Unsubscribe {
        channel_id: String,
    },
    SavePlaylist(SavedPlaylist),
    UnsavePlaylist {
        playlist_id: String,
    },
    /// Load the videos tab of the channels that were not loaded in the last
    /// 30 minutes, or of all with `force`. Each channel comes with the time it
    /// was loaded, if it was.
    LoadFeed {
        channels: Vec<(String, Option<i64>)>,
        force: bool,
    },
    /// Replace the feed videos of a channel in the database.
    SaveFeed {
        channel_id: String,
        fetched_at: i64,
        videos: Vec<FeedVideo>,
    },
    ForgetFeed {
        channel_id: String,
    },
    /// Add a watch to the history, or replace the last watch of the video.
    SaveWatch(HistoryEntry),
    ForgetWatch {
        video_id: String,
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
    /// Send `Message::Tick` once a second, `count` times.
    Ticks {
        session: u64,
        count: u8,
    },
    /// Find the videos related to the one that plays.
    LoadRelated {
        session: u64,
        id: String,
    },
    /// Find the SponsorBlock segments of the video that plays.
    LoadSegments {
        session: u64,
        id: String,
    },
    Player(PlayerCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Feed,
    Subscriptions,
    Playlists,
    Queue,
    History,
}

impl Tab {
    pub const ALL: [Self; 6] =
        [Self::Search, Self::Feed, Self::Subscriptions, Self::Playlists, Self::Queue, Self::History];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Feed => "Feed",
            Self::Subscriptions => "Subscriptions",
            Self::Playlists => "Playlists",
            Self::Queue => "Queue",
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
/// Rows above the list on the Feed tab (the status line).
pub const FEED_HEADER_ROWS: u16 = 1;
/// Rows above the list on a channel or playlist screen.
pub const SCREEN_HEADER_ROWS: u16 = 2;
/// Each list item takes this many rows without thumbnails.
pub const ROWS_PER_ITEM: u16 = 2;

#[derive(Debug, Clone)]
#[expect(clippy::struct_excessive_bools, reason = "independent settings from the configuration")]
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
    /// The subscriptions feed.
    pub feed: FeedState,
    /// Watched videos, newest first.
    pub history: Vec<HistoryEntry>,
    pub history_selection: Selection,
    /// A short message for the status bar, cleared by the next key.
    pub status: Option<String>,
    pub thumbnails: Thumbnails,
    /// The player: full size over everything else, or in the mini-player
    /// next to the browse screen.
    pub player: Option<PlayerState>,
    /// The caption language, such as `en`.
    pub captions_language: String,
    /// Show captions when a video starts.
    pub captions_on: bool,
    /// Videos to play next, in order.
    pub queue: Vec<Video>,
    pub queue_selection: Selection,
    /// When a video ends, play the next one from the queue or the related videos.
    pub autoplay: bool,
    /// Skip SponsorBlock segments. The runtime has the categories.
    pub sponsorblock: bool,
    /// The videos played before the one that plays now, oldest first. It
    /// lasts until the player closes.
    played: Vec<Video>,
    /// The videos that "previous video" went back from, the latest last.
    /// "Next video" plays them again before the queue.
    forward: Vec<Video>,
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
            feed: FeedState::default(),
            history: Vec::new(),
            history_selection: Selection::default(),
            status: None,
            thumbnails: Thumbnails::new(false),
            player: None,
            captions_language: "en".into(),
            captions_on: false,
            queue: Vec::new(),
            queue_selection: Selection::default(),
            autoplay: true,
            sponsorblock: false,
            played: Vec::new(),
            forward: Vec::new(),
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
        // Keep the place in the video that plays, before the app quits or fails.
        if effects.iter().any(|e| matches!(e, Effect::Quit | Effect::Fail(_)))
            && let Some(save) = self.save_watch(false)
        {
            effects.insert(0, save);
        }
        if self.thumbnails.enabled && self.player.as_ref().is_none_or(|p| p.mini) {
            // The visible items and the next screen of them.
            let (items, selection, rows) = match self.screens.last() {
                Some(Screen::Channel(s)) => (s.list.items.as_slice(), s.list.selection, self.rows(SCREEN_HEADER_ROWS)),
                Some(Screen::Playlist(s)) => (s.list.items.as_slice(), s.list.selection, self.rows(SCREEN_HEADER_ROWS)),
                None if self.tab == Tab::Search => (self.search.results.as_slice(), self.search.selection, self.page()),
                None if self.tab == Tab::Feed => {
                    (self.feed.items.as_slice(), self.feed.selection, self.rows(FEED_HEADER_ROWS))
                }
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
                self.on_video_loaded(request, result);
                Vec::new()
            }
            Message::CommentsLoaded { request, generation, slot, result } => {
                self.on_comments_loaded(request, generation, slot, result)
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
            Message::RelatedLoaded { session, result } => self.on_related(session, result),
            Message::SegmentsLoaded { session, result } => self.on_segments(session, result),
            Message::FeedLoading { count } => self.on_feed_loading(count),
            Message::FeedChannelLoaded { channel_id, fetched_at, result } => {
                self.on_feed_channel(channel_id, fetched_at, result)
            }
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
        self.queue_selection.clamp(self.queue.len(), plain);
        self.history_selection.clamp(self.history.len(), plain);
        let feed = self.rows(FEED_HEADER_ROWS);
        self.feed.selection.clamp(self.feed.items.len(), feed);
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
        if self.player.as_ref().is_some_and(|p| !p.mini) {
            return self.on_player_key(key);
        }
        if self.player.is_some()
            && let Some(effects) = self.on_mini_player_key(key)
        {
            return effects;
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
                    && let Some(
                        a @ (Action::Open | Action::Details | Action::Channel | Action::Toggle | Action::Enqueue),
                    ) = action
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
            Tab::Feed => {
                if let Some(effects) = self.on_feed_key(action) {
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
            Tab::Queue => {
                if let Some(effects) = self.on_queue_key(action) {
                    return effects;
                }
            }
            Tab::History => {
                if let Some(effects) = self.on_history_key(action) {
                    return effects;
                }
            }
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
                // The comments go back to the details first.
                if let Some(Screen::Video(s)) = self.screens.last_mut()
                    && s.pane == VideoPane::Comments
                {
                    s.pane = VideoPane::Details;
                } else {
                    self.screens.pop();
                }
            }
            Action::Search => return self.start_search_input(),
            Action::NextTab | Action::PrevTab => {
                self.screens.clear();
                self.tab = self.tab.offset(if action == Action::NextTab { 1 } else { -1 });
            }
            Action::Refresh if matches!(self.screens.last(), Some(Screen::Video(s)) if s.pane == VideoPane::Comments) =>
            {
                return self.reload_comments(None);
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
            Screen::Video(s) if s.pane == VideoPane::Comments => self.on_comments_action(action, rows),
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
                    Action::Comments => return self.show_comments(),
                    Action::Enqueue => {
                        let video = match &s.details {
                            Some(d) => player::details_video(d),
                            None => player::plain_video(&s.id, &s.title),
                        };
                        self.toggle_queued(&video);
                    }
                    _ => {}
                }
                Vec::new()
            }
        }
    }

    /// Shows the comments of the video screen on top. They load the first time.
    fn show_comments(&mut self) -> Vec<Effect> {
        self.request += 1;
        let generation = self.request;
        let Some(Screen::Video(s)) = self.screens.last_mut() else { return Vec::new() };
        s.pane = VideoPane::Comments;
        if s.comments.is_some() {
            return Vec::new();
        }
        s.comments = Some(Box::new(CommentsState::loading(generation)));
        let from = first_comments(s);
        vec![comments_effect(s.request, generation, &s.id, CommentsLoad { slot: CommentsSlot::First, from })]
    }

    /// Loads the comments again from the first page, in the order `sort`, or
    /// in the order that shows now.
    fn reload_comments(&mut self, sort: Option<(String, String)>) -> Vec<Effect> {
        self.request += 1;
        let generation = self.request;
        let Some(Screen::Video(s)) = self.screens.last_mut() else { return Vec::new() };
        let Some(c) = s.comments.as_mut() else { return Vec::new() };
        let (label, token) = match sort {
            Some((label, token)) => (Some(label), Some(token)),
            None => (None, c.sort().map(|o| o.continuation.clone())),
        };
        c.restart(generation, label.as_deref());
        let from = token.map_or_else(|| first_comments(s), CommentsFrom::Token);
        vec![comments_effect(s.request, generation, &s.id, CommentsLoad { slot: CommentsSlot::First, from })]
    }

    fn on_video_loaded(&mut self, request: u64, result: Result<VideoDetails, String>) {
        let Some(Screen::Video(s)) = self.screen_mut(request) else { return };
        s.loading = false;
        match result {
            Ok(d) => {
                s.title.clone_from(&d.title);
                s.details = Some(d);
                s.error = None;
            }
            Err(e) => s.error = Some(e),
        }
    }

    fn on_comments_loaded(
        &mut self,
        request: u64,
        generation: u64,
        slot: CommentsSlot,
        result: Result<CommentPage, String>,
    ) -> Vec<Effect> {
        let Some(Screen::Video(s)) = self.screen_mut(request) else { return Vec::new() };
        // A page for an older list, before a new order or a refresh.
        let Some(c) = s.comments.as_mut().filter(|c| c.generation == generation) else { return Vec::new() };
        match result {
            Ok(page) => {
                let load = c.add_page(slot, page);
                load.map(|l| comments_effect(request, generation, &s.id, l)).into_iter().collect()
            }
            Err(e) => {
                c.fail(&slot, e);
                Vec::new()
            }
        }
    }

    /// Keys while the video screen shows the comments.
    fn on_comments_action(&mut self, action: Action, rows: usize) -> Vec<Effect> {
        // Each comment takes about three rows.
        let page = rows * usize::from(self.rows_per_item()) / 3;
        let Some(Screen::Video(s)) = self.screens.last_mut() else { return Vec::new() };
        let (request, id) = (s.request, s.id.clone());
        if action == Action::Comments {
            s.pane = VideoPane::Details;
            return Vec::new();
        }
        let Some(c) = s.comments.as_mut() else { return Vec::new() };
        let generation = c.generation;
        let load = match action {
            Action::Open => c.open(),
            Action::Sort => {
                let Some(next) = c.next_sort() else { return Vec::new() };
                let sort = (next.label.clone(), next.continuation.clone());
                return self.reload_comments(Some(sort));
            }
            Action::Channel => {
                let Some(CommentRow::Comment { comment, .. }) = c.selected_row() else { return Vec::new() };
                let author = comment.author.clone();
                return Target::channel(&author).map(|t| self.open(t)).unwrap_or_default();
            }
            _ => c.navigate(action, page).flatten(),
        };
        load.map(|l| comments_effect(request, generation, &id, l)).into_iter().collect()
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
            Action::Enqueue => {
                if let Item::Video(v) = item {
                    self.toggle_queued(v);
                } else {
                    self.status = Some("Only videos can go in the queue.".into());
                }
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
                    pane: VideoPane::Details,
                    comments: None,
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
            self.feed.forget_channel(&channel_id);
            self.rebuild_feed();
            return vec![Effect::Unsubscribe { channel_id: channel_id.clone() }, Effect::ForgetFeed { channel_id }];
        }
        // The runtime sets the time when it saves.
        let s = Subscription { channel_id, name, handle, subscribed_at: 0 };
        self.status = Some(format!("Subscribed to {}.", s.name));
        let at = self.subscriptions.partition_point(|x| x.name.to_lowercase() <= s.name.to_lowercase());
        self.subscriptions.insert(at, s.clone());
        let load = Effect::LoadFeed { channels: vec![(s.channel_id.clone(), None)], force: true };
        vec![Effect::Subscribe(s), load]
    }

    /// Effects to run once, when the app starts: the feed loads the channels
    /// that were not loaded in the last 30 minutes.
    pub fn start(&mut self) -> Vec<Effect> {
        self.rebuild_feed();
        self.load_feed(false)
    }

    fn load_feed(&self, force: bool) -> Vec<Effect> {
        if self.subscriptions.is_empty() {
            return Vec::new();
        }
        let channels = self
            .subscriptions
            .iter()
            .map(|s| (s.channel_id.clone(), self.feed.fetched.get(&s.channel_id).copied()))
            .collect();
        vec![Effect::LoadFeed { channels, force }]
    }

    /// `count` more channels load now.
    fn on_feed_loading(&mut self, count: usize) -> Vec<Effect> {
        self.feed.loading += count;
        self.feed.error = None;
        Vec::new()
    }

    fn on_feed_channel(
        &mut self,
        channel_id: String,
        fetched_at: i64,
        result: Result<Vec<Video>, String>,
    ) -> Vec<Effect> {
        self.feed.loading = self.feed.loading.saturating_sub(1);
        // The user unsubscribed while it loaded.
        let Some(sub) = self.subscriptions.iter().find(|s| s.channel_id == channel_id) else { return Vec::new() };
        let channel = ChannelRef { id: Some(channel_id.clone()), name: sub.name.clone(), handle: sub.handle.clone() };
        match result.map(|videos| self.feed.set_channel(&channel, fetched_at, videos)) {
            Ok(Some(videos)) => {
                self.rebuild_feed();
                vec![Effect::SaveFeed { channel_id, fetched_at, videos }]
            }
            Ok(None) => {
                self.feed.error = Some(format!("{}: no dates for its videos", channel.name));
                Vec::new()
            }
            Err(e) => {
                self.feed.error = Some(format!("{}: {e}", channel.name));
                Vec::new()
            }
        }
    }

    /// Builds the feed list again, for the subscribed channels.
    fn rebuild_feed(&mut self) {
        let rows = self.rows(FEED_HEADER_ROWS);
        let subscribed: Vec<&str> = self.subscriptions.iter().map(|s| s.channel_id.as_str()).collect();
        self.feed.rebuild(&subscribed, rows);
    }

    fn on_feed_key(&mut self, action: Option<Action>) -> Option<Vec<Effect>> {
        let action = action?;
        let rows = self.rows(FEED_HEADER_ROWS);
        if self.feed.selection.apply(action, self.feed.items.len(), rows) {
            return Some(Vec::new());
        }
        if action == Action::Refresh {
            if self.feed.loading > 0 {
                self.status = Some("The feed is loading already.".into());
                return Some(Vec::new());
            }
            return Some(self.load_feed(true));
        }
        let item = self.feed.items.get(self.feed.selection.index)?.clone();
        match action {
            Action::Open | Action::Details | Action::Channel | Action::Toggle | Action::Enqueue => {
                Some(self.on_item(&item, action))
            }
            _ => None,
        }
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

    /// Adds a video to the end of the queue, or removes it if it is there.
    fn toggle_queued(&mut self, v: &Video) {
        if let Some(i) = self.queue.iter().position(|q| q.id == v.id) {
            self.queue.remove(i);
            self.status = Some(format!("Removed from the queue. {} left.", self.queue.len()));
        } else {
            self.queue.push(v.clone());
            self.status = Some(format!("Added to the queue. {} in the queue.", self.queue.len()));
        }
        self.queue_selection.clamp(self.queue.len(), self.plain_rows());
        self.refresh_up_next();
    }

    fn on_queue_key(&mut self, action: Option<Action>) -> Option<Vec<Effect>> {
        let action = action?;
        let rows = self.plain_rows();
        if self.queue_selection.apply(action, self.queue.len(), rows) {
            return Some(Vec::new());
        }
        let v = self.queue.get(self.queue_selection.index)?.clone();
        match action {
            Action::Open => Some(self.play(v.id, v.title)),
            Action::Enqueue | Action::Delete => {
                self.toggle_queued(&v);
                Some(Vec::new())
            }
            Action::Details => Some(self.open(Target::open(&Item::Video(v)))),
            Action::Channel => match Target::channel_of(&Item::Video(v)) {
                Some(t) => Some(self.open(t)),
                None => Some(Vec::new()),
            },
            _ => None,
        }
    }

    fn on_history_key(&mut self, action: Option<Action>) -> Option<Vec<Effect>> {
        let action = action?;
        let rows = self.plain_rows();
        if self.history_selection.apply(action, self.history.len(), rows) {
            return Some(Vec::new());
        }
        let e = self.history.get(self.history_selection.index)?.clone();
        let video = player::history_video(&e);
        match action {
            Action::Open => Some(self.play(e.video_id, e.title)),
            Action::Enqueue => {
                self.toggle_queued(&video);
                Some(Vec::new())
            }
            Action::Delete => {
                self.history.remove(self.history_selection.index);
                self.history_selection.clamp(self.history.len(), rows);
                Some(vec![Effect::ForgetWatch { video_id: e.video_id }])
            }
            Action::Details => Some(self.open(Target::open(&Item::Video(video)))),
            Action::Channel => match Target::channel_of(&Item::Video(video)) {
                Some(t) => Some(self.open(t)),
                None => Some(Vec::new()),
            },
            _ => None,
        }
    }

    /// Puts a watch at the top of the history.
    fn remember_watch(&mut self, e: &HistoryEntry) {
        // The selection stays on the same video when the entries move.
        let selected = self.history.get(self.history_selection.index).map(|h| h.video_id.clone());
        self.history.retain(|h| h.video_id != e.video_id);
        self.history.insert(0, e.clone());
        self.history.truncate(HISTORY_LIMIT);
        if let Some(i) = selected.and_then(|id| self.history.iter().position(|h| h.video_id == id)) {
            self.history_selection.index = i;
        }
        self.history_selection.clamp(self.history.len(), self.plain_rows());
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

/// Where the first page of a video's comments comes from: the token in the
/// details, or else the video.
fn first_comments(s: &VideoScreen) -> CommentsFrom {
    match s.details.as_ref().and_then(|d| d.comments.clone()) {
        Some(token) => CommentsFrom::Token(token),
        None => CommentsFrom::Video(s.id.clone()),
    }
}

fn comments_effect(request: u64, generation: u64, video_id: &str, load: CommentsLoad) -> Effect {
    Effect::LoadComments { request, generation, video_id: video_id.to_string(), slot: load.slot, from: load.from }
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
    fn loads_thumbnails_with_the_mini_player_but_not_the_full_one() {
        let mut a = app();
        a.thumbnails = Thumbnails::new(true);
        a.update(Message::Input(Input::Resize { width: 100, height: 24 }));
        let generation = search(&mut a, "rust");
        a.play("v1".into(), "Title".into());
        let effects = a.update(Message::SearchResult { generation, append: false, result: Ok(page_of(10, 0, false)) });
        assert!(thumbnail_urls(&effects).is_empty());
        let effects = a.update(Message::Input(Input::Key("esc".parse().unwrap())));
        assert!(!thumbnail_urls(&effects).is_empty(), "{effects:?}");
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
        assert!(matches!(key(&mut a, "enter").as_slice(), [Effect::ResolveStreams { .. }, Effect::LoadRelated { .. }]));
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
        assert_eq!(a.tab, Tab::Feed);
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
        assert_eq!(
            key(&mut a, "enter"),
            [
                Effect::ResolveStreams { session: 1, id: "v1".into(), max_height: 480 },
                Effect::LoadRelated { session: 1, id: "v1".into() }
            ]
        );
        assert_eq!(key(&mut a, "q"), [Effect::Player(PlayerCommand::Stop)]);
        assert!(matches!(a.screens.last(), Some(Screen::Video(_))));
        key(&mut a, "esc");
        assert!(a.screens.is_empty());
        assert!(matches!(
            key(&mut a, "enter").as_slice(),
            [Effect::ResolveStreams { session: 2, .. }, Effect::LoadRelated { .. }]
        ));
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
        assert!(matches!(effects.as_slice(), [Effect::Subscribe(s), Effect::LoadFeed { channels, force: true }]
                if s.channel_id == "UC1" && s.handle.as_deref() == Some("@real") && channels == &[("UC1".to_string(), None)]));
        assert!(a.is_subscribed("UC1"));
        assert_eq!(
            key(&mut a, "s"),
            vec![Effect::Unsubscribe { channel_id: "UC1".into() }, Effect::ForgetFeed { channel_id: "UC1".into() }]
        );
        assert!(!a.is_subscribed("UC1"));
    }

    #[test]
    fn subscriptions_tab_opens_and_removes() {
        let mut a = app();
        a.subscriptions =
            vec![Subscription { channel_id: "UC1".into(), name: "A".into(), handle: None, subscribed_at: 1 }];
        a.tab = Tab::Subscriptions;
        assert_eq!(key(&mut a, "enter"), vec![Effect::LoadChannel { request: 1, id: "UC1".into() }]);
        key(&mut a, "esc");
        assert_eq!(
            key(&mut a, "s"),
            vec![Effect::Unsubscribe { channel_id: "UC1".into() }, Effect::ForgetFeed { channel_id: "UC1".into() }]
        );
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
        a.tab = Tab::Playlists;
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

    pub(crate) fn details(id: &str, comments: Option<&str>) -> VideoDetails {
        VideoDetails {
            id: id.into(),
            title: "Title".into(),
            channel: None,
            subscribers_text: None,
            views_text: None,
            date_text: None,
            publish_date: None,
            duration_secs: None,
            likes_text: None,
            live: false,
            description: crate::source::model::Description::default(),
            chapters: Vec::new(),
            thumbnails: Vec::new(),
            related: Vec::new(),
            comments: comments.map(str::to_string),
            comments_count_text: Some("10M".into()),
        }
    }

    pub(crate) fn comment(id: &str) -> crate::source::model::Comment {
        crate::source::model::Comment {
            id: id.into(),
            author: ChannelRef { id: Some(format!("UC{id}")), name: format!("@{id}"), handle: None },
            text: crate::source::model::Description { text: format!("Comment {id}"), links: Vec::new() },
            published_text: Some("1 day ago".into()),
            edited: false,
            likes_text: Some("5".into()),
            reply_count_text: Some("2".into()),
            replies: Some(format!("r-{id}")),
            pinned: false,
            hearted: false,
            by_uploader: false,
            verified: false,
            reply_level: u8::from(id.contains('.')),
        }
    }

    pub(crate) fn comment_page(ids: &[&str], continuation: Option<&str>) -> CommentPage {
        let sort = |label: &str, selected| crate::source::model::CommentSort {
            label: label.into(),
            selected,
            continuation: format!("sort-{label}"),
        };
        CommentPage {
            comments: ids.iter().map(|id| comment(id)).collect(),
            continuation: continuation.map(str::to_string),
            count_text: Some("12 Comments".into()),
            sorts: vec![sort("Top", true), sort("Newest", false)],
            note: None,
        }
    }

    fn comments_state(a: &App) -> &CommentsState {
        let Some(Screen::Video(s)) = a.screens.last() else { panic!("no video screen") };
        s.comments.as_ref().expect("comments")
    }

    fn pane(a: &App) -> VideoPane {
        let Some(Screen::Video(s)) = a.screens.last() else { panic!("no video screen") };
        s.pane
    }

    /// The generation of a `LoadComments` effect.
    fn loads(effects: &[Effect]) -> Vec<(u64, CommentsSlot, CommentsFrom)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::LoadComments { request: 1, generation, video_id, slot, from } if video_id == "v" => {
                    Some((*generation, slot.clone(), from.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn comments_open_from_the_details_token_or_the_video() {
        let mut a = app();
        a.open(Target::Video { id: "v".into(), title: "t".into() });
        a.update(Message::VideoLoaded { request: 1, result: Ok(details("v", Some("tok"))) });
        let got = loads(&key(&mut a, "C"));
        assert_eq!(got.len(), 1);
        assert_eq!((&got[0].1, &got[0].2), (&CommentsSlot::First, &CommentsFrom::Token("tok".into())));
        assert_eq!(pane(&a), VideoPane::Comments);
        // C goes back to the details, and again to the comments with no new load.
        assert!(key(&mut a, "C").is_empty());
        assert_eq!(pane(&a), VideoPane::Details);
        assert!(key(&mut a, "C").is_empty());
        assert_eq!(pane(&a), VideoPane::Comments);
        // Without a token, from the video. Before the details load too.
        let mut a = app();
        a.open(Target::Video { id: "v".into(), title: "t".into() });
        let got = loads(&key(&mut a, "C"));
        assert_eq!(got[0].2, CommentsFrom::Video("v".into()));
    }

    #[test]
    fn comments_page_reply_and_open_the_author() {
        let mut a = app();
        a.update(Message::Input(Input::Resize { width: 80, height: 24 }));
        a.open(Target::Video { id: "v".into(), title: "t".into() });
        let (generation, ..) = loads(&key(&mut a, "C"))[0].clone();
        let ids: Vec<String> = (0..8).map(|i| format!("c{i}")).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let page = comment_page(&ids, Some("p2"));
        let effects =
            a.update(Message::CommentsLoaded { request: 1, generation, slot: CommentsSlot::First, result: Ok(page) });
        assert!(effects.is_empty());
        assert_eq!(comments_state(&a).rows.len(), 8);
        // Moving down near the end loads the next page.
        key(&mut a, "j");
        key(&mut a, "j");
        let got = loads(&key(&mut a, "j"));
        assert_eq!(got, [(generation, CommentsSlot::More, CommentsFrom::Token("p2".into()))]);
        // Enter loads the replies of the selected comment.
        let got = loads(&key(&mut a, "enter"));
        assert_eq!(
            got,
            [(generation, CommentsSlot::Replies { parent: "c3".into() }, CommentsFrom::Token("r-c3".into()))]
        );
        let replies = comment_page(&["c3.a", "c3.b"], None);
        a.update(Message::CommentsLoaded {
            request: 1,
            generation,
            slot: CommentsSlot::Replies { parent: "c3".into() },
            result: Ok(replies),
        });
        assert_eq!(comments_state(&a).rows.len(), 10);
        // c opens the author of the selected comment.
        let effects = key(&mut a, "c");
        assert!(matches!(effects.as_slice(), [Effect::LoadChannel { id, .. }] if id == "UCc3"), "{effects:?}");
    }

    #[test]
    fn sort_and_refresh_start_over_and_drop_old_pages() {
        let mut a = app();
        a.open(Target::Video { id: "v".into(), title: "t".into() });
        let (first, ..) = loads(&key(&mut a, "C"))[0].clone();
        a.update(Message::CommentsLoaded {
            request: 1,
            generation: first,
            slot: CommentsSlot::First,
            result: Ok(comment_page(&["a", "b"], Some("p2"))),
        });
        let got = loads(&key(&mut a, "o"));
        let (newest, slot, from) = got[0].clone();
        assert_eq!((slot, from), (CommentsSlot::First, CommentsFrom::Token("sort-Newest".into())));
        assert!(newest > first);
        assert_eq!(comments_state(&a).sort().unwrap().label, "Newest");
        assert!(comments_state(&a).rows.is_empty());
        // A late page for the old order is dropped.
        a.update(Message::CommentsLoaded {
            request: 1,
            generation: first,
            slot: CommentsSlot::More,
            result: Ok(comment_page(&["old"], None)),
        });
        assert!(comments_state(&a).rows.is_empty());
        // r reloads in the order that shows.
        let got = loads(&key(&mut a, "r"));
        assert_eq!(got[0].2, CommentsFrom::Token("sort-Newest".into()));
        assert!(matches!(a.screens.last(), Some(Screen::Video(_))), "only the comments reload");
        // An error shows in the status bar.
        let generation = got[0].0;
        a.update(Message::CommentsLoaded {
            request: 1,
            generation,
            slot: CommentsSlot::First,
            result: Err("down".into()),
        });
        assert_eq!(comments_state(&a).error.as_deref(), Some("down"));
    }

    #[test]
    fn back_leaves_the_comments_first() {
        let mut a = app();
        a.open(Target::Video { id: "v".into(), title: "t".into() });
        key(&mut a, "C");
        key(&mut a, "esc");
        assert_eq!(pane(&a), VideoPane::Details);
        key(&mut a, "esc");
        assert!(a.screens.is_empty());
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

    #[test]
    fn the_queue_tab_adds_plays_and_removes() {
        let mut a = app();
        results(&mut a, vec![video("v1"), video("v2"), channel_item("UC1")]);
        assert!(key(&mut a, "a").is_empty());
        assert_eq!(a.status.as_deref(), Some("Added to the queue. 1 in the queue."));
        key(&mut a, "j");
        key(&mut a, "a");
        key(&mut a, "j");
        key(&mut a, "a");
        assert_eq!(a.status.as_deref(), Some("Only videos can go in the queue."));
        key(&mut a, "k");
        key(&mut a, "a");
        assert_eq!(a.status.as_deref(), Some("Removed from the queue. 1 left."));
        key(&mut a, "a");
        // The Queue tab.
        key(&mut a, "tab");
        a.tab = Tab::Queue;
        assert_eq!(a.queue.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["v1", "v2"]);
        key(&mut a, "j");
        let effects = key(&mut a, "enter");
        assert!(
            matches!(effects.as_slice(), [Effect::ResolveStreams { id, .. }, Effect::LoadRelated { .. }] if id == "v2")
        );
        assert_eq!(a.queue.len(), 1);
        key(&mut a, "q");
        key(&mut a, "a");
        assert!(a.queue.is_empty());
    }

    #[test]
    fn queue_keeps_a_video_that_cannot_play_and_details_can_queue() {
        let mut a = App::new(KeyMap::default(), Graphics::Unavailable("tmux".into()));
        a.queue = vec![match video("v1") {
            Item::Video(v) => v,
            _ => unreachable!(),
        }];
        a.tab = Tab::Queue;
        assert!(key(&mut a, "enter").is_empty());
        assert_eq!(a.queue.len(), 1);

        let mut a = app();
        a.open(Target::Video { id: "v9".into(), title: "Video v9".into() });
        key(&mut a, "a");
        assert_eq!(a.queue.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["v9"]);
    }

    #[test]
    fn the_history_tab_plays_queues_and_deletes() {
        let mut a = app();
        a.history = ["h1", "h2"]
            .iter()
            .map(|id| HistoryEntry {
                video_id: (*id).into(),
                title: format!("Video {id}"),
                channel_id: Some("UC1".into()),
                channel_name: Some("C".into()),
                duration_secs: Some(600),
                position_secs: 0,
                finished: true,
                watched_at: 1,
            })
            .collect();
        a.tab = Tab::History;
        key(&mut a, "j");
        assert_eq!(key(&mut a, "d"), [Effect::ForgetWatch { video_id: "h2".into() }]);
        assert_eq!(a.history.len(), 1);
        assert_eq!(a.history_selection.index, 0);
        key(&mut a, "a");
        assert_eq!(a.queue.len(), 1);
        assert!(matches!(key(&mut a, "c").as_slice(), [Effect::LoadChannel { id, .. }] if id == "UC1"));
        key(&mut a, "esc");
        let effects = key(&mut a, "enter");
        assert!(
            matches!(effects.as_slice(), [Effect::ResolveStreams { id, .. }, Effect::LoadRelated { .. }] if id == "h1")
        );
    }

    #[test]
    fn the_history_selection_follows_its_video() {
        let mut a = app();
        let entry = |id: &str| HistoryEntry {
            video_id: id.into(),
            title: id.into(),
            channel_id: None,
            channel_name: None,
            duration_secs: None,
            position_secs: 0,
            finished: true,
            watched_at: 1,
        };
        a.history = vec![entry("a"), entry("b"), entry("c")];
        a.history_selection.index = 1;
        a.remember_watch(&entry("c"));
        assert_eq!(a.history[a.history_selection.index].video_id, "b");
        a.remember_watch(&entry("b"));
        assert_eq!(a.history_selection.index, 0);
    }

    fn feed_video(id: &str, ago: &str) -> Video {
        match video(id) {
            Item::Video(mut v) => {
                v.published_text = Some(ago.into());
                v
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_feed_loads_at_start_and_on_refresh() {
        let mut a = app();
        assert!(a.start().is_empty(), "no subscriptions, nothing to load");
        a.subscriptions = ["UC1", "UC2"]
            .iter()
            .map(|id| Subscription { channel_id: (*id).into(), name: (*id).into(), handle: None, subscribed_at: 1 })
            .collect();
        a.feed.fetched.insert("UC1".into(), 500);
        let effects = a.start();
        let want = vec![("UC1".to_string(), Some(500)), ("UC2".to_string(), None)];
        assert_eq!(effects, [Effect::LoadFeed { channels: want.clone(), force: false }]);

        a.update(Message::FeedLoading { count: 2 });
        let effects = a.update(Message::FeedChannelLoaded {
            channel_id: "UC2".into(),
            fetched_at: 1_000_000,
            result: Ok(vec![feed_video("b1", "1 hour ago"), feed_video("b2", "2 days ago")]),
        });
        assert!(
            matches!(effects.as_slice(), [Effect::SaveFeed { channel_id, videos, .. }] if channel_id == "UC2" && videos.len() == 2)
        );
        a.update(Message::FeedChannelLoaded {
            channel_id: "UC1".into(),
            fetched_at: 1_000_000,
            result: Err("HTTP 500".into()),
        });
        assert_eq!(a.feed.loading, 0);
        assert_eq!(a.feed.error.as_deref(), Some("UC1: HTTP 500"));

        a.tab = Tab::Feed;
        assert_eq!(a.feed.items.len(), 2);
        let mut fetched = want;
        fetched[1].1 = Some(1_000_000);
        assert_eq!(key(&mut a, "r"), [Effect::LoadFeed { channels: fetched, force: true }]);
        // Enter plays, and the list moves with the browse keys.
        key(&mut a, "j");
        assert!(
            matches!(key(&mut a, "enter").as_slice(), [Effect::ResolveStreams { id, .. }, Effect::LoadRelated { .. }] if id == "b2")
        );
    }

    #[test]
    fn unsubscribing_takes_the_channel_out_of_the_feed() {
        let mut a = app();
        a.subscriptions =
            vec![Subscription { channel_id: "UC1".into(), name: "A".into(), handle: None, subscribed_at: 1 }];
        a.update(Message::FeedChannelLoaded {
            channel_id: "UC1".into(),
            fetched_at: 1_000_000,
            result: Ok(vec![feed_video("a1", "1 hour ago")]),
        });
        assert_eq!(a.feed.items.len(), 1);
        a.tab = Tab::Subscriptions;
        key(&mut a, "s");
        assert!(a.feed.items.is_empty());
    }

    #[test]
    fn late_replies_and_refresh_while_loading() {
        let mut a = app();
        a.subscriptions =
            vec![Subscription { channel_id: "UC1".into(), name: "A".into(), handle: None, subscribed_at: 1 }];
        a.tab = Tab::Feed;
        a.update(Message::FeedLoading { count: 1 });
        assert!(key(&mut a, "r").is_empty());
        assert_eq!(a.status.as_deref(), Some("The feed is loading already."));
        // Unsubscribed while it loaded: the reply is dropped.
        a.subscriptions.clear();
        let effects = a.update(Message::FeedChannelLoaded {
            channel_id: "UC1".into(),
            fetched_at: 1_000_000,
            result: Ok(vec![feed_video("a1", "1 hour ago")]),
        });
        assert!(effects.is_empty());
        assert!(a.feed.fetched.is_empty());
        assert_eq!(a.feed.loading, 0);
    }
}
