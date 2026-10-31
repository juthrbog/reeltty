//! Key notation, actions, and the key map.
//!
//! Keys are written as strings in the configuration file, for example `"j"`,
//! `"G"`, `"ctrl-d"`, `"space"`, or `"shift-tab"`. Each action has a list of
//! keys. A list in the configuration file replaces the default list for that
//! action.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::str::FromStr;

use serde::Deserialize;

/// A key press, without terminal-specific types.
///
/// Shift is not a separate flag. It is part of the character (`G`), or of
/// the key itself (`BackTab`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyPress {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    F(u8),
}

impl KeyPress {
    pub const fn plain(code: KeyCode) -> Self {
        Self { code, ctrl: false, alt: false }
    }

    pub const fn ctrl(c: char) -> Self {
        Self { code: KeyCode::Char(c), ctrl: true, alt: false }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown key {0:?}")]
pub struct KeyParseError(String);

impl FromStr for KeyPress {
    type Err = KeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || KeyParseError(s.to_string());
        let mut rest = s;
        let (mut ctrl, mut alt) = (false, false);
        // A single character is always a plain key, so "-" works.
        while rest.chars().count() > 1 {
            if let Some(r) = rest.strip_prefix("ctrl-") {
                ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("alt-") {
                alt = true;
                rest = r;
            } else {
                break;
            }
        }
        let mut chars = rest.chars();
        let code = match (chars.next(), chars.next()) {
            (Some(c), None) => KeyCode::Char(c),
            _ => match rest {
                "space" => KeyCode::Char(' '),
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "shift-tab" => KeyCode::BackTab,
                "backspace" => KeyCode::Backspace,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "pgup" => KeyCode::PageUp,
                "pgdn" => KeyCode::PageDown,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                f => {
                    let n: u8 = f.strip_prefix('f').and_then(|n| n.parse().ok()).ok_or_else(err)?;
                    if !(1..=12).contains(&n) {
                        return Err(err());
                    }
                    KeyCode::F(n)
                }
            },
        };
        // Terminals send ctrl-i as tab, ctrl-m as enter, ctrl-[ as esc, and
        // ctrl with an upper-case letter as the lower-case one. Such a binding
        // would never fire.
        if ctrl
            && let KeyCode::Char(c) = code
            && (c.is_ascii_uppercase() || matches!(c, 'i' | 'm' | '['))
        {
            return Err(err());
        }
        Ok(Self { code, ctrl, alt })
    }
}

impl fmt::Display for KeyPress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("ctrl-")?;
        }
        if self.alt {
            f.write_str("alt-")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("shift-tab"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Up => f.write_str("up"),
            KeyCode::Down => f.write_str("down"),
            KeyCode::Left => f.write_str("left"),
            KeyCode::Right => f.write_str("right"),
            KeyCode::PageUp => f.write_str("pgup"),
            KeyCode::PageDown => f.write_str("pgdn"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::F(n) => write!(f, "f{n}"),
        }
    }
}

/// Actions on the browse screen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Quit,
    Up,
    Down,
    Top,
    Bottom,
    HalfPageUp,
    HalfPageDown,
    Open,
    Back,
    Search,
    Filters,
    NextTab,
    PrevTab,
    Refresh,
    Details,
    Channel,
    Toggle,
    Enqueue,
    Delete,
    ShowPlayer,
    ClosePlayer,
    Help,
}

impl Action {
    pub const ALL: [Self; 22] = [
        Self::Quit,
        Self::Up,
        Self::Down,
        Self::Top,
        Self::Bottom,
        Self::HalfPageUp,
        Self::HalfPageDown,
        Self::Open,
        Self::Back,
        Self::Search,
        Self::Filters,
        Self::NextTab,
        Self::PrevTab,
        Self::Refresh,
        Self::Details,
        Self::Channel,
        Self::Toggle,
        Self::Enqueue,
        Self::Delete,
        Self::ShowPlayer,
        Self::ClosePlayer,
        Self::Help,
    ];

    pub const fn description(self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::Up => "move up",
            Self::Down => "move down",
            Self::Top => "go to the top",
            Self::Bottom => "go to the bottom",
            Self::HalfPageUp => "half a page up",
            Self::HalfPageDown => "half a page down",
            Self::Open => "open",
            Self::Back => "go back",
            Self::Search => "search",
            Self::Filters => "search filters",
            Self::NextTab => "next tab",
            Self::PrevTab => "previous tab",
            Self::Refresh => "refresh",
            Self::Details => "video details",
            Self::Channel => "go to the channel",
            Self::Toggle => "subscribe or save",
            Self::Enqueue => "add to the queue",
            Self::Delete => "delete an entry",
            Self::ShowPlayer => "full-size player",
            Self::ClosePlayer => "stop mini-player",
            Self::Help => "show or hide help",
        }
    }

    fn default_keys(self) -> &'static [&'static str] {
        match self {
            Self::Quit => &["q", "ctrl-c"],
            Self::Up => &["k", "up"],
            Self::Down => &["j", "down"],
            Self::Top => &["g", "home"],
            Self::Bottom => &["G", "end"],
            Self::HalfPageUp => &["ctrl-u", "pgup"],
            Self::HalfPageDown => &["ctrl-d", "pgdn"],
            Self::Open => &["enter", "l"],
            Self::Back => &["esc", "h"],
            Self::Search => &["/"],
            Self::Filters => &["f"],
            Self::NextTab => &["tab"],
            Self::PrevTab => &["shift-tab"],
            Self::Refresh => &["r"],
            Self::Details => &["i"],
            Self::Channel => &["c"],
            Self::Toggle => &["s"],
            Self::Enqueue => &["a"],
            Self::Delete => &["d"],
            Self::ShowPlayer => &["v"],
            Self::ClosePlayer => &["x"],
            Self::Help => &["?"],
        }
    }
}

/// Actions in the player.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerAction {
    Back,
    Minimize,
    TogglePause,
    SeekForward,
    SeekBack,
    SeekForwardLong,
    SeekBackLong,
    VolumeUp,
    VolumeDown,
    Mute,
    SpeedUp,
    SpeedDown,
    SpeedReset,
    NextChapter,
    PrevChapter,
    Captions,
    Quality,
    NextVideo,
    PrevVideo,
}

impl PlayerAction {
    pub const ALL: [Self; 19] = [
        Self::Back,
        Self::Minimize,
        Self::TogglePause,
        Self::SeekForward,
        Self::SeekBack,
        Self::SeekForwardLong,
        Self::SeekBackLong,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::Mute,
        Self::SpeedUp,
        Self::SpeedDown,
        Self::SpeedReset,
        Self::NextChapter,
        Self::PrevChapter,
        Self::Captions,
        Self::Quality,
        Self::NextVideo,
        Self::PrevVideo,
    ];

    pub const fn description(self) -> &'static str {
        match self {
            Self::Back => "stop and go back",
            Self::Minimize => "mini-player",
            Self::TogglePause => "pause or play",
            Self::SeekForward => "forward 5 s",
            Self::SeekBack => "back 5 s",
            Self::SeekForwardLong => "forward 1 min",
            Self::SeekBackLong => "back 1 min",
            Self::VolumeUp => "volume up",
            Self::VolumeDown => "volume down",
            Self::Mute => "mute or unmute",
            Self::SpeedUp => "faster",
            Self::SpeedDown => "slower",
            Self::SpeedReset => "normal speed",
            Self::NextChapter => "next chapter",
            Self::PrevChapter => "previous chapter",
            Self::Captions => "captions on or off",
            Self::Quality => "video quality",
            Self::NextVideo => "next video",
            Self::PrevVideo => "previous video",
        }
    }

    /// The same keys as mpv.
    fn default_keys(self) -> &'static [&'static str] {
        match self {
            // mpv leaves full screen with esc and quits with q.
            Self::Back => &["q"],
            Self::Minimize => &["esc"],
            Self::TogglePause => &["space", "p"],
            Self::SeekForward => &["right"],
            Self::SeekBack => &["left"],
            Self::SeekForwardLong => &["up"],
            Self::SeekBackLong => &["down"],
            Self::VolumeUp => &["0", "*"],
            Self::VolumeDown => &["9", "/"],
            Self::Mute => &["m"],
            Self::SpeedUp => &["]"],
            Self::SpeedDown => &["["],
            Self::SpeedReset => &["backspace"],
            Self::NextChapter => &["pgdn"],
            Self::PrevChapter => &["pgup"],
            Self::Captions => &["c"],
            // mpv has no quality key.
            Self::Quality => &["v"],
            // mpv's playlist keys.
            Self::NextVideo => &[">"],
            Self::PrevVideo => &["<"],
        }
    }
}

/// The actions of one table in `[keys]`.
pub trait Actions: Copy + Eq + Hash + Ord + fmt::Debug + 'static {
    /// The table name, for error messages.
    const TABLE: &'static str;
    /// The action that must keep at least one key, so the user can never get stuck.
    const REQUIRED: Self;
    fn all() -> &'static [Self];
    fn default_keys(self) -> &'static [&'static str];
}

impl Actions for Action {
    const TABLE: &'static str = "[keys.browse]";
    const REQUIRED: Self = Self::Quit;

    fn all() -> &'static [Self] {
        &Self::ALL
    }

    fn default_keys(self) -> &'static [&'static str] {
        Self::default_keys(self)
    }
}

impl Actions for PlayerAction {
    const TABLE: &'static str = "[keys.player]";
    const REQUIRED: Self = Self::Back;

    fn all() -> &'static [Self] {
        &Self::ALL
    }

    fn default_keys(self) -> &'static [&'static str] {
        Self::default_keys(self)
    }
}

/// A `[keys.*]` table: action name to a list of keys.
pub type Overrides<A> = BTreeMap<A, Vec<String>>;
/// The `[keys.browse]` table.
pub type KeyOverrides = Overrides<Action>;
/// The `[keys.player]` table.
pub type PlayerKeyOverrides = Overrides<PlayerAction>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyMapError {
    #[error("{table} {action}: {source}")]
    Parse { table: &'static str, action: String, source: KeyParseError },
    #[error("{table} key {key} is bound to both {first} and {second}")]
    Conflict { table: &'static str, key: String, first: String, second: String },
    #[error("{table} {action} must have at least one key")]
    NoKey { table: &'static str, action: String },
}

/// The keys of one table. A key has at most one action in a table. The same
/// key can mean different things in different tables.
#[derive(Debug, Clone)]
pub struct Bindings<A> {
    lookup: HashMap<KeyPress, A>,
    bindings: Vec<(A, Vec<KeyPress>)>,
}

/// The action name as the configuration file writes it.
fn name(action: impl fmt::Debug) -> String {
    let debug = format!("{action:?}");
    let mut out = String::new();
    for (i, c) in debug.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

impl<A: Actions> Bindings<A> {
    /// Builds the table from the defaults and the overrides in the configuration file.
    pub fn new(overrides: &Overrides<A>) -> Result<Self, KeyMapError> {
        let mut lookup = HashMap::new();
        let mut bindings = Vec::new();
        for &action in A::all() {
            let mut keys: Vec<KeyPress> = match overrides.get(&action) {
                Some(list) => list
                    .iter()
                    .map(|s| {
                        s.parse().map_err(|source| KeyMapError::Parse { table: A::TABLE, action: name(action), source })
                    })
                    .collect::<Result<_, _>>()?,
                None => action.default_keys().iter().map(|s| s.parse().expect("default keys parse")).collect(),
            };
            if action == A::REQUIRED && keys.is_empty() {
                return Err(KeyMapError::NoKey { table: A::TABLE, action: name(action) });
            }
            let mut seen = HashSet::new();
            keys.retain(|k| seen.insert(*k));
            for &k in &keys {
                if let Some(first) = lookup.insert(k, action) {
                    return Err(KeyMapError::Conflict {
                        table: A::TABLE,
                        key: k.to_string(),
                        first: name(first),
                        second: name(action),
                    });
                }
            }
            bindings.push((action, keys));
        }
        Ok(Self { lookup, bindings })
    }

    pub fn action(&self, key: KeyPress) -> Option<A> {
        self.lookup.get(&key).copied()
    }

    /// Every action with its keys, in a fixed order, for the help screen.
    pub fn bindings(&self) -> &[(A, Vec<KeyPress>)] {
        &self.bindings
    }

    /// The first key of an action, for short hints in the status bar.
    pub fn first_key(&self, action: A) -> Option<KeyPress> {
        self.bindings.iter().find(|(a, _)| *a == action).and_then(|(_, k)| k.first().copied())
    }
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    browse: Bindings<Action>,
    pub player: Bindings<PlayerAction>,
}

impl KeyMap {
    /// Builds the key map from the defaults and the overrides in the configuration file.
    pub fn new(browse: &KeyOverrides, player: &PlayerKeyOverrides) -> Result<Self, KeyMapError> {
        Ok(Self { browse: Bindings::new(browse)?, player: Bindings::new(player)? })
    }

    /// The browse action of a key.
    pub fn action(&self, key: KeyPress) -> Option<Action> {
        self.browse.action(key)
    }

    /// Every browse action with its keys, in a fixed order, for the help screen.
    pub fn bindings(&self) -> &[(Action, Vec<KeyPress>)] {
        self.browse.bindings()
    }

    /// The first key of a browse action, for short hints in the status bar.
    pub fn first_key(&self, action: Action) -> Option<KeyPress> {
        self.browse.first_key(action)
    }
}

impl Default for KeyMap {
    fn default() -> Self {
        Self::new(&KeyOverrides::new(), &PlayerKeyOverrides::new()).expect("default key map has no conflicts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> KeyPress {
        s.parse().unwrap()
    }

    #[test]
    fn parses_notation() {
        assert_eq!(key("j"), KeyPress::plain(KeyCode::Char('j')));
        assert_eq!(key("G"), KeyPress::plain(KeyCode::Char('G')));
        assert_eq!(key("-"), KeyPress::plain(KeyCode::Char('-')));
        assert_eq!(key("ctrl-d"), KeyPress::ctrl('d'));
        assert_eq!(key("ctrl-alt-x"), KeyPress { code: KeyCode::Char('x'), ctrl: true, alt: true });
        assert_eq!(key("space"), KeyPress::plain(KeyCode::Char(' ')));
        assert_eq!(key("shift-tab"), KeyPress::plain(KeyCode::BackTab));
        assert_eq!(key("f12"), KeyPress::plain(KeyCode::F(12)));
        assert!("f13".parse::<KeyPress>().is_err());
        assert!("ctrl-".parse::<KeyPress>().is_err());
        assert!("bogus".parse::<KeyPress>().is_err());
        for unsendable in ["ctrl-i", "ctrl-m", "ctrl-[", "ctrl-G"] {
            assert!(unsendable.parse::<KeyPress>().is_err(), "{unsendable}");
        }
    }

    #[test]
    fn display_round_trips() {
        for s in ["j", "G", "ctrl-d", "alt-x", "space", "enter", "shift-tab", "pgdn", "f5", "/"] {
            assert_eq!(key(s).to_string(), s);
        }
    }

    #[test]
    fn defaults_have_no_conflicts() {
        let km = KeyMap::default();
        assert_eq!(km.action(key("j")), Some(Action::Down));
        assert_eq!(km.action(key("ctrl-c")), Some(Action::Quit));
        assert_eq!(km.action(key("x")), Some(Action::ClosePlayer));
        assert_eq!(km.action(key("v")), Some(Action::ShowPlayer));
        assert_eq!(km.action(key("z")), None);
    }

    #[test]
    fn override_replaces_default_list() {
        let o = KeyOverrides::from([(Action::Down, vec!["n".to_string()])]);
        let km = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap();
        assert_eq!(km.action(key("n")), Some(Action::Down));
        assert_eq!(km.action(key("j")), None);
    }

    #[test]
    fn quit_needs_a_key() {
        let o = KeyOverrides::from([(Action::Quit, vec![])]);
        let err = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap_err();
        assert_eq!(err, KeyMapError::NoKey { table: "[keys.browse]", action: "quit".into() });
        let o = PlayerKeyOverrides::from([(PlayerAction::Back, vec![])]);
        let err = KeyMap::new(&KeyOverrides::new(), &o).unwrap_err();
        assert_eq!(err.to_string(), "[keys.player] back must have at least one key");
    }

    #[test]
    fn repeated_key_in_one_list_is_fine() {
        let o = KeyOverrides::from([(Action::Down, vec!["n".to_string(), "n".to_string()])]);
        let km = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap();
        assert_eq!(km.action(key("n")), Some(Action::Down));
        let (_, keys) = km.bindings().iter().find(|(a, _)| *a == Action::Down).unwrap();
        assert_eq!(keys, &[key("n")]);
    }

    #[test]
    fn conflict_is_an_error() {
        let o = KeyOverrides::from([(Action::Down, vec!["k".to_string()])]);
        let err = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap_err();
        assert_eq!(
            err,
            KeyMapError::Conflict {
                table: "[keys.browse]",
                key: "k".into(),
                first: "up".into(),
                second: "down".into()
            }
        );
    }

    #[test]
    fn player_keys_are_a_separate_table() {
        let km = KeyMap::default();
        // q quits the app in browse, and stops the video in the player.
        assert_eq!(km.action(key("q")), Some(Action::Quit));
        assert_eq!(km.player.action(key("q")), Some(PlayerAction::Back));
        assert_eq!(km.player.action(key("space")), Some(PlayerAction::TogglePause));
        assert_eq!(km.player.action(key("j")), None);
        assert_eq!(km.player.action(key("esc")), Some(PlayerAction::Minimize));

        let o = PlayerKeyOverrides::from([(PlayerAction::Mute, vec!["space".to_string()])]);
        let err = KeyMap::new(&KeyOverrides::new(), &o).unwrap_err();
        assert_eq!(err.to_string(), "[keys.player] key space is bound to both toggle_pause and mute");
        let o = PlayerKeyOverrides::from([(PlayerAction::SeekForwardLong, vec!["l".to_string()])]);
        let km = KeyMap::new(&KeyOverrides::new(), &o).unwrap();
        assert_eq!(km.player.action(key("l")), Some(PlayerAction::SeekForwardLong));
        assert_eq!(km.player.action(key("up")), None);
    }
}
