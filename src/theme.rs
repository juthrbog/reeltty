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
//! Image cells (Kitty placeholders) never take a theme style. Their color
//! holds the image ID.

use ratatui::style::{Color, Modifier, Style};

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
    /// The selected row of a list, table or menu.
    selection: Style::new().add_modifier(REVERSED),
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
    fn no_color_keeps_the_modifiers() {
        let (a, n) = (Theme::ansi(), Theme::no_color());
        assert!(a.colors && !n.colors);
        for ((role, color), (_, plain)) in a.styles().into_iter().zip(n.styles()) {
            assert_eq!((plain.fg, plain.bg, plain.underline_color), (None, None, None), "{role}");
            assert_eq!((plain.add_modifier, plain.sub_modifier), (color.add_modifier, color.sub_modifier), "{role}");
        }
    }
}
