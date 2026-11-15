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
    Comments,
    Sort,
    NextLink,
    PrevLink,
    Help,
    TabSearch,
    TabFeed,
    TabSubscriptions,
    TabPlaylists,
    TabQueue,
    TabHistory,
    TabDownloads,
    Download,
}

impl Action {
    pub const ALL: [Self; 34] = [
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
        Self::Comments,
        Self::Sort,
        Self::NextLink,
        Self::PrevLink,
        Self::Help,
        Self::TabSearch,
        Self::TabFeed,
        Self::TabSubscriptions,
        Self::TabPlaylists,
        Self::TabQueue,
        Self::TabHistory,
        Self::TabDownloads,
        Self::Download,
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
            Self::Delete => "remove an entry",
            Self::ShowPlayer => "full-size player",
            Self::ClosePlayer => "stop mini-player",
            Self::Comments => "comments",
            Self::Sort => "sort comments",
            Self::NextLink => "next link",
            Self::PrevLink => "previous link",
            Self::Help => "show or hide help",
            Self::TabSearch => "Search tab",
            Self::TabFeed => "Feed tab",
            Self::TabSubscriptions => "Subscriptions tab",
            Self::TabPlaylists => "Playlists tab",
            Self::TabQueue => "Queue tab",
            Self::TabHistory => "History tab",
            Self::TabDownloads => "Downloads tab",
            Self::Download => "download or stop",
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
            Self::Comments => &["C"],
            Self::Sort => &["o"],
            Self::NextLink => &["n"],
            Self::PrevLink => &["N"],
            Self::Help => &["?"],
            // In the order of the tab bar.
            Self::TabSearch => &["1"],
            Self::TabFeed => &["2"],
            Self::TabSubscriptions => &["3"],
            Self::TabPlaylists => &["4"],
            Self::TabQueue => &["5"],
            Self::TabHistory => &["6"],
            Self::TabDownloads => &["7"],
            Self::Download => &["D"],
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
    Help,
}

impl PlayerAction {
    pub const ALL: [Self; 20] = [
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
        Self::Help,
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
            Self::Help => "show or hide help",
        }
    }

    /// Mostly the same keys as mpv. The differences: `/` is not a volume key,
    /// because it is search in browse, and `Q` is the quality menu. In mpv,
    /// `Q` quits and keeps the place, which reeltty always does.
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
            Self::VolumeDown => &["9"],
            Self::Mute => &["m"],
            Self::SpeedUp => &["]"],
            Self::SpeedDown => &["["],
            Self::SpeedReset => &["backspace"],
            Self::NextChapter => &["pgdn"],
            Self::PrevChapter => &["pgup"],
            // mpv shows or hides subtitles with v.
            Self::Captions => &["v"],
            // mpv has no quality key.
            Self::Quality => &["Q"],
            // mpv's playlist keys.
            Self::NextVideo => &[">"],
            Self::PrevVideo => &["<"],
            // mpv shows its keys with ? too.
            Self::Help => &["?"],
        }
    }
}

/// The actions of one table in `[keys]`.
pub trait Actions: Copy + Eq + Hash + Ord + fmt::Debug + 'static {
    /// The table name, for error messages.
    const TABLE: &'static str;
    /// The action that must keep at least one key, so the user can never get stuck.
    const REQUIRED: Self;
    /// The action that has ctrl-c. ctrl-c always quits, so no other action
    /// can have it, and this one always has it.
    const CTRL_C: Option<Self>;
    fn all() -> &'static [Self];
    fn default_keys(self) -> &'static [&'static str];
}

impl Actions for Action {
    const TABLE: &'static str = "[keys.browse]";
    const REQUIRED: Self = Self::Quit;
    const CTRL_C: Option<Self> = Some(Self::Quit);

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
    const CTRL_C: Option<Self> = None;

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
    /// `taken` says which actions took its default keys, if any did.
    #[error("{table} {action} must have at least one key{taken}")]
    NoKey { table: &'static str, action: String, taken: String },
    #[error("{table} {action}: ctrl-c always quits, so it cannot have another action")]
    CtrlC { table: &'static str, action: String },
}

/// The keys of one table. A key has at most one action in a table. The same
/// key can mean different things in different tables.
#[derive(Debug, Clone)]
pub struct Bindings<A> {
    lookup: HashMap<KeyPress, A>,
    list: Vec<(A, Vec<KeyPress>)>,
    /// Actions left with no key because other actions took all their default
    /// keys, for a warning. The user did not ask for that.
    lost: Vec<String>,
}

/// Which actions the configuration file gave the `defaults` of an action
/// to, as ": s is bound to search", or nothing.
fn taken_by<A: Actions>(defaults: &[KeyPress], written: &HashMap<A, Vec<KeyPress>>) -> String {
    let parts: Vec<String> = defaults
        .iter()
        .filter_map(|k| {
            // In the fixed order of the actions, so the message is the same each time.
            A::all()
                .iter()
                .find(|a| written.get(*a).is_some_and(|keys| keys.contains(k)))
                .map(|a| format!("{k} is bound to {}", name(*a)))
        })
        .collect();
    if parts.is_empty() { String::new() } else { format!(": {}", parts.join(", ")) }
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
    ///
    /// A default key that the configuration file gives to another action
    /// goes to that action. So a file from before a new action got its
    /// default keys still loads.
    pub fn new(overrides: &Overrides<A>) -> Result<Self, KeyMapError> {
        let mut written = HashMap::new();
        for (&action, list) in overrides {
            let keys = list
                .iter()
                .map(|s| {
                    s.parse().map_err(|source| KeyMapError::Parse { table: A::TABLE, action: name(action), source })
                })
                .collect::<Result<Vec<KeyPress>, _>>()?;
            if Some(action) != A::CTRL_C && keys.contains(&KeyPress::ctrl('c')) {
                return Err(KeyMapError::CtrlC { table: A::TABLE, action: name(action) });
            }
            written.insert(action, keys);
        }
        let taken: HashSet<KeyPress> = written.values().flatten().copied().collect();
        let mut lookup = HashMap::new();
        let mut bindings = Vec::new();
        let mut lost = Vec::new();
        for &action in A::all() {
            let defaults: Vec<KeyPress> =
                action.default_keys().iter().map(|s| s.parse().expect("default keys parse")).collect();
            let mut keys: Vec<KeyPress> = match written.get(&action) {
                Some(keys) => keys.clone(),
                None => defaults.iter().copied().filter(|k| !taken.contains(k)).collect(),
            };
            if Some(action) == A::CTRL_C && !keys.contains(&KeyPress::ctrl('c')) {
                keys.push(KeyPress::ctrl('c'));
            }
            let by_others = !written.contains_key(&action) && !defaults.is_empty();
            if action == A::REQUIRED && keys.is_empty() {
                let taken = if by_others { taken_by(&defaults, &written) } else { String::new() };
                return Err(KeyMapError::NoKey { table: A::TABLE, action: name(action), taken });
            }
            if keys.is_empty() && by_others {
                lost.push(format!("{} {} has no key{}", A::TABLE, name(action), taken_by(&defaults, &written)));
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
        Ok(Self { lookup, list: bindings, lost })
    }

    /// Actions that lost all their keys to other actions.
    pub fn lost(&self) -> &[String] {
        &self.lost
    }

    pub fn action(&self, key: KeyPress) -> Option<A> {
        self.lookup.get(&key).copied()
    }

    /// Every action with its keys, in a fixed order, for the help screen.
    pub fn bindings(&self) -> &[(A, Vec<KeyPress>)] {
        &self.list
    }

    /// The first key of an action, for short hints in the status bar.
    pub fn first_key(&self, action: A) -> Option<KeyPress> {
        self.list.iter().find(|(a, _)| *a == action).and_then(|(_, k)| k.first().copied())
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

    /// Actions that the configuration file left with no key by giving their
    /// keys to other actions, for a warning.
    pub fn warnings(&self) -> Vec<String> {
        self.browse.lost().iter().chain(self.player.lost()).cloned().collect()
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

    /// The player action of a key while the mini-player plays. The browse
    /// keys come first: `quit` and `close_player` stop the video, as the
    /// player's back key does, and help shows the help. The other browse keys do browse things. Then
    /// come the player keys that make sense while the player is small.
    pub fn mini_player_action(&self, key: KeyPress) -> Option<PlayerAction> {
        match self.browse.action(key) {
            Some(Action::Quit | Action::ClosePlayer) => Some(PlayerAction::Back),
            Some(Action::Help) => Some(PlayerAction::Help),
            Some(_) => None,
            None => self.player.action(key).filter(|a| !matches!(a, PlayerAction::Minimize | PlayerAction::Quality)),
        }
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
    fn mini_player_keys() {
        let map = KeyMap::default();
        let mini = |s: &str| map.mini_player_action(key(s));
        // Quit and close_player stop the video.
        assert_eq!(mini("q"), Some(PlayerAction::Back));
        assert_eq!(mini("x"), Some(PlayerAction::Back));
        // Player keys that browse does not use.
        assert_eq!(mini("space"), Some(PlayerAction::TogglePause));
        assert_eq!(mini("9"), Some(PlayerAction::VolumeDown));
        assert_eq!(mini("right"), Some(PlayerAction::SeekForward));
        // Browse keys, and keys that need the full player.
        for s in ["v", "/", "c", "up", "pgdn", "esc", "Q"] {
            assert_eq!(mini(s), None, "{s}");
        }
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
    fn back_needs_a_key_and_quit_always_has_ctrl_c() {
        // ctrl-c always quits, so quit is never without a key.
        let o = KeyOverrides::from([(Action::Quit, vec![])]);
        let km = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap();
        assert_eq!(km.first_key(Action::Quit), Some(KeyPress::ctrl('c')));
        let o = KeyOverrides::from([(Action::Quit, vec!["Q".to_string()])]);
        let km = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap();
        let (_, keys) = km.bindings().iter().find(|(a, _)| *a == Action::Quit).unwrap();
        assert_eq!(keys, &[key("Q"), KeyPress::ctrl('c')]);

        let o = PlayerKeyOverrides::from([(PlayerAction::Back, vec![])]);
        let err = KeyMap::new(&KeyOverrides::new(), &o).unwrap_err();
        assert_eq!(err.to_string(), "[keys.player] back must have at least one key");
        // The error names the action that took the key.
        let o = PlayerKeyOverrides::from([(PlayerAction::Quality, vec!["q".to_string()])]);
        let err = KeyMap::new(&KeyOverrides::new(), &o).unwrap_err();
        assert_eq!(err.to_string(), "[keys.player] back must have at least one key: q is bound to quality");
    }

    #[test]
    fn ctrl_c_cannot_have_another_action() {
        let o = KeyOverrides::from([(Action::ClosePlayer, vec!["ctrl-c".to_string()])]);
        let err = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "[keys.browse] close_player: ctrl-c always quits, so it cannot have another action"
        );
        let o = PlayerKeyOverrides::from([(PlayerAction::Back, vec!["q".to_string(), "ctrl-c".to_string()])]);
        assert!(matches!(KeyMap::new(&KeyOverrides::new(), &o), Err(KeyMapError::CtrlC { .. })));
        let o = KeyOverrides::from([(Action::Quit, vec!["ctrl-c".to_string()])]);
        assert!(KeyMap::new(&o, &PlayerKeyOverrides::new()).is_ok());
    }

    #[test]
    fn an_action_that_loses_its_keys_is_a_warning() {
        let o = KeyOverrides::from([(Action::Search, vec!["s".to_string()])]);
        let km = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap();
        assert_eq!(km.warnings(), ["[keys.browse] toggle has no key: s is bound to search"]);
        // An action the file leaves with no key on purpose is no warning.
        let o = KeyOverrides::from([(Action::Toggle, vec![])]);
        assert!(KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap().warnings().is_empty());
        assert!(KeyMap::default().warnings().is_empty());
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
        let o = KeyOverrides::from([(Action::Down, vec!["k".to_string()]), (Action::Up, vec!["k".to_string()])]);
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
    fn a_default_key_gives_way_to_a_written_one() {
        // The keys that came before the mini-player still load.
        let browse = KeyOverrides::from([(Action::Delete, vec!["x".to_string()])]);
        let player = PlayerKeyOverrides::from([(PlayerAction::Back, vec!["q".to_string(), "esc".to_string()])]);
        let km = KeyMap::new(&browse, &player).unwrap();
        assert_eq!(km.action(key("x")), Some(Action::Delete));
        assert_eq!(km.first_key(Action::ClosePlayer), None);
        assert_eq!(km.player.action(key("esc")), Some(PlayerAction::Back));
        assert_eq!(km.player.first_key(PlayerAction::Minimize), None);
        // An action keeps its other default keys.
        let o = KeyOverrides::from([(Action::Delete, vec!["k".to_string()])]);
        let km = KeyMap::new(&o, &PlayerKeyOverrides::new()).unwrap();
        assert_eq!(km.action(key("k")), Some(Action::Delete));
        assert_eq!(km.action(key("up")), Some(Action::Up));
        // Quit must still keep a key.
        let o = KeyOverrides::from([(Action::Delete, vec!["q".to_string(), "ctrl-c".to_string()])]);
        assert!(KeyMap::new(&o, &PlayerKeyOverrides::new()).is_err());
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

        let o = PlayerKeyOverrides::from([
            (PlayerAction::Mute, vec!["space".to_string()]),
            (PlayerAction::TogglePause, vec!["space".to_string()]),
        ]);
        let err = KeyMap::new(&KeyOverrides::new(), &o).unwrap_err();
        assert_eq!(err.to_string(), "[keys.player] key space is bound to both toggle_pause and mute");
        let o = PlayerKeyOverrides::from([(PlayerAction::SeekForwardLong, vec!["l".to_string()])]);
        let km = KeyMap::new(&KeyOverrides::new(), &o).unwrap();
        assert_eq!(km.player.action(key("l")), Some(PlayerAction::SeekForwardLong));
        assert_eq!(km.player.action(key("up")), None);
    }
}
