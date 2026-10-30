//! The app state and its update function (Elm style).
//!
//! All events arrive as `Message` values. `App::update` changes the state and
//! returns `Effect` values. The runtime carries out the effects, for example
//! by starting a task that later sends another `Message`. `update` never
//! blocks and never does I/O.

use crate::keymap::{Action, KeyMap, KeyPress};
use crate::terminal::detect::Graphics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Input(Input),
    /// The input source stopped. The text says why.
    InputClosed(String),
    /// SIGTERM or SIGHUP.
    Terminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Key(KeyPress),
    Resize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Quit,
    /// Quit with an error.
    Fail(String),
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

/// The selected row in a list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub index: usize,
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
        true
    }
}

#[derive(Debug, Clone)]
pub struct App {
    pub keymap: KeyMap,
    pub graphics: Graphics,
    pub tab: Tab,
    pub show_help: bool,
}

impl App {
    pub fn new(keymap: KeyMap, graphics: Graphics) -> Self {
        Self { keymap, graphics, tab: Tab::Search, show_help: false }
    }

    #[allow(clippy::needless_pass_by_value, reason = "later messages carry data that update takes")]
    pub fn update(&mut self, msg: Message) -> Vec<Effect> {
        match msg {
            Message::Input(Input::Resize) => Vec::new(),
            Message::Input(Input::Key(key)) => self.on_key(key),
            Message::InputClosed(why) => vec![Effect::Fail(format!("terminal input stopped: {why}"))],
            Message::Terminate => vec![Effect::Quit],
        }
    }

    fn on_key(&mut self, key: KeyPress) -> Vec<Effect> {
        let Some(action) = self.keymap.action(key) else {
            return Vec::new();
        };
        if self.show_help {
            match action {
                Action::Help | Action::Back => self.show_help = false,
                Action::Quit => return vec![Effect::Quit],
                _ => {}
            }
            return Vec::new();
        }
        match action {
            Action::Quit => return vec![Effect::Quit],
            Action::Help => self.show_help = true,
            Action::NextTab => self.tab = self.tab.offset(1),
            Action::PrevTab => self.tab = self.tab.offset(-1),
            _ => {}
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(KeyMap::default(), Graphics::KittyShm)
    }

    fn key(app: &mut App, s: &str) -> Vec<Effect> {
        app.update(Message::Input(Input::Key(s.parse().unwrap())))
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
    fn selection_stays_in_bounds() {
        let mut s = Selection::default();
        assert!(s.apply(Action::Up, 10, 6));
        assert_eq!(s.index, 0);
        s.apply(Action::HalfPageDown, 10, 6);
        assert_eq!(s.index, 3);
        s.apply(Action::Bottom, 10, 6);
        assert_eq!(s.index, 9);
        s.apply(Action::Down, 10, 6);
        assert_eq!(s.index, 9);
        s.apply(Action::HalfPageUp, 10, 6);
        assert_eq!(s.index, 6);
        s.apply(Action::Top, 0, 6);
        assert_eq!(s.index, 0);
        s.apply(Action::Bottom, 0, 6);
        assert_eq!(s.index, 0);
        assert!(!s.apply(Action::Open, 10, 6));
    }
}
