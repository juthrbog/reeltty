//! Key notation, actions, and the key map.
//!
//! Keys are written as strings in the configuration file, for example `"j"`,
//! `"G"`, `"ctrl-d"`, `"space"`, or `"shift-tab"`. Each action has a list of
//! keys. A list in the configuration file replaces the default list for that
//! action.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
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
    Help,
}

impl Action {
    pub const ALL: [Self; 18] = [
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
            Self::Help => &["?"],
        }
    }
}

/// The `[keys.browse]` table: action name to a list of keys.
pub type KeyOverrides = BTreeMap<Action, Vec<String>>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyMapError {
    #[error("[keys.browse] {action:?}: {source}")]
    Parse { action: Action, source: KeyParseError },
    #[error("[keys.browse] key {key} is bound to both {first:?} and {second:?}")]
    Conflict { key: String, first: Action, second: Action },
    #[error("[keys.browse] quit must have at least one key")]
    NoQuitKey,
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    lookup: HashMap<KeyPress, Action>,
    bindings: Vec<(Action, Vec<KeyPress>)>,
}

impl KeyMap {
    /// Builds the key map from the defaults and the overrides in the configuration file.
    pub fn new(overrides: &KeyOverrides) -> Result<Self, KeyMapError> {
        let mut lookup = HashMap::new();
        let mut bindings = Vec::new();
        for action in Action::ALL {
            let mut keys: Vec<KeyPress> = match overrides.get(&action) {
                Some(list) => list
                    .iter()
                    .map(|s| s.parse().map_err(|source| KeyMapError::Parse { action, source }))
                    .collect::<Result<_, _>>()?,
                None => action.default_keys().iter().map(|s| s.parse().expect("default keys parse")).collect(),
            };
            if action == Action::Quit && keys.is_empty() {
                return Err(KeyMapError::NoQuitKey);
            }
            let mut seen = HashSet::new();
            keys.retain(|k| seen.insert(*k));
            for &k in &keys {
                if let Some(first) = lookup.insert(k, action) {
                    return Err(KeyMapError::Conflict { key: k.to_string(), first, second: action });
                }
            }
            bindings.push((action, keys));
        }
        Ok(Self { lookup, bindings })
    }

    pub fn action(&self, key: KeyPress) -> Option<Action> {
        self.lookup.get(&key).copied()
    }

    /// Every action with its keys, in a fixed order, for the help screen.
    pub fn bindings(&self) -> &[(Action, Vec<KeyPress>)] {
        &self.bindings
    }

    /// The first key of an action, for short hints in the status bar.
    pub fn first_key(&self, action: Action) -> Option<KeyPress> {
        self.bindings.iter().find(|(a, _)| *a == action).and_then(|(_, k)| k.first().copied())
    }
}

impl Default for KeyMap {
    fn default() -> Self {
        Self::new(&KeyOverrides::new()).expect("default key map has no conflicts")
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
        assert_eq!(km.action(key("x")), None);
    }

    #[test]
    fn override_replaces_default_list() {
        let o = KeyOverrides::from([(Action::Down, vec!["n".to_string()])]);
        let km = KeyMap::new(&o).unwrap();
        assert_eq!(km.action(key("n")), Some(Action::Down));
        assert_eq!(km.action(key("j")), None);
    }

    #[test]
    fn quit_needs_a_key() {
        let o = KeyOverrides::from([(Action::Quit, vec![])]);
        assert_eq!(KeyMap::new(&o).unwrap_err(), KeyMapError::NoQuitKey);
    }

    #[test]
    fn repeated_key_in_one_list_is_fine() {
        let o = KeyOverrides::from([(Action::Down, vec!["n".to_string(), "n".to_string()])]);
        let km = KeyMap::new(&o).unwrap();
        assert_eq!(km.action(key("n")), Some(Action::Down));
        let (_, keys) = km.bindings().iter().find(|(a, _)| *a == Action::Down).unwrap();
        assert_eq!(keys, &[key("n")]);
    }

    #[test]
    fn conflict_is_an_error() {
        let o = KeyOverrides::from([(Action::Down, vec!["k".to_string()])]);
        let err = KeyMap::new(&o).unwrap_err();
        assert_eq!(err, KeyMapError::Conflict { key: "k".into(), first: Action::Up, second: Action::Down });
    }
}
