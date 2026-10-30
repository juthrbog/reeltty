//! The configuration file and the standard file locations.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Deserialize;

use crate::keymap::{KeyMap, KeyOverrides, PlayerKeyOverrides};

/// File locations from the XDG base directories.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub database: PathBuf,
    pub log_file: PathBuf,
    pub thumbnail_cache: PathBuf,
}

impl Paths {
    /// `$XDG_CONFIG_HOME/reeltty/config.toml`, `$XDG_DATA_HOME/reeltty/reeltty.db`,
    /// `$XDG_STATE_HOME/reeltty/reeltty.log`, and `$XDG_CACHE_HOME/reeltty/thumbnails`.
    pub fn from_env() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "", "reeltty").context("no home directory")?;
        let state = dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir());
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            database: dirs.data_dir().join("reeltty.db"),
            log_file: state.join("reeltty.log"),
            thumbnail_cache: dirs.cache_dir().join("thumbnails"),
        })
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
    pub captions: Captions,
    pub feed: Feed,
    pub keys: Keys,
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
    use crate::keymap::{Action, KeyPress};

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
    fn rejects_unknown_fields_and_actions() {
        assert!(Config::parse("thumbnail = false").is_err());
        assert!(Config::parse("[keys.browse]\nfly = [\"f\"]").is_err());
    }

    #[test]
    fn rejects_bad_keys() {
        let err = Config::parse("[keys.browse]\ndown = [\"k\"]").unwrap_err();
        assert!(err.to_string().contains("bound to both"), "{err}");
        let err = Config::parse("[keys.player]\nmute = [\"space\"]").unwrap_err();
        assert!(err.to_string().contains("[keys.player]"), "{err}");
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (_, source) = Config::load(&dir.path().join("none.toml")).unwrap();
        assert_eq!(source, Source::Defaults);
    }
}
