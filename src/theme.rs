//! The styles of the UI, by role. The UI asks the theme for a role, such as
//! `muted` or `accent`, and never names a color itself.
//!
//! The default theme uses only the 16 ANSI colors, so it follows the colors
//! of the terminal, on dark and light backgrounds and over tmux or SSH. It
//! has one accent color (magenta), uses red only for errors and LIVE, and
//! uses yellow only for a pill on a word or mark that reads without color.
//! On a light background, yellow has weak contrast either way, so it never
//! carries meaning alone. No state depends on color
//! alone: each one also has a modifier, a mark or a word.
//!
//! Truecolor themes are opt-in, with the `theme` setting. They are made from
//! the default theme, so they have the same roles and modifiers: each ANSI
//! color becomes a color of the palette, and dim text becomes the palette's
//! muted color. They never paint the background behind text, only the
//! selection and pills, so images and the terminal background show through.
//!
//! Image cells (Kitty placeholders) never take a theme style. Their color
//! holds the image ID.

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

/// Declares the roles once, so the struct, `ansi()`, `no_color()` and the
/// test list cannot get out of step.
macro_rules! theme {
    ($($(#[doc = $doc:literal])* $role:ident: $style:expr,)*) => {
        /// The style of each role of the UI.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Theme {
            /// False for the `NO_COLOR` theme. Then the UI also removes any
            /// color that is left after drawing.
            pub(crate) colors: bool,
            $($(#[doc = $doc])* pub(crate) $role: Style,)*
        }

        impl Theme {
            /// The default theme: the 16 ANSI colors of the terminal.
            pub const fn ansi() -> Self {
                Self { colors: true, $($role: $style,)* }
            }

            /// The default theme without colors, for `NO_COLOR`. The
            /// modifiers stay, so every state still shows.
            pub const fn no_color() -> Self {
                let a = Self::ansi();
                Self { colors: false, $($role: plain(a.$role),)* }
            }

            /// Each role's style, changed by `f`.
            fn map(self, f: impl Fn(Style) -> Style) -> Self {
                Self { colors: self.colors, $($role: f(self.$role),)* }
            }

            /// Every role and its style.
            #[cfg(test)]
            pub(crate) fn styles(&self) -> Vec<(&'static str, Style)> {
                vec![$((stringify!($role), self.$role),)*]
            }
        }
    };
}

const BOLD: Modifier = Modifier::BOLD;
const DIM: Modifier = Modifier::DIM;
const REVERSED: Modifier = Modifier::REVERSED;
const ACCENT: Color = Color::Magenta;

/// A style that is drawn on top of a dim border, such as a block title,
/// must remove the dim, because ratatui adds modifiers together.
const fn strong() -> Style {
    Style::new().add_modifier(BOLD).remove_modifier(DIM)
}

/// A pill: the color as background, and the terminal background as text.
const fn pill(color: Color) -> Style {
    Style::new().fg(color).add_modifier(REVERSED.union(BOLD))
}

const fn dim() -> Style {
    Style::new().add_modifier(DIM)
}

theme! {
    /// The one accent: the filter chip, the comment bar, marks.
    accent: Style::new().fg(ACCENT),
    /// " reeltty " on the tab row.
    wordmark: Style::new().fg(ACCENT).add_modifier(REVERSED.union(BOLD)),
    /// A tab that is not selected.
    tab: dim(),
    /// The selected tab. Bold next to dim also shows it without color.
    tab_selected: Style::new().fg(ACCENT).add_modifier(BOLD),
    /// Details, dates, dividers and empty-state text.
    muted: dim(),
    /// Item titles, block titles and headings.
    title: strong(),
    /// Every border.
    border: dim(),
    /// The thumb of a scrollbar on a border. It removes the dim of the
    /// border under it.
    scrollbar: Style::new().remove_modifier(DIM),
    /// The selected row of a menu or table, and the selected link.
    selection: Style::new().add_modifier(REVERSED),
    /// The bar before each line of the selected row of a list.
    list_bar: Style::new().fg(ACCENT),
    /// The title of the selected row of a list, on top of `title`. The
    /// underline also shows it without color.
    list_title: Style::new().fg(ACCENT).add_modifier(Modifier::UNDERLINED),
    /// The `▶` before the video that plays now.
    now_playing: Style::new().fg(ACCENT).add_modifier(BOLD),
    /// A key in a hint.
    key: strong(),
    /// The label next to a key.
    key_label: dim(),
    /// A link in a description or comment.
    link: Style::new().fg(ACCENT).add_modifier(Modifier::UNDERLINED),
    /// "Loading…" and other work that is going on.
    status_progress: dim(),
    /// The "✓" before a confirmation.
    status_success: Style::new().fg(Color::Green).add_modifier(BOLD),
    /// The "!" before a warning.
    status_warn: pill(Color::Yellow),
    /// An error, with a "✗" before it.
    status_error: Style::new().fg(Color::Red),
    /// LIVE.
    badge_live: pill(Color::Red),
    /// MEMBERS.
    badge_members: pill(Color::Yellow),
    /// SHORT.
    badge_short: dim(),
    /// CHANNEL.
    badge_channel: dim(),
    /// PLAYLIST and COURSE.
    badge_playlist: dim(),
    /// A comment by the uploader of the video.
    badge_owner: Style::new().fg(ACCENT).add_modifier(REVERSED),
    /// "[Subscribed]" and "[Saved]". The brackets show the state.
    saved: Style::new().add_modifier(BOLD),
    /// "[Pinned]".
    pinned: Style::new().add_modifier(BOLD),
    /// The played part of the progress bar.
    progress_played: Style::new().fg(ACCENT),
    /// SponsorBlock segments on the progress bar, which also have glyphs of
    /// their own.
    progress_sponsor: Style::new().fg(Color::Green),
    /// The part of the progress bar that is not played yet.
    progress_track: dim(),
}

/// `s` without its colors.
const fn plain(s: Style) -> Style {
    Style { fg: None, bg: None, underline_color: None, ..s }
}

impl Theme {
    /// The selected link.
    pub fn link_selected(&self) -> Style {
        self.link.patch(self.selection)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::ansi()
    }
}

/// The colors of a truecolor theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Text, titles and keys.
    pub text: Color,
    /// Details, borders and other dim text.
    pub muted: Color,
    /// The background of the selection.
    pub surface: Color,
    /// The text on a pill: the background color of the theme.
    pub base: Color,
    pub accent: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::from_u32(hex)
}

/// <https://catppuccin.com/palette>
pub const CATPPUCCIN_MOCHA: Palette = Palette {
    text: rgb(0x00cd_d6f4),
    muted: rgb(0x007f_849c),
    surface: rgb(0x0045_475a),
    base: rgb(0x001e_1e2e),
    accent: rgb(0x00cb_a6f7),
    green: rgb(0x00a6_e3a1),
    red: rgb(0x00f3_8ba8),
    yellow: rgb(0x00f9_e2af),
};

/// The light Catppuccin flavor.
pub const CATPPUCCIN_LATTE: Palette = Palette {
    text: rgb(0x004c_4f69),
    muted: rgb(0x008c_8fa1),
    surface: rgb(0x00cc_d0da),
    base: rgb(0x00ef_f1f5),
    accent: rgb(0x0088_39ef),
    green: rgb(0x0040_a02b),
    red: rgb(0x00d2_0f39),
    yellow: rgb(0x00df_8e1d),
};

/// <https://github.com/folke/tokyonight.nvim>, the "night" style.
pub const TOKYO_NIGHT: Palette = Palette {
    text: rgb(0x00c0_caf5),
    muted: rgb(0x0073_7aa2),
    surface: rgb(0x0033_467c),
    base: rgb(0x001a_1b26),
    accent: rgb(0x00bb_9af7),
    green: rgb(0x009e_ce6a),
    red: rgb(0x00f7_768e),
    yellow: rgb(0x00e0_af68),
};

impl Theme {
    /// A truecolor theme from `p`, with the roles of the default theme.
    pub fn truecolor(p: &Palette) -> Self {
        Self::ansi().map(|s| {
            let color = |c: Option<Color>| match c {
                Some(Color::Magenta) => Some(p.accent),
                Some(Color::Green) => Some(p.green),
                Some(Color::Red) => Some(p.red),
                Some(Color::Yellow) => Some(p.yellow),
                other => other,
            };
            let mut s = Style { fg: color(s.fg), ..s };
            if s.add_modifier.contains(DIM) {
                // Dim text: the muted color, which is clearer than DIM on any palette.
                s = Style { fg: s.fg.or(Some(p.muted)), add_modifier: s.add_modifier - DIM, ..s };
            } else if s.fg.is_none() {
                s.fg = Some(p.text);
            }
            if s.add_modifier.contains(REVERSED) {
                // A pill, or the selection, which has no color of its own.
                s = match s.fg {
                    Some(fg) if fg != p.text => Style { fg: Some(p.base), bg: Some(fg), ..s },
                    _ => Style { fg: Some(p.text), bg: Some(p.surface), ..s },
                };
                s.add_modifier -= REVERSED;
            }
            s
        })
    }

    /// The theme for `name`. `light` is whether the terminal background is
    /// light, if the terminal said. `"auto"` picks the light Catppuccin
    /// theme on a light background, and the dark one otherwise.
    pub fn named(name: ThemeName, light: Option<bool>) -> Self {
        match name {
            ThemeName::Ansi => Self::ansi(),
            ThemeName::Auto if light == Some(true) => Self::truecolor(&CATPPUCCIN_LATTE),
            ThemeName::Auto | ThemeName::CatppuccinMocha => Self::truecolor(&CATPPUCCIN_MOCHA),
            ThemeName::CatppuccinLatte => Self::truecolor(&CATPPUCCIN_LATTE),
            ThemeName::TokyoNight => Self::truecolor(&TOKYO_NIGHT),
        }
    }
}

/// The `theme` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    /// The 16 ANSI colors of the terminal.
    #[default]
    Ansi,
    /// Catppuccin Latte on a light background, else Catppuccin Mocha.
    Auto,
    CatppuccinMocha,
    CatppuccinLatte,
    TokyoNight,
}

impl ThemeName {
    /// The name as in the configuration file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ansi => "ansi",
            Self::Auto => "auto",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::CatppuccinLatte => "catppuccin-latte",
            Self::TokyoNight => "tokyo-night",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_uses_few_colors() {
        for (role, style) in Theme::ansi().styles() {
            assert_eq!(style.bg, None, "{role}: no background, so images and the terminal show through");
            match style.fg {
                None | Some(Color::Magenta) => {}
                Some(Color::Green) => assert!(matches!(role, "status_success" | "progress_sponsor"), "{role}"),
                Some(Color::Red) => assert!(matches!(role, "status_error" | "badge_live"), "{role}"),
                // Weak on light backgrounds, so only as a pill on a word or a
                // mark, never as the color of plain text.
                Some(Color::Yellow) => {
                    assert!(matches!(role, "status_warn" | "badge_members"), "{role}");
                    assert!(style.add_modifier.contains(REVERSED), "{role}");
                }
                Some(c) => panic!("{role} uses {c:?}"),
            }
        }
    }

    #[test]
    fn titles_are_not_dim_on_dim_borders() {
        let t = Theme::ansi();
        for s in [t.title, t.key] {
            assert!(s.sub_modifier.contains(DIM));
            assert!(s.add_modifier.contains(BOLD));
        }
    }

    #[test]
    fn truecolor_themes_follow_the_default_roles() {
        let a = Theme::ansi();
        for p in [CATPPUCCIN_MOCHA, CATPPUCCIN_LATTE, TOKYO_NIGHT] {
            let t = Theme::truecolor(&p);
            assert!(t.colors);
            for ((role, ansi), (_, style)) in a.styles().into_iter().zip(t.styles()) {
                assert!(matches!(style.fg, Some(Color::Rgb(..))), "{role}: {style:?}");
                assert!(!style.add_modifier.intersects(DIM | REVERSED), "{role}: {style:?}");
                // The same other modifiers, so every state shows as before.
                assert_eq!(style.add_modifier, ansi.add_modifier - DIM - REVERSED, "{role}");
                // Only pills and the selection paint the background.
                assert_eq!(style.bg.is_some(), ansi.add_modifier.contains(REVERSED), "{role}");
            }
            assert_eq!(t.accent.fg, Some(p.accent));
            assert_eq!(t.muted.fg, Some(p.muted));
            assert_eq!(t.title.fg, Some(p.text));
            assert_eq!((t.selection.fg, t.selection.bg), (Some(p.text), Some(p.surface)));
            assert_eq!((t.badge_live.fg, t.badge_live.bg), (Some(p.base), Some(p.red)));
            assert_eq!((t.wordmark.fg, t.wordmark.bg), (Some(p.base), Some(p.accent)));
        }
    }

    #[test]
    fn auto_follows_the_background() {
        assert_eq!(Theme::named(ThemeName::Ansi, Some(true)), Theme::ansi());
        assert_eq!(Theme::named(ThemeName::Auto, Some(true)), Theme::truecolor(&CATPPUCCIN_LATTE));
        assert_eq!(Theme::named(ThemeName::Auto, Some(false)), Theme::truecolor(&CATPPUCCIN_MOCHA));
        // No answer from the terminal: dark.
        assert_eq!(Theme::named(ThemeName::Auto, None), Theme::truecolor(&CATPPUCCIN_MOCHA));
        assert_eq!(Theme::named(ThemeName::TokyoNight, Some(true)), Theme::truecolor(&TOKYO_NIGHT));
    }

    #[test]
    fn no_color_keeps_the_modifiers() {
        let (a, n) = (Theme::ansi(), Theme::no_color());
        assert!(a.colors && !n.colors);
        for ((role, color), (_, plain)) in a.styles().into_iter().zip(n.styles()) {
            assert_eq!((plain.fg, plain.bg, plain.underline_color), (None, None, None), "{role}");
            assert_eq!((plain.add_modifier, plain.sub_modifier), (color.add_modifier, color.sub_modifier), "{role}");
        }
    }
}
