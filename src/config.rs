//! The configuration file and the standard file locations.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Deserialize;

use crate::keymap::{KeyMap, KeyOverrides, PlayerKeyOverrides};
use crate::source::download::FileFormat;
use crate::sponsorblock::Category;

/// File locations from the XDG base directories, on Linux and on macOS.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub database: PathBuf,
    pub log_file: PathBuf,
    pub thumbnail_cache: PathBuf,
    /// Downloaded videos. Not in the cache folder, because nothing may
    /// delete them but the user.
    pub downloads: PathBuf,
}

impl Paths {
    /// `$XDG_CONFIG_HOME/reeltty/config.toml`, `$XDG_DATA_HOME/reeltty/reeltty.db`,
    /// `$XDG_STATE_HOME/reeltty/reeltty.log`, `$XDG_CACHE_HOME/reeltty/thumbnails`, and
    /// `$XDG_DATA_HOME/reeltty/downloads`.
    ///
    /// macOS uses the same folders, not `~/Library`, as Ghostty, kitty, mpv
    /// and yt-dlp do there.
    pub fn from_env() -> Result<Self> {
        let home = std::env::home_dir().filter(|h| !h.as_os_str().is_empty()).context("no home directory")?;
        Ok(Self::from_vars(&home, |name| std::env::var_os(name)))
    }

    /// The paths in `home`, with the XDG variables that `var` gives. A
    /// variable that is not set, is empty or is not an absolute path is
    /// ignored, as the XDG Base Directory Specification says.
    fn from_vars(home: &Path, var: impl Fn(&str) -> Option<OsString>) -> Self {
        let dir = |name: &str, default: &str| {
            let base = var(name).map(PathBuf::from).filter(|p| p.is_absolute());
            base.unwrap_or_else(|| home.join(default)).join("reeltty")
        };
        let data = dir("XDG_DATA_HOME", ".local/share");
        Self {
            config_file: dir("XDG_CONFIG_HOME", ".config").join("config.toml"),
            database: data.join("reeltty.db"),
            log_file: dir("XDG_STATE_HOME", ".local/state").join("reeltty.log"),
            thumbnail_cache: dir("XDG_CACHE_HOME", ".cache").join("thumbnails"),
            downloads: data.join("downloads"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The yt-dlp program. If not set, reeltty uses `yt-dlp` from `PATH`.
    pub yt_dlp: Option<PathBuf>,
    /// Show thumbnails in lists.
    pub thumbnails: Thumbnails,
    /// When a video ends, play the next one.
    pub autoplay: Autoplay,
    /// The largest video size to play.
    pub max_quality: MaxQuality,
    /// No spinner while things load.
    pub reduce_motion: bool,
    /// Space between list items.
    pub density: Density,
    /// The colors: the terminal's 16 (`"ansi"`), or a truecolor theme.
    pub theme: crate::theme::ThemeName,
    /// The player's accent color: the theme's (`"theme"`), or the main color
    /// of the video's thumbnail (`"auto"`).
    pub accent: Accent,
    /// The full player hides its status line while the video plays and no
    /// key was pressed for 3 seconds. The progress bar stays.
    pub autohide_status: bool,
    pub captions: Captions,
    pub feed: Feed,
    pub sponsorblock: SponsorBlock,
    pub downloads: Downloads,
    pub keys: Keys,
}

/// The `[downloads]` table: how downloaded videos are kept.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Downloads {
    /// `"separate"` video and audio files, or one `"mkv"` file, which needs ffmpeg.
    pub format: FileFormat,
}

/// The `[sponsorblock]` table: which parts of videos to skip.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SponsorBlock {
    pub on: bool,
    pub categories: Vec<Category>,
}

impl Default for SponsorBlock {
    fn default() -> Self {
        Self { on: true, categories: vec![Category::Sponsor] }
    }
}

impl SponsorBlock {
    /// The categories to skip. None when it is off.
    pub fn skipped(&self) -> &[Category] {
        if self.on { &self.categories } else { &[] }
    }
}

/// The `[feed]` table: which videos the subscriptions feed shows.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Feed {
    pub shorts: bool,
    pub live: bool,
}

/// A wrapper so that the default is `true`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
pub struct Autoplay(pub bool);

impl Default for Autoplay {
    fn default() -> Self {
        Self(true)
    }
}

/// The `[captions]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Captions {
    /// A language code such as `en` or `de`.
    pub language: String,
    /// Show captions when a video starts.
    pub on: bool,
}

impl Default for Captions {
    fn default() -> Self {
        Self { language: "en".into(), on: false }
    }
}

/// A wrapper so that the default is `true`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
pub struct Thumbnails(pub bool);

impl Default for Thumbnails {
    fn default() -> Self {
        Self(true)
    }
}

/// The largest video size to play: `"auto"` (up to 1080p), `"best"` (up to
/// 2160p), or a height such as `1440`. The size also follows the picture,
/// so a small window gets a small stream. Larger sizes use more CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "RawMaxQuality")]
pub enum MaxQuality {
    #[default]
    Auto,
    Best,
    /// A height in pixels on the short side.
    Height(u32),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawMaxQuality {
    Name(String),
    Height(u32),
}

impl TryFrom<RawMaxQuality> for MaxQuality {
    type Error = String;

    fn try_from(raw: RawMaxQuality) -> std::result::Result<Self, String> {
        match raw {
            RawMaxQuality::Name(n) if n == "auto" => Ok(Self::Auto),
            RawMaxQuality::Name(n) if n == "best" => Ok(Self::Best),
            RawMaxQuality::Name(n) => Err(format!("max_quality must be \"auto\", \"best\", or a height, not \"{n}\"")),
            RawMaxQuality::Height(h) => Ok(Self::Height(h)),
        }
    }
}

impl MaxQuality {
    /// The largest height, as a size that YouTube offers.
    pub fn height(self) -> u32 {
        match self {
            Self::Auto => 1080,
            Self::Best => crate::source::streams::RUNGS[crate::source::streams::RUNGS.len() - 1],
            Self::Height(h) => crate::source::streams::rung_below(h),
        }
    }
}

/// Space between list items: none (`"compact"`), a blank row
/// (`"comfortable"`), or a blank row only on a screen with room for it
/// (`"auto"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
    Auto,
}

impl Density {
    /// With `"auto"`, a screen this many rows high or higher has a blank row
    /// between list items.
    pub const AUTO_MIN_HEIGHT: u16 = 40;

    /// The blank rows after each list item on a screen `height` rows high.
    pub fn gap(self, height: u16) -> u16 {
        match self {
            Self::Compact => 0,
            Self::Comfortable => 1,
            Self::Auto => u16::from(height >= Self::AUTO_MIN_HEIGHT),
        }
    }
}

/// The `accent` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    /// The accent color of the theme.
    #[default]
    Theme,
    /// The main color of the thumbnail of the video that plays.
    Auto,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keys {
    pub browse: KeyOverrides,
    pub player: PlayerKeyOverrides,
}

/// Where the configuration came from, for `reeltty doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File,
    Defaults,
}

impl Config {
    /// Reads the file. A missing file gives the defaults.
    pub fn load(path: &Path) -> Result<(Self, Source)> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let config = Self::parse(&text).with_context(|| format!("read {}", path.display()))?;
                Ok((config, Source::File))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Self::default(), Source::Defaults)),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        config.keymap()?;
        Ok(config)
    }

    pub fn keymap(&self) -> Result<KeyMap> {
        Ok(KeyMap::new(&self.keys.browse, &self.keys.player)?)
    }

    pub fn yt_dlp_program(&self) -> PathBuf {
        self.yt_dlp.clone().unwrap_or_else(|| PathBuf::from("yt-dlp"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Action, KeyPress, PlayerAction};

    #[test]
    fn paths_default_to_the_home_folder() {
        let p = Paths::from_vars(Path::new("/home/me"), |_| None);
        assert_eq!(p.config_file, Path::new("/home/me/.config/reeltty/config.toml"));
        assert_eq!(p.database, Path::new("/home/me/.local/share/reeltty/reeltty.db"));
        assert_eq!(p.log_file, Path::new("/home/me/.local/state/reeltty/reeltty.log"));
        assert_eq!(p.thumbnail_cache, Path::new("/home/me/.cache/reeltty/thumbnails"));
        assert_eq!(p.downloads, Path::new("/home/me/.local/share/reeltty/downloads"));
    }

    #[test]
    fn paths_follow_the_xdg_variables() {
        let p = Paths::from_vars(Path::new("/home/me"), |name| Some(format!("/x/{name}").into()));
        assert_eq!(p.config_file, Path::new("/x/XDG_CONFIG_HOME/reeltty/config.toml"));
        assert_eq!(p.database, Path::new("/x/XDG_DATA_HOME/reeltty/reeltty.db"));
        assert_eq!(p.log_file, Path::new("/x/XDG_STATE_HOME/reeltty/reeltty.log"));
        assert_eq!(p.thumbnail_cache, Path::new("/x/XDG_CACHE_HOME/reeltty/thumbnails"));
        assert_eq!(p.downloads, Path::new("/x/XDG_DATA_HOME/reeltty/downloads"));
    }

    #[test]
    fn paths_ignore_empty_and_relative_variables() {
        let var = |name: &str| match name {
            "XDG_CONFIG_HOME" => Some("".into()),
            "XDG_DATA_HOME" => Some("relative/data".into()),
            _ => None,
        };
        let p = Paths::from_vars(Path::new("/home/me"), var);
        assert_eq!(p.config_file, Path::new("/home/me/.config/reeltty/config.toml"));
        assert_eq!(p.database, Path::new("/home/me/.local/share/reeltty/reeltty.db"));
    }

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::parse("").unwrap();
        assert!(c.thumbnails.0);
        assert_eq!(c.yt_dlp_program(), PathBuf::from("yt-dlp"));
    }

    #[test]
    fn reads_all_fields() {
        let c = Config::parse(
            r#"
            yt_dlp = "/home/me/.local/bin/yt-dlp"
            thumbnails = false

            [keys.browse]
            down = ["n", "down"]
            "#,
        )
        .unwrap();
        assert!(!c.thumbnails.0);
        assert_eq!(c.yt_dlp_program(), PathBuf::from("/home/me/.local/bin/yt-dlp"));
        let km = c.keymap().unwrap();
        assert_eq!(km.action("n".parse::<KeyPress>().unwrap()), Some(Action::Down));
    }

    #[test]
    fn reads_the_max_quality() {
        let height = |text: &str| Config::parse(text).unwrap().max_quality.height();
        assert_eq!(height(""), 1080);
        assert_eq!(height("max_quality = \"auto\""), 1080);
        assert_eq!(height("max_quality = \"best\""), 2160);
        assert_eq!(height("max_quality = 1440"), 1440);
        assert_eq!(height("max_quality = 1000"), 720);
        assert_eq!(height("max_quality = 100"), 144);
        let err = Config::parse("max_quality = \"4k\"").unwrap_err();
        assert!(format!("{err:#}").contains("\"auto\", \"best\", or a height"), "{err:#}");
        assert!(Config::parse("max_quality = -1").is_err());
    }

    #[test]
    fn reads_the_download_format() {
        let format = |text: &str| Config::parse(text).unwrap().downloads.format;
        assert_eq!(format(""), FileFormat::Separate);
        assert_eq!(format("[downloads]\nformat = \"separate\""), FileFormat::Separate);
        assert_eq!(format("[downloads]\nformat = \"mkv\""), FileFormat::Mkv);
        assert!(Config::parse("[downloads]\nformat = \"mp4\"").is_err());
        assert!(Config::parse("[downloads]\ndir = \"/tmp\"").is_err());
    }

    #[test]
    fn reads_reduce_motion() {
        assert!(!Config::parse("").unwrap().reduce_motion);
        assert!(Config::parse("reduce_motion = true").unwrap().reduce_motion);
        assert!(Config::parse("reduce_motion = \"yes\"").is_err());
    }

    #[test]
    fn reads_the_density() {
        let density = |text: &str| Config::parse(text).unwrap().density;
        assert_eq!(density(""), Density::Compact);
        assert_eq!(density("density = \"comfortable\""), Density::Comfortable);
        assert_eq!(density("density = \"auto\""), Density::Auto);
        assert!(Config::parse("density = \"cozy\"").is_err());
        assert_eq!(Density::Compact.gap(60), 0);
        assert_eq!(Density::Comfortable.gap(10), 1);
        assert_eq!(Density::Auto.gap(39), 0);
        assert_eq!(Density::Auto.gap(40), 1);
    }

    #[test]
    fn reads_the_theme() {
        use crate::theme::ThemeName;
        let theme = |text: &str| Config::parse(text).unwrap().theme;
        assert_eq!(theme(""), ThemeName::Ansi);
        assert_eq!(theme("theme = \"auto\""), ThemeName::Auto);
        assert_eq!(theme("theme = \"catppuccin-latte\""), ThemeName::CatppuccinLatte);
        assert_eq!(theme("theme = \"tokyo-night\""), ThemeName::TokyoNight);
        assert!(Config::parse("theme = \"dracula\"").is_err());
        for name in [ThemeName::Ansi, ThemeName::Auto, ThemeName::CatppuccinMocha, ThemeName::CatppuccinLatte] {
            assert_eq!(theme(&format!("theme = \"{}\"", name.as_str())), name);
        }
    }

    #[test]
    fn reads_the_accent() {
        assert_eq!(Config::parse("").unwrap().accent, Accent::Theme);
        assert_eq!(Config::parse("accent = \"auto\"").unwrap().accent, Accent::Auto);
        assert!(Config::parse("accent = \"red\"").is_err());
    }

    #[test]
    fn reads_autohide_status() {
        assert!(!Config::parse("").unwrap().autohide_status);
        assert!(Config::parse("autohide_status = true").unwrap().autohide_status);
    }

    #[test]
    fn reads_the_mini_player_keys() {
        let c = Config::parse(
            "[keys.browse]\nshow_player = [\"P\"]\nclose_player = [\"X\"]\n[keys.player]\nminimize = [\"i\"]",
        )
        .unwrap();
        let km = c.keymap().unwrap();
        let key = |s: &str| s.parse::<KeyPress>().unwrap();
        assert_eq!(km.action(key("P")), Some(Action::ShowPlayer));
        assert_eq!(km.action(key("X")), Some(Action::ClosePlayer));
        assert_eq!(km.player.action(key("i")), Some(PlayerAction::Minimize));
        assert_eq!(km.player.action(key("esc")), None);
    }

    #[test]
    fn rejects_unknown_fields_and_actions() {
        assert!(Config::parse("thumbnail = false").is_err());
        assert!(Config::parse("[keys.browse]\nfly = [\"f\"]").is_err());
    }

    #[test]
    fn rejects_bad_keys() {
        let err = Config::parse("[keys.browse]\ndown = [\"k\"]\nup = [\"k\"]").unwrap_err();
        assert!(err.to_string().contains("bound to both"), "{err}");
        let err = Config::parse("[keys.player]\nmute = [\"space\"]\nback = [\"space\"]").unwrap_err();
        assert!(err.to_string().contains("[keys.player]"), "{err}");
    }

    #[test]
    fn reads_sponsorblock() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.sponsorblock.skipped(), [Category::Sponsor]);
        let c = Config::parse("[sponsorblock]\ncategories = [\"intro\", \"music_offtopic\"]").unwrap();
        assert_eq!(c.sponsorblock.skipped(), [Category::Intro, Category::MusicOfftopic]);
        let c = Config::parse("[sponsorblock]\non = false").unwrap();
        assert!(c.sponsorblock.skipped().is_empty());
        assert!(Config::parse("[sponsorblock]\ncategories = [\"ads\"]").is_err());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (_, source) = Config::load(&dir.path().join("none.toml")).unwrap();
        assert_eq!(source, Source::Defaults);
    }
}
