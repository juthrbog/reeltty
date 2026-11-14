//! Draws the `App` state. Pure: the same state always gives the same frame.

mod items;
mod player;
mod screens;
mod search;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table};
use unicode_width::UnicodeWidthStr as _;

use crate::app::player::{Phase, dock};
use crate::app::{App, StatusKind, StatusMessage, Tab};
use crate::keymap::Action;
use crate::theme::Theme;

pub fn view(app: &App, f: &mut Frame) {
    draw(app, f);
    // The theme has no colors, and this also removes any color that a
    // widget sets by itself.
    if !app.theme.colors {
        remove_colors(f.buffer_mut());
    }
}

fn draw(app: &App, f: &mut Frame) {
    if let Some(p) = app.player.as_ref().filter(|p| !p.mini) {
        return player::draw(app, p, f);
    }
    let [tabs, body, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    draw_tabs(app, f, tabs);
    let dock = app.player.as_ref().and_then(|_| dock(f.area().width, f.area().height));
    match (&app.player, dock) {
        (Some(p), Some(d)) => {
            let [list, column] = Layout::horizontal([Constraint::Min(0), Constraint::Length(d.cols)]).areas(body);
            draw_body(app, f, list);
            player::draw_mini(app, p, f, column);
        }
        _ => draw_body(app, f, body),
    }
    draw_status(app, f, status, dock.is_none());
    if app.show_help {
        draw_help(app, f, body);
    }
}

/// A bordered block in the theme with `title`. Only the border takes the
/// border style: a style on the block would also cover image cells.
pub(crate) fn bordered<'a>(theme: &Theme, title: impl Into<Line<'a>>) -> Block<'a> {
    let mut title = title.into();
    title.style = theme.title.patch(title.style);
    Block::bordered().border_style(theme.border).title(title)
}

/// An error in the body of a view: "✗ " and the text.
pub(crate) fn error_line(theme: &Theme, text: String) -> Line<'static> {
    Line::from(vec![
        Span::styled("✗ ", theme.status_error.add_modifier(Modifier::BOLD)),
        Span::styled(text, theme.status_error),
    ])
}

/// Key hints such as `j/k choose · enter play`. A hint whose key is not
/// bound is left out.
pub(crate) fn hints<'a, K: std::fmt::Display>(parts: impl IntoIterator<Item = (Option<K>, &'a str)>) -> String {
    parts.into_iter().filter_map(|(key, label)| key.map(|k| format!("{k} {label}"))).collect::<Vec<_>>().join(" · ")
}

/// Removes the colors from every cell, for `NO_COLOR`, and keeps the
/// modifiers. Image cells keep their colors, because the colors hold the
/// image ID and the position.
fn remove_colors(buf: &mut Buffer) {
    for cell in &mut buf.content {
        if !cell.symbol().starts_with(crate::terminal::kitty::PLACEHOLDER) {
            cell.set_fg(Color::Reset).set_bg(Color::Reset);
            cell.underline_color = Color::Reset;
        }
    }
}

/// The size in cells of the video that `view` draws, or `None` when it
/// draws no video.
pub fn video_cells(app: &App) -> Option<(u16, u16)> {
    let p = app.player.as_ref().filter(|p| p.is_running())?;
    let up_next = p.phase == Phase::Ended && p.up_next.is_some();
    let no_dock = p.mini && dock(app.width, app.height).is_none();
    (!up_next && !no_dock).then_some((p.geometry.cols, p.geometry.rows))
}

/// The wordmark shows from this width. Below it, the tabs need the room.
const WORDMARK_MIN_WIDTH: u16 = 80;
const WORDMARK: &str = " reeltty ";

/// The tab row: the wordmark, then the tabs with `·` between them. When the
/// tabs do not all fit, the row shows the ones around the selected tab, with
/// `‹` and `›` for the hidden ones.
fn draw_tabs(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let mut tabs_area = area;
    if area.width >= WORDMARK_MIN_WIDTH {
        let [mark, rest] =
            Layout::horizontal([Constraint::Length(cells(WORDMARK.width())), Constraint::Min(0)]).areas(area);
        f.render_widget(Span::styled(WORDMARK, t.wordmark), mark);
        tabs_area = rest;
    }
    let titles: Vec<String> = Tab::ALL.iter().map(|tab| format!(" {} ", tab.title())).collect();
    let widths: Vec<u16> = titles.iter().map(|s| cells(s.width())).collect();
    let selected = Tab::ALL.iter().position(|tab| *tab == app.tab).unwrap_or(0);
    let shown = tab_window(&widths, selected, tabs_area.width);
    let mut spans = Vec::new();
    if shown.start > 0 {
        spans.push(Span::styled("‹", t.muted));
    }
    for i in shown.clone() {
        if i > shown.start {
            spans.push(Span::styled("·", t.muted));
        }
        let style = if i == selected { t.tab_selected } else { t.tab };
        spans.push(Span::styled(titles[i].clone(), style));
    }
    if shown.end < titles.len() {
        spans.push(Span::styled("›", t.muted));
    }
    f.render_widget(Line::from(spans), tabs_area);
}

fn cells(width: usize) -> u16 {
    u16::try_from(width).unwrap_or(u16::MAX)
}

/// The tabs that fit in `width` cells, with the `selected` one among them.
/// `widths` are the tab widths with their padding. A `·` goes between two
/// tabs, and `‹` or `›` takes one cell when tabs are hidden on that side.
/// The last tab needs no space after it.
fn tab_window(widths: &[u16], selected: usize, width: u16) -> std::ops::Range<usize> {
    let n = widths.len();
    let cost = |start: usize, end: usize| -> u32 {
        let tabs: u32 = widths[start..end].iter().map(|w| u32::from(*w)).sum();
        let dividers = u32::try_from(end - start).unwrap_or(0).saturating_sub(1);
        let left = u32::from(start > 0);
        let right = u32::from(end < n);
        (tabs + dividers + left + right).saturating_sub(u32::from(end == n))
    };
    let width = u32::from(width);
    let selected = selected.min(n.saturating_sub(1));
    if n == 0 || cost(0, n) <= width {
        return 0..n;
    }
    let mut start = 0;
    while start < selected && cost(start, selected + 1) > width {
        start += 1;
    }
    let mut end = selected + 1;
    while end < n && cost(start, end + 1) <= width {
        end += 1;
    }
    start..end
}

fn draw_body(app: &App, f: &mut Frame, area: Rect) {
    if let Some(screen) = app.screens.last() {
        return screens::draw(app, screen, f, area);
    }
    match app.tab {
        Tab::Search => search::draw(app, f, area),
        Tab::Feed => screens::draw_feed(app, f, area),
        Tab::Subscriptions => screens::draw_subscriptions(app, f, area),
        Tab::Playlists => screens::draw_saved(app, f, area),
        Tab::Queue => screens::draw_queue(app, f, area),
        Tab::History => screens::draw_history(app, f, area),
    }
}

/// The key hints and a message. With `no_dock`, the screen is too small for
/// the mini-player, so the message can be the video that plays.
fn draw_status(app: &App, f: &mut Frame, area: Rect, no_dock: bool) {
    let t = &app.theme;
    let hint = |a: Action, label: &str| {
        app.keymap
            .first_key(a)
            .map(|k| vec![Span::styled(k.to_string(), t.key), Span::styled(format!(" {label}  "), t.key_label)])
    };
    let mut wanted = vec![(Action::Help, "help"), (Action::Search, "search")];
    if !app.screens.is_empty() {
        wanted.push((Action::Back, "back"));
    } else if app.tab == Tab::Search {
        wanted.push((Action::Filters, "filters"));
    }
    wanted.push((Action::Quit, "quit"));
    let hints: Vec<Span> = wanted.into_iter().filter_map(|(a, l)| hint(a, l)).flatten().collect();
    let hints = Line::from(hints).right_aligned();
    let message = app
        .status
        .clone()
        // Nothing else on screen shows the video, so it comes before lasting notes.
        .or_else(|| app.player.as_ref().filter(|_| no_dock).map(player::status_message))
        .or_else(|| match app.screens.last() {
            Some(screen) => screens::status(screen),
            None => (app.tab == Tab::Search).then(|| search::status(&app.search)).flatten(),
        })
        // A lasting note, so not a warning: it would stay on screen in every
        // tmux session.
        .or_else(|| app.graphics.message().map(StatusMessage::info));
    let message = message.map(|m| status_line(t, &m));
    // A message that does not fit next to the hints gets the whole bar.
    let hints_width = u16::try_from(hints.width()).unwrap_or(u16::MAX);
    let message_width = message.as_ref().map_or(0, |m| u16::try_from(m.width()).unwrap_or(u16::MAX));
    let hints_width = if message_width > area.width.saturating_sub(hints_width + 2) { 0 } else { hints_width };
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(hints_width)]).spacing(2).areas(area);
    if let Some(m) = message {
        f.render_widget(Paragraph::new(m), left);
    }
    if hints_width > 0 {
        f.render_widget(hints, right);
    }
}

/// A status bar message, marked and styled by its kind: "✓" for a change
/// that worked, "!" for a warning and "✗" for an error. The mark also shows
/// the kind without color.
pub(crate) fn status_line(t: &Theme, m: &StatusMessage) -> Line<'static> {
    let text = m.text.clone();
    match m.kind {
        StatusKind::Info => Line::from(text),
        StatusKind::Progress => Line::styled(text, t.status_progress),
        StatusKind::Success => Line::from(vec![Span::styled("✓", t.status_success), Span::from(" "), Span::from(text)]),
        StatusKind::Warn => {
            Line::from(vec![Span::styled("!", t.status_warn), Span::from(" "), Span::styled(text, t.title)])
        }
        StatusKind::Error => {
            let mark = t.status_error.add_modifier(Modifier::BOLD);
            Line::from(vec![Span::styled("✗", mark), Span::from(" "), Span::styled(text, t.status_error)])
        }
    }
}

/// The keys of the browse screen, then of the player. The player keys work
/// in the mini-player too, when browse does not use them.
fn draw_help(app: &App, f: &mut Frame, area: Rect) {
    let keys = |keys: &[crate::keymap::KeyPress]| keys.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
    // A heading has no keys.
    let mut entries: Vec<(String, Option<String>)> = vec![("Browse".into(), None)];
    entries.extend(app.keymap.bindings().iter().map(|(a, k)| (a.description().to_string(), Some(keys(k)))));
    entries.push(("Player".into(), None));
    entries.extend(app.keymap.player.bindings().iter().map(|(a, k)| (a.description().to_string(), Some(keys(k)))));
    let cells = |n: Option<usize>| u16::try_from(n.unwrap_or(0)).unwrap_or(u16::MAX);
    let description_width = cells(entries.iter().map(|(d, _)| d.width()).max());
    let keys_width = cells(entries.iter().map(|(_, k)| k.as_deref().map_or(0, str::width)).max());
    // Two spaces after each column.
    let column = description_width.saturating_add(keys_width).saturating_add(3);
    // As many columns as the height needs and the width allows.
    let fit = usize::from(area.height.saturating_sub(2)).max(1);
    let most = usize::from(area.width.saturating_sub(2) / column.max(1)).max(1);
    let columns = entries.len().div_ceil(fit).clamp(1, most);
    let per_column = entries.len().div_ceil(columns);
    let height = u16::try_from(per_column).unwrap_or(u16::MAX).saturating_add(2);
    let width = column.saturating_mul(u16::try_from(columns).unwrap_or(1)).saturating_add(2);
    let popup = centered(area, width, height);
    let t = &app.theme;
    let mut block = bordered(t, " Keys ");
    if per_column > fit {
        block = block.title_bottom(Line::styled(" More keys in a larger window ", t.muted));
    }
    let rows: Vec<Row> = entries
        .into_iter()
        .map(|(description, keys)| match keys {
            Some(keys) => Row::new([Line::from(description), Line::styled(keys, t.key)]),
            None => Row::new([Line::styled(description, t.title), Line::default()]),
        })
        .collect();
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    let ratio = u32::try_from(columns).unwrap_or(1);
    let areas = Layout::horizontal(vec![Constraint::Ratio(1, ratio); columns]).split(inner);
    let mut rows = rows.into_iter();
    for column in areas.iter() {
        let chunk: Vec<Row> = rows.by_ref().take(per_column).collect();
        f.render_widget(Table::new(chunk, [Constraint::Length(description_width), Constraint::Min(0)]), *column);
    }
}

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::screens::Target;
    use crate::app::thumbnails::Thumbnails;
    use crate::app::{Effect, Selection};
    use crate::app::{Input, Message};
    use crate::keymap::{KeyCode, KeyMap, KeyPress};
    use crate::source::model::{
        Channel, ChannelInfo, ChannelPage, ChannelRef, Chapter, Description, Item, ItemPage, Link, LinkTarget,
        Playlist, PlaylistKind, SearchPage, Video, VideoDetails, VideoKind,
    };
    use crate::store::Subscription;
    use crate::terminal::detect::Graphics;
    use crate::terminal::kitty;
    use crate::thumbnails::Picture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use std::sync::Arc;

    fn buffer(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| view(app, f)).unwrap();
        t.backend().buffer().clone()
    }

    /// Like `buffer`, but without the pass that removes colors for
    /// `NO_COLOR`, to test the theme alone.
    fn draw_only(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(app, f)).unwrap();
        t.backend().buffer().clone()
    }

    /// The screen with its styles. A run of cells with the same style shows
    /// as `{code}text{/}`, where the code has a color letter (R G Y B M C,
    /// `#` for an image) and modifier letters (b bold, d dim, r reversed,
    /// u underlined, i italic). Unstyled text shows bare, and image cells
    /// show as `▒`.
    fn styled(buf: &ratatui::buffer::Buffer) -> String {
        let code = |c: &ratatui::buffer::Cell| {
            let color = match c.fg {
                Color::Red => "R",
                Color::Green => "G",
                Color::Yellow => "Y",
                Color::Blue => "B",
                Color::Magenta => "M",
                Color::Cyan => "C",
                Color::Reset => "",
                Color::Rgb(..) => "#",
                _ => "?",
            };
            let mut mods = String::new();
            for (m, letter) in [
                (Modifier::BOLD, 'b'),
                (Modifier::DIM, 'd'),
                (Modifier::REVERSED, 'r'),
                (Modifier::UNDERLINED, 'u'),
                (Modifier::ITALIC, 'i'),
            ] {
                if c.modifier.contains(m) {
                    mods.push(letter);
                }
            }
            let bg = if c.bg == Color::Reset { "" } else { " bg?" };
            let code = [color, &mods].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
            format!("{code}{bg}")
        };
        let mut out = String::new();
        for y in 0..buf.area.height {
            let mut current = String::new();
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                let c = code(cell);
                if c != current {
                    if !current.is_empty() {
                        out.push_str("{/}");
                    }
                    if !c.is_empty() {
                        out.push('{');
                        out.push_str(&c);
                        out.push('}');
                    }
                    current = c;
                }
                out.push_str(if cell.symbol().starts_with(kitty::PLACEHOLDER) { "▒" } else { cell.symbol() });
            }
            if !current.is_empty() {
                out.push_str("{/}");
            }
            out.push('\n');
        }
        out
    }

    /// The screen as text. Image placeholder cells show as `▒`.
    fn render(app: &App, w: u16, h: u16) -> String {
        let buf = buffer(app, w, h);
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                let symbol = buf[(x, y)].symbol();
                out.push_str(if symbol.starts_with(kitty::PLACEHOLDER) { "▒" } else { symbol });
            }
            out.push('\n');
        }
        out
    }

    fn press(app: &mut App, s: &str) {
        app.update(Message::Input(Input::Key(s.parse().unwrap())));
    }

    #[test]
    fn browse_screen() {
        let app = App::new(KeyMap::default(), Graphics::KittyShm);
        insta::assert_snapshot!(render(&app, 60, 8));
    }

    #[test]
    fn browse_screen_without_video() {
        let mut app = App::new(KeyMap::default(), Graphics::Unavailable("herdr".into()));
        app.tab = Tab::Subscriptions;
        insta::assert_snapshot!(render(&app, 100, 8));
    }

    #[test]
    fn help_overlay() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "?");
        insta::assert_snapshot!(render(&app, 120, 24));
        // Smaller windows say that keys are missing.
        assert!(render(&app, 80, 24).contains("More keys in a larger window"));
        assert!(!render(&app, 120, 24).contains("More keys"));
    }

    fn sample_results() -> Vec<Item> {
        let chan = |name: &str| ChannelRef { id: Some(format!("UC{name}")), name: name.into(), handle: None };
        let thumb = |id: &str| {
            vec![crate::source::model::Thumbnail {
                url: format!("https://i.ytimg.com/vi/{id}/hq.jpg"),
                width: 336,
                height: 188,
            }]
        };
        let video = |id: &str, title: &str, kind, channels, members_only| {
            Item::Video(Video {
                id: id.into(),
                title: title.into(),
                kind,
                channels,
                duration_text: (kind != VideoKind::Live).then(|| "12:34".to_string()),
                duration_secs: None,
                views_text: Some(if kind == VideoKind::Live { "32 watching" } else { "1.2M views" }.into()),
                published_text: (kind != VideoKind::Live).then(|| "3 years ago".to_string()),
                thumbnails: thumb(id),
                members_only,
                badges: if members_only { Vec::new() } else { vec!["4K".into()] },
                description_snippet: None,
            })
        };
        vec![
            video("a", "Rust in 100 seconds", VideoKind::Normal, vec![chan("Fireship")], false),
            video("b", "Lofi beats to code to", VideoKind::Live, vec![chan("Lofi Girl")], false),
            video("c", "Async Rust with a friend", VideoKind::Normal, vec![chan("Jon Gjengset"), chan("Ana")], true),
            Item::Channel(Channel {
                id: "UCjon".into(),
                name: "Jon Gjengset".into(),
                handle: Some("@jonhoo".into()),
                subscribers_text: Some("111K subscribers".into()),
                description_snippet: None,
                verified: true,
                thumbnails: thumb("jon"),
            }),
            Item::Playlist(Playlist {
                id: "PLx".into(),
                title: "Crust of Rust".into(),
                kind: PlaylistKind::Playlist,
                channel: Some(chan("Jon Gjengset")),
                count_text: Some("44 videos".into()),
                thumbnails: thumb("crust"),
                first_video_id: None,
            }),
        ]
    }

    fn app_with_results(height: u16) -> App {
        search_app(height, false).0
    }

    /// An app with search results, and the effects of the reply.
    fn search_app(height: u16, thumbnails: bool) -> (App, Vec<Effect>) {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.thumbnails = Thumbnails::new(thumbnails);
        app.update(Message::Input(Input::Resize { width: 70, height }));
        press(&mut app, "/");
        for c in "rust".chars() {
            app.update(Message::Input(Input::Key(KeyPress::plain(KeyCode::Char(c)))));
        }
        press(&mut app, "enter");
        let page = SearchPage {
            items: sample_results(),
            estimated_results: Some(6_192_373),
            continuation: None,
            ..SearchPage::default()
        };
        let generation = app.search.generation;
        let effects = app.update(Message::SearchResult { generation, append: false, result: Ok(page) });
        (app, effects)
    }

    /// Checks that `buf` has no colors except in image cells, which keep
    /// theirs. Returns the number of image cells.
    fn assert_no_colors_but_images(buf: &ratatui::buffer::Buffer) -> usize {
        let mut images = 0;
        for cell in &buf.content {
            if cell.symbol().starts_with(kitty::PLACEHOLDER) {
                assert!(matches!(cell.fg, Color::Rgb(..)), "an image cell keeps its ID color");
                images += 1;
            } else {
                assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{:?}", cell.symbol());
            }
        }
        images
    }

    #[test]
    fn no_color_keeps_images_and_modifiers() {
        let (mut app, effects) = search_app(20, true);
        let url = effects
            .into_iter()
            .find_map(|e| match e {
                Effect::LoadThumbnail { url, .. } => Some(url),
                _ => None,
            })
            .unwrap();
        let slot = app.thumbnails.slot();
        let rgba = vec![0; (slot.width * slot.height * 4) as usize];
        let picture = Picture { width: slot.width, height: slot.height, cols: slot.cols, rows: slot.rows, rgba };
        app.update(Message::ThumbnailLoaded { url, slot, result: Ok(Arc::new(picture)) });
        let colored = buffer(&app, 70, 20);
        assert!(colored.content.iter().any(|c| c.fg != Color::Reset && !c.symbol().starts_with(kitty::PLACEHOLDER)));

        app.theme = crate::theme::Theme::no_color();
        let plain = buffer(&app, 70, 20);
        assert_eq!(assert_no_colors_but_images(&plain), usize::from(slot.cols * slot.rows));
        // The text and the modifiers stay the same.
        for (a, b) in colored.content.iter().zip(&plain.content) {
            assert_eq!((a.symbol(), a.modifier), (b.symbol(), b.modifier));
        }
        // The selected title (the first result) is still reversed. A LIVE pill
        // is reversed too, so the check is on the title.
        let y = (0..20)
            .find(|y| (0..70).map(|x| plain[(x, *y)].symbol()).collect::<String>().contains("Rust in 100"))
            .unwrap();
        let x = (0..70).find(|x| plain[(*x, y)].symbol() == "R").unwrap();
        assert!(plain[(x, y)].modifier.contains(Modifier::REVERSED), "the selection still shows");
    }

    /// Every image cell keeps its ID color and no style, and no other cell
    /// has a background or a color that only a truecolor terminal shows.
    fn assert_images_untouched(buf: &ratatui::buffer::Buffer) -> usize {
        let mut images = 0;
        for cell in &buf.content {
            if cell.symbol().starts_with(kitty::PLACEHOLDER) {
                assert!(matches!(cell.fg, Color::Rgb(..)), "an image cell keeps its ID color");
                assert_eq!((cell.bg, cell.modifier), (Color::Reset, Modifier::empty()), "an image cell has no style");
                images += 1;
            } else {
                assert_eq!(cell.bg, Color::Reset, "no background: {:?}", cell.symbol());
                assert!(
                    !matches!(cell.fg, Color::Rgb(..) | Color::Indexed(_)),
                    "only the 16 colors: {:?}",
                    cell.symbol()
                );
            }
        }
        images
    }

    #[test]
    fn no_theme_touches_images() {
        use crate::theme::Theme;
        for theme in [Theme::ansi(), Theme::no_color()] {
            let (mut app, effects) = search_app(20, true);
            for url in effects.into_iter().filter_map(|e| match e {
                Effect::LoadThumbnail { url, .. } => Some(url),
                _ => None,
            }) {
                let slot = app.thumbnails.slot();
                let rgba = vec![0; (slot.width * slot.height * 4) as usize];
                let picture =
                    Picture { width: slot.width, height: slot.height, cols: slot.cols, rows: slot.rows, rgba };
                app.update(Message::ThumbnailLoaded { url, slot, result: Ok(Arc::new(picture)) });
            }
            app.theme = theme;
            // The first result is selected, and its row has an image.
            assert!(assert_images_untouched(&draw_only(&app, 70, 20)) > 0);
            let mut app = playing_app();
            app.theme = theme;
            assert!(assert_images_untouched(&draw_only(&app, 40, 12)) > 0);
            let mut app = mini_app(100, 24, true);
            app.theme = theme;
            assert!(assert_images_untouched(&draw_only(&app, 100, 24)) > 0);
        }
    }

    #[test]
    fn the_no_color_theme_alone_has_no_colors() {
        let mut app = playing_app();
        app.theme = crate::theme::Theme::no_color();
        let g = app.player.as_ref().unwrap().geometry;
        assert_eq!(assert_no_colors_but_images(&draw_only(&app, 40, 12)), usize::from(g.cols * g.rows));
        let (mut app, _) = search_app(16, false);
        app.theme = crate::theme::Theme::no_color();
        assert_no_colors_but_images(&draw_only(&app, 70, 16));
    }

    #[test]
    fn styles_by_role() {
        let (mut app, _) = search_app(16, false);
        app.status = Some(StatusMessage::success("Saved Crust of Rust."));
        insta::assert_snapshot!("search_results_styled", styled(&draw_only(&app, 70, 16)));
    }

    #[test]
    fn status_marks_by_kind() {
        let t = crate::theme::Theme::ansi();
        let line = |m: StatusMessage| status_line(&t, &m);
        let text = |l: &Line| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert_eq!(text(&line(StatusMessage::info("Note"))), "Note");
        assert_eq!(text(&line(StatusMessage::progress("Loading…"))), "Loading…");
        assert_eq!(line(StatusMessage::progress("Loading…")).style, t.status_progress);
        let ok = line(StatusMessage::success("Saved"));
        assert_eq!((text(&ok), ok.spans[0].style), ("✓ Saved".to_string(), t.status_success));
        let warn = line(StatusMessage::warn("No chapters"));
        assert_eq!((text(&warn), warn.spans[0].style), ("! No chapters".to_string(), t.status_warn));
        let error = line(StatusMessage::error("Failed"));
        assert_eq!(text(&error), "✗ Failed");
        assert_eq!(error.spans[2].style, t.status_error);
    }

    #[test]
    fn the_tab_row_keeps_the_selected_tab() {
        // " Search " 8, " Feed " 6, " Subscriptions " 15, " Playlists " 11, " Queue " 7, " History " 9.
        let widths = [8, 6, 15, 11, 7, 9];
        assert_eq!(tab_window(&widths, 0, 60), 0..6, "all fit: 56 + 5 dividers - 1");
        assert_eq!(tab_window(&widths, 5, 59), 1..6);
        assert_eq!(tab_window(&widths, 0, 50), 0..4);
        assert_eq!(tab_window(&widths, 5, 50), 2..6);
        assert_eq!(tab_window(&widths, 0, 40), 0..3);
        assert_eq!(tab_window(&widths, 3, 40), 1..4);
        assert_eq!(tab_window(&widths, 4, 40), 2..5);
        assert_eq!(tab_window(&widths, 5, 40), 3..6);
        // Too narrow for even the selected tab: it shows, cut off.
        assert_eq!(tab_window(&widths, 2, 4), 2..3);
        assert_eq!(tab_window(&[], 0, 10), 0..0);
    }

    #[test]
    fn narrow_tab_rows() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "shift-tab");
        insta::assert_snapshot!("tabs_history_40", render(&app, 40, 10));
        insta::assert_snapshot!("tabs_history_50", render(&app, 50, 8));
    }

    #[test]
    fn no_color_keeps_the_video() {
        let mut app = playing_app();
        app.theme = crate::theme::Theme::no_color();
        let buf = buffer(&app, 40, 12);
        let g = app.player.as_ref().unwrap().geometry;
        assert_eq!(assert_no_colors_but_images(&buf), usize::from(g.cols * g.rows));
        let (r, gr, b) = kitty::id_color(kitty::VIDEO_ID);
        assert_eq!(buf[(g.x, g.y)].fg, Color::Rgb(r, gr, b));
    }

    #[test]
    fn search_results_with_thumbnails() {
        let (mut app, effects) = search_app(20, true);
        let urls: Vec<String> = effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::LoadThumbnail { url, .. } => Some(url),
                _ => None,
            })
            .collect();
        assert_eq!(urls.len(), 5);
        let slot = app.thumbnails.slot();
        let rgba = vec![0; (slot.width * slot.height * 4) as usize];
        let picture = Picture { width: slot.width, height: slot.height, cols: slot.cols, rows: slot.rows, rgba };
        for url in [&urls[0], &urls[3]] {
            let result = Ok(Arc::new(picture.clone()));
            app.update(Message::ThumbnailLoaded { url: url.clone(), slot, result });
        }
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 20));

        // Each cell names its row and column, and the color names the image.
        let buf = buffer(&app, 70, 20);
        let first = (0..20).find(|y| buf[(1, *y)].symbol().starts_with(kitty::PLACEHOLDER)).unwrap();
        assert_eq!(buf[(1, first)].symbol(), kitty::cell(0, 0).unwrap());
        assert_eq!(buf[(2, first + 1)].symbol(), kitty::cell(1, 1).unwrap());
        let (r, g, b) = kitty::id_color(app.thumbnails.image(&app.search.results[0]).unwrap());
        assert_eq!(buf[(1, first)].fg, Color::Rgb(r, g, b));

        // Item 1 is selected. The highlight covers its text but not its image.
        let selected = first + 3;
        // x = 13 is the LIVE pill, which is reversed too, so check the title.
        assert!(buf[(20, selected)].modifier.contains(Modifier::REVERSED), "the text is highlighted");
        assert!(buf[(1, selected)].symbol().is_empty() || buf[(1, selected)].symbol() == " ");
        app.update(Message::ThumbnailLoaded { url: urls[1].clone(), slot, result: Ok(Arc::new(picture.clone())) });
        let buf = buffer(&app, 70, 20);
        let (r, g, b) = kitty::id_color(app.thumbnails.image(&app.search.results[1]).unwrap());
        for x in 1..=slot.cols {
            let cell = &buf[(x, selected)];
            assert!(cell.symbol().starts_with(kitty::PLACEHOLDER));
            assert!(!cell.modifier.contains(Modifier::REVERSED), "the highlight leaves images alone");
            assert_eq!(cell.fg, Color::Rgb(r, g, b));
        }
    }

    #[test]
    fn partial_items_get_no_picture() {
        // The list has 11 rows: three whole items and two rows of a fourth.
        let (mut app, effects) = search_app(16, true);
        let slot = app.thumbnails.slot();
        let rgba = vec![0; (slot.width * slot.height * 4) as usize];
        let picture = Picture { width: slot.width, height: slot.height, cols: slot.cols, rows: slot.rows, rgba };
        for e in effects {
            if let Effect::LoadThumbnail { url, .. } = e {
                app.update(Message::ThumbnailLoaded { url, slot, result: Ok(Arc::new(picture.clone())) });
            }
        }
        let buf = buffer(&app, 70, 16);
        let rows_with_pictures = (0..16).filter(|y| buf[(1, *y)].symbol().starts_with(kitty::PLACEHOLDER)).count();
        assert_eq!(rows_with_pictures, 9);
    }

    #[test]
    fn narrow_lists_drop_thumbnails() {
        let (app, _) = search_app(12, true);
        let mut t = Terminal::new(TestBackend::new(25, 6)).unwrap();
        t.draw(|f| {
            items::draw_list(&app.theme, &app.search.results, Selection::default(), &app.thumbnails, &[], f, f.area());
        })
        .unwrap();
        let buf = t.backend().buffer();
        assert!(buf.content().iter().all(|c| !c.symbol().starts_with(kitty::PLACEHOLDER)));
        // Still three lines for each item: the second one starts on row 3.
        assert_eq!(buf[(0, 3)].symbol(), "L");
    }

    #[test]
    fn search_results() {
        let mut app = app_with_results(16);
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 16));
    }

    #[test]
    fn search_results_scroll_with_the_selection() {
        let mut app = app_with_results(10);
        press(&mut app, "G");
        insta::assert_snapshot!(render(&app, 70, 10));
    }

    #[test]
    fn typing_a_query() {
        let mut app = app_with_results(16);
        press(&mut app, "/");
        press(&mut app, "backspace");
        insta::assert_snapshot!(render(&app, 70, 16));
    }

    #[test]
    fn filter_menu() {
        let mut app = app_with_results(16);
        press(&mut app, "f");
        press(&mut app, "enter");
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 16));
    }

    #[test]
    fn search_error() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "/");
        app.update(Message::Input(Input::Key(KeyPress::plain(KeyCode::Char('x')))));
        press(&mut app, "enter");
        let generation = app.search.generation;
        app.update(Message::SearchResult { generation, append: false, result: Err("network: timed out".into()) });
        insta::assert_snapshot!(render(&app, 70, 8));
    }

    fn channel_info() -> ChannelInfo {
        ChannelInfo {
            id: "UCjon".into(),
            name: "Jon Gjengset".into(),
            handle: Some("@jonhoo".into()),
            subscribers_text: Some("111K subscribers".into()),
            videos_text: Some("120 videos".into()),
            description: Some("\nRust streams and videos.\nMore text.".into()),
            verified: true,
            avatar: Vec::new(),
        }
    }

    #[test]
    fn channel_screen() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 70, height: 12 }));
        app.open(Target::Channel { id: "UCjon".into(), name: "Jon".into() });
        app.subscriptions.push(Subscription {
            channel_id: "UCjon".into(),
            name: "Jon".into(),
            handle: None,
            subscribed_at: 0,
        });
        let videos = ItemPage { items: sample_results().into_iter().take(2).collect(), continuation: Some("t".into()) };
        app.update(Message::ChannelLoaded { request: 1, result: Ok(ChannelPage { info: channel_info(), videos }) });
        insta::assert_snapshot!(render(&app, 70, 12));
    }

    #[test]
    fn playlist_screen_loading_then_failed() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.open(Target::Playlist { id: "PLx".into(), title: "Crust of Rust".into() });
        let loading = render(&app, 70, 8);
        app.update(Message::PlaylistLoaded { request: 1, result: Err("HTTP 404: Not Found".into()) });
        insta::assert_snapshot!(format!("{loading}{}", render(&app, 70, 8)));
    }

    fn a_comment(id: &str, name: &str, text: &str) -> crate::source::model::Comment {
        crate::source::model::Comment {
            id: id.into(),
            author: ChannelRef { id: Some("UC1".into()), name: name.into(), handle: None },
            text: Description { text: text.into(), links: Vec::new() },
            published_text: Some("6 years ago".into()),
            edited: false,
            likes_text: Some("4.8M".into()),
            reply_count_text: Some("986".into()),
            replies: Some("r".into()),
            pinned: false,
            hearted: false,
            by_uploader: false,
            verified: false,
            reply_level: u8::from(id.contains('.')),
        }
    }

    #[test]
    fn comments_pane() {
        use crate::app::comments::CommentsSlot;
        use crate::source::model::{CommentPage, CommentSort};
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 60, height: 24 }));
        app.open(Target::Video { id: "v".into(), title: "Me at the zoo".into() });
        let generation = match app.update(Message::Input(Input::Key("C".parse().unwrap()))).as_slice() {
            [Effect::LoadComments { generation, .. }] => *generation,
            other => panic!("{other:?}"),
        };
        insta::assert_snapshot!("comments_loading", render(&app, 60, 8));
        let pinned = crate::source::model::Comment {
            pinned: true,
            hearted: true,
            verified: true,
            ..a_comment("a", "@SanDiegoZoo", "We're so honored that the first ever YouTube video was filmed here!")
        };
        let long = (1..=9).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let uploader = crate::source::model::Comment {
            by_uploader: true,
            edited: true,
            likes_text: None,
            reply_count_text: None,
            replies: None,
            ..a_comment("b", "@jawed", &long)
        };
        let sort = |label: &str, selected| CommentSort { label: label.into(), selected, continuation: "t".into() };
        let page = CommentPage {
            comments: vec![pinned, uploader, a_comment("c", "@third", "Third")],
            continuation: None,
            count_text: Some("10,645,363 Comments".into()),
            sorts: vec![sort("Top", true), sort("Newest", false)],
            note: None,
        };
        app.update(Message::CommentsLoaded { request: 1, generation, slot: CommentsSlot::First, result: Ok(page) });
        app.update(Message::Input(Input::Key("enter".parse().unwrap())));
        let replies = CommentPage {
            comments: vec![a_comment("a.1", "@reply", "Very cool!")],
            continuation: Some("more".into()),
            ..CommentPage::default()
        };
        let slot = CommentsSlot::Replies { parent: "a".into() };
        app.update(Message::CommentsLoaded { request: 1, generation, slot, result: Ok(replies) });
        insta::assert_snapshot!("comments_pane", render(&app, 60, 24));
        insta::assert_snapshot!("comments_pane_styled", styled(&draw_only(&app, 60, 24)));
        // The selected comment shows in full, and the list follows it.
        for _ in 0..3 {
            app.update(Message::Input(Input::Key("j".parse().unwrap())));
        }
        let text = render(&app, 60, 24);
        assert!(text.contains("line 9"), "{text}");
        assert!(!text.contains("SanDiegoZoo"), "scrolled past the first comment: {text}");
        // A note when there are no comments.
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.open(Target::Video { id: "v".into(), title: "t".into() });
        app.update(Message::Input(Input::Key("C".parse().unwrap())));
        let page = CommentPage { note: Some("Comments are turned off.".into()), ..CommentPage::default() };
        app.update(Message::CommentsLoaded { request: 1, generation: 2, slot: CommentsSlot::First, result: Ok(page) });
        assert!(render(&app, 60, 8).contains("Comments are turned off."));
    }

    #[test]
    fn the_selected_link_is_reversed_and_scrolled_into_view() {
        use crate::app::screens::VideoScreen;
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 40, height: 10 }));
        app.open(Target::Video { id: "v".into(), title: "t".into() });
        let mut text = (1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        text.push('\n');
        text.push_str("see x.com");
        let start = text.chars().count() - 5;
        let mut details = crate::app::tests::details("v", None);
        details.description =
            Description { text, links: vec![Link { start, len: 5, target: LinkTarget::Url("https://x.com".into()) }] };
        app.update(Message::VideoLoaded { request: 1, result: Ok(details) });
        render(&app, 40, 10);
        press(&mut app, "n");
        let buf = buffer(&app, 40, 10);
        let screen = render(&app, 40, 10);
        assert!(screen.contains("see x.com"), "{screen}");
        let row =
            (0..10).find(|&y| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>().contains("see x.com")).unwrap();
        let x = (0..40).find(|&x| buf[(x, row)].symbol() == "x").unwrap();
        assert!(buf[(x, row)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(x - 2, row)].modifier.contains(Modifier::REVERSED));
        let Some(crate::app::screens::Screen::Video(VideoScreen { link, .. })) = app.screens.last() else { panic!() };
        assert_eq!(*link, Some(0));
        assert!(screen.lines().last().unwrap().contains("enter: open https://x.com"), "{screen}");

        // A link at the end of a long paragraph that wraps.
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 40, height: 10 }));
        app.open(Target::Video { id: "v".into(), title: "t".into() });
        let text = format!("{}see x.com", "word ".repeat(80));
        let start = text.chars().count() - 5;
        let mut details = crate::app::tests::details("v", None);
        details.description =
            Description { text, links: vec![Link { start, len: 5, target: LinkTarget::Url("https://x.com".into()) }] };
        app.update(Message::VideoLoaded { request: 1, result: Ok(details) });
        render(&app, 40, 10);
        press(&mut app, "n");
        let screen = render(&app, 40, 10);
        assert!(screen.contains("x.com"), "{screen}");
    }

    #[test]
    fn video_details_screen() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.open(Target::Video { id: "aircAruvnKk".into(), title: "But what is a neural network?".into() });
        let text = "Learn more: 3b1b.co/nn\n0:00 Intro\nThanks!".to_string();
        let details = VideoDetails {
            id: "aircAruvnKk".into(),
            title: "But what is a neural network?".into(),
            channel: Some(ChannelRef { id: Some("UCYO".into()), name: "3Blue1Brown".into(), handle: None }),
            subscribers_text: Some("8.73M subscribers".into()),
            views_text: Some("25,037,755 views".into()),
            date_text: Some("Oct 5, 2017".into()),
            publish_date: None,
            duration_secs: Some(1112),
            likes_text: Some("568K".into()),
            live: false,
            description: Description {
                text,
                links: vec![
                    Link { start: 12, len: 10, target: LinkTarget::Url("https://3b1b.co/nn".into()) },
                    Link {
                        start: 23,
                        len: 4,
                        target: LinkTarget::Video { id: "aircAruvnKk".into(), start_secs: Some(0) },
                    },
                ],
            },
            chapters: vec![
                Chapter { title: "Introduction".into(), start_secs: 0 },
                Chapter { title: "Series preview".into(), start_secs: 67 },
            ],
            thumbnails: Vec::new(),
            related: Vec::new(),
            comments: None,
            comments_count_text: None,
        };
        app.update(Message::VideoLoaded { request: 1, result: Ok(details) });
        insta::assert_snapshot!(render(&app, 70, 18));

        // At 10 rows the text is taller than the screen. G goes to the end,
        // and j cannot go past it.
        render(&app, 70, 10);
        press(&mut app, "G");
        press(&mut app, "j");
        let Some(crate::app::screens::Screen::Video(s)) = app.screens.last() else { panic!("no video screen") };
        let max = s.max_scroll.get();
        assert!(max > 0 && max < 20, "max {max}");
        assert_eq!(s.scroll.get(), max);
        press(&mut app, "k");
        let Some(crate::app::screens::Screen::Video(s)) = app.screens.last() else { panic!("no video screen") };
        assert_eq!(s.scroll.get(), max - 1);
    }

    #[test]
    fn subscriptions_tab() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.subscriptions = vec![
            Subscription {
                channel_id: "UC1".into(),
                name: "3Blue1Brown".into(),
                handle: Some("@3blue1brown".into()),
                subscribed_at: 0,
            },
            Subscription {
                channel_id: "UC2".into(),
                name: "Jon Gjengset".into(),
                handle: Some("@jonhoo".into()),
                subscribed_at: 0,
            },
        ];
        app.tab = Tab::Subscriptions;
        press(&mut app, "j");
        insta::assert_snapshot!(render(&app, 70, 10));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut app = app_with_results(3);
        press(&mut app, "f");
        render(&app, 4, 3);
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "?");
        render(&app, 5, 2);
    }

    fn playing_app() -> App {
        use crate::app::player::PlayerEvent;
        use crate::source::streams::{FormatInfo, Streams};
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 40, height: 12 }));
        app.update(Message::Input(Input::CellSize { width: 10, height: 20 }));
        app.play("v1".into(), "Big Buck Bunny".into());
        let streams = Streams {
            video_url: "https://v".into(),
            audio_url: None,
            headers: Vec::new(),
            available_at: None,
            expires_at: None,
            format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
            chapters: Vec::new(),
            channel: None,
            duration_secs: None,
            live: false,
            qualities: Vec::new(),
            captions: Vec::new(),
        };
        app.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(Box::new(streams)) });
        for event in [PlayerEvent::FileLoaded, PlayerEvent::Duration(635_000), PlayerEvent::Position(65_000)] {
            app.update(Message::Player { session: 1, event });
        }
        app
    }

    #[test]
    fn player_shows_the_video_and_progress() {
        let mut app = playing_app();
        insta::assert_snapshot!(render(&app, 40, 12));
        // The video cells name the video image.
        let buf = buffer(&app, 40, 12);
        let g = app.player.as_ref().unwrap().geometry;
        let (r, gr, b) = kitty::id_color(kitty::VIDEO_ID);
        for (x, y) in [(g.x, g.y), (g.x + g.cols - 1, g.y + g.rows - 1)] {
            assert_eq!(buf[(x, y)].fg, Color::Rgb(r, gr, b));
            assert!(buf[(x, y)].symbol().starts_with(kitty::PLACEHOLDER));
        }
        app.update(Message::Player { session: 1, event: crate::app::player::PlayerEvent::Paused(true) });
        app.update(Message::Player { session: 1, event: crate::app::player::PlayerEvent::Muted(true) });
        let text = render(&app, 80, 12);
        assert!(text.contains("Paused  720p  muted"), "{text}");
    }

    #[test]
    fn the_progress_bar_marks_skipped_parts() {
        use crate::sponsorblock::{Category, Segment};
        let mut app = playing_app();
        let event = |e| Message::Player { session: 1, event: e };
        app.update(event(crate::app::player::PlayerEvent::Duration(100_000)));
        app.update(event(crate::app::player::PlayerEvent::Position(50_000)));
        let segment =
            |start_ms, end_ms| Segment { category: Category::Sponsor, start_ms, end_ms, video_duration_ms: None };
        app.update(Message::SegmentsLoaded {
            session: 1,
            result: Ok(vec![segment(20_000, 30_000), segment(70_000, 80_000)]),
        });
        app.status = Some(StatusMessage::warn("This video has no chapters."));
        insta::assert_snapshot!("player_progress_styled", styled(&draw_only(&app, 40, 12)));
        app.status = None;
        let buf = buffer(&app, 40, 12);
        // " 0:50 " comes first, then a bar of 40 - 6 - 6 = 28 cells for 100 s.
        let y = 10;
        let cell = |secs: u16| &buf[(6 + secs * 28 / 100, y)];
        let t = app.theme;
        // Skipped parts have glyphs of their own, so they show without color too.
        for (secs, style, symbol) in [
            (10, t.progress_played, "━"),
            (25, t.progress_sponsor, "┅"),
            (60, t.progress_track, "─"),
            (75, t.progress_sponsor, "┄"),
        ] {
            assert_eq!(cell(secs).symbol(), symbol, "at {secs} s");
            assert!(has_style(cell(secs), style), "at {secs} s: {:?}", cell(secs));
        }
    }

    /// Whether `cell` shows `style`: its colors (or none), and its
    /// modifiers.
    fn has_style(cell: &ratatui::buffer::Cell, style: ratatui::style::Style) -> bool {
        cell.fg == style.fg.unwrap_or(Color::Reset)
            && cell.bg == style.bg.unwrap_or(Color::Reset)
            && cell.modifier.contains(style.add_modifier)
            && !cell.modifier.intersects(style.sub_modifier)
    }

    #[test]
    fn player_before_and_after_playback() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 50, height: 8 }));
        app.play("v1".into(), "Big Buck Bunny".into());
        insta::assert_snapshot!("player_resolving", render(&app, 50, 8));
        let streams = crate::source::streams::Streams {
            video_url: "https://v".into(),
            audio_url: None,
            headers: Vec::new(),
            available_at: Some(115),
            expires_at: None,
            format: crate::source::streams::FormatInfo::default(),
            chapters: Vec::new(),
            channel: None,
            duration_secs: None,
            live: false,
            qualities: Vec::new(),
            captions: Vec::new(),
        };
        app.update(Message::StreamsResolved { session: 1, now: 100, result: Ok(Box::new(streams)) });
        insta::assert_snapshot!("player_waiting", render(&app, 50, 8));
        app.update(Message::Tick { session: 1, now: 115 });
        app.update(Message::Player {
            session: 1,
            event: crate::app::player::PlayerEvent::Failed("HTTP error 404".into()),
        });
        insta::assert_snapshot!("player_failed", render(&app, 50, 8));

        // A long error wraps and is shown in full.
        app.play("v2".into(), "Big Buck Bunny".into());
        let long = "yt-dlp failed: ERROR: [youtube] v2: Sign in to confirm your age. This video may be inappropriate";
        app.update(Message::StreamsResolved { session: 2, now: 0, result: Err(long.into()) });
        let text = render(&app, 30, 12);
        assert!(text.contains("inappropriate"), "{text}");
    }

    #[test]
    fn player_with_chapters_captions_and_the_quality_menu() {
        use crate::app::player::PlayerEvent;
        use crate::source::model::Chapter;
        use crate::source::streams::{Caption, FormatInfo, Quality, Streams};
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.update(Message::Input(Input::Resize { width: 60, height: 14 }));
        app.captions_on = true;
        app.play("v1".into(), "Big Buck Bunny".into());
        let streams = Streams {
            video_url: "https://v".into(),
            audio_url: Some("https://a".into()),
            headers: Vec::new(),
            available_at: None,
            expires_at: None,
            format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
            chapters: vec![Chapter { title: "Intro".into(), start_secs: 0 }],
            channel: None,
            duration_secs: None,
            live: false,
            qualities: [360, 720]
                .into_iter()
                .map(|res| Quality { res, long: res * 16 / 9, url: String::new(), available_at: None })
                .collect(),
            captions: vec![Caption { lang: "en".into(), name: "English".into(), url: String::new(), auto: false }],
        };
        app.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(Box::new(streams)) });
        for event in [PlayerEvent::FileLoaded, PlayerEvent::Speed(150), PlayerEvent::Duration(60_000)] {
            app.update(Message::Player { session: 1, event });
        }
        press(&mut app, "v");
        insta::assert_snapshot!(render(&app, 60, 14));
    }

    #[test]
    fn player_messages_take_the_whole_status_line() {
        let mut app = playing_app();
        app.status = Some(StatusMessage::warn("This video has no captions in \"en\"."));
        let text = render(&app, 40, 12);
        assert!(text.contains("This video has no captions in \"en\"."), "{text}");
    }

    #[test]
    fn queue_tab() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        app.tab = Tab::Queue;
        insta::assert_snapshot!("queue_tab_empty", render(&app, 60, 8));
        app.queue = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(v) => Some(v),
                _ => None,
            })
            .take(2)
            .collect();
        insta::assert_snapshot!("queue_tab", render(&app, 60, 10));
    }

    /// Search results, and a video in the mini-player, on a `width` x `height` screen.
    fn mini_app(width: u16, height: u16, streams: bool) -> App {
        use crate::app::player::PlayerEvent;
        use crate::source::streams::{FormatInfo, Streams};
        let mut app = app_with_results(height);
        app.update(Message::Input(Input::Resize { width, height }));
        app.update(Message::Input(Input::CellSize { width: 10, height: 20 }));
        app.play("v1".into(), "Big Buck Bunny".into());
        if streams {
            let streams = Streams {
                video_url: "https://v".into(),
                audio_url: None,
                headers: Vec::new(),
                available_at: None,
                expires_at: None,
                format: FormatInfo { width: Some(1280), height: Some(720), ..FormatInfo::default() },
                chapters: Vec::new(),
                channel: None,
                duration_secs: None,
                live: false,
                qualities: Vec::new(),
                captions: Vec::new(),
            };
            app.update(Message::StreamsResolved { session: 1, now: 0, result: Ok(Box::new(streams)) });
            for event in [PlayerEvent::FileLoaded, PlayerEvent::Duration(635_000), PlayerEvent::Position(65_000)] {
                app.update(Message::Player { session: 1, event });
            }
        }
        press(&mut app, "esc");
        app
    }

    #[test]
    fn mini_player_next_to_search_results() {
        let app = mini_app(100, 24, true);
        insta::assert_snapshot!(render(&app, 100, 24));
        // The video cells are in the box, and nothing else names the video image.
        let buf = buffer(&app, 100, 24);
        let g = app.player.as_ref().unwrap().geometry;
        let (r, gr, b) = kitty::id_color(kitty::VIDEO_ID);
        let mut cells = 0;
        for y in 0..24 {
            for x in 0..100 {
                if buf[(x, y)].fg == Color::Rgb(r, gr, b) && buf[(x, y)].symbol().starts_with(kitty::PLACEHOLDER) {
                    assert!((g.x..g.x + g.cols).contains(&x) && (g.y..g.y + g.rows).contains(&y), "{x},{y}");
                    cells += 1;
                }
            }
        }
        assert_eq!(cells, g.cols * g.rows);
    }

    #[test]
    fn video_cells_follow_what_view_draws() {
        use crate::app::player::PlayerEvent;
        let app = playing_app();
        let g = app.player.as_ref().unwrap().geometry;
        assert_eq!(video_cells(&app), Some((g.cols, g.rows)));
        // The mini-player on a screen with room for the dock, then without.
        let mut app = mini_app(100, 24, true);
        let g = app.player.as_ref().unwrap().geometry;
        assert_eq!(video_cells(&app), Some((g.cols, g.rows)));
        assert_eq!(video_cells(&mini_app(60, 16, true)), None);
        // Before mpv runs, and the up next list.
        assert_eq!(video_cells(&mini_app(100, 24, false)), None);
        let videos = sample_results().into_iter().filter_map(|i| match i {
            Item::Video(v) if v.kind == VideoKind::Normal => Some(v),
            _ => None,
        });
        app.update(Message::RelatedLoaded { session: 1, result: Ok(videos.collect()) });
        app.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        assert_eq!(video_cells(&app), None);
    }

    #[test]
    fn mini_player_before_and_after_playback() {
        use crate::app::player::PlayerEvent;
        let app = mini_app(80, 16, false);
        insta::assert_snapshot!("mini_player_resolving", render(&app, 80, 16));
        let mut app = mini_app(80, 16, true);
        let videos: Vec<Video> = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(v) if v.kind == VideoKind::Normal => Some(v),
                _ => None,
            })
            .collect();
        app.update(Message::RelatedLoaded { session: 1, result: Ok(videos) });
        app.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        insta::assert_snapshot!("mini_player_up_next", render(&app, 80, 16));
    }

    #[test]
    fn a_small_screen_shows_the_mini_player_in_the_status_bar() {
        let app = mini_app(60, 16, true);
        let text = render(&app, 60, 16);
        assert!(text.lines().last().unwrap().contains("▶ Big Buck Bunny 1:05 / 10:35"), "{text}");
        assert!(!text.contains('▒'), "no video shows: {text}");
        // A failure shows, in red.
        let mut app = mini_app(60, 16, false);
        let result = Err("private video".into());
        app.update(Message::StreamsResolved { session: 1, now: 0, result });
        let buf = buffer(&app, 60, 16);
        let last: String = (0..60).map(|x| buf[(x, 15)].symbol()).collect();
        assert!(last.contains("Cannot play Big Buck Bunny: private video"), "{last}");
        assert_eq!(buf[(0, 15)].symbol(), "✗");
        assert!(has_style(&buf[(2, 15)], app.theme.status_error));
        // Before the streams, it says so.
        let text = render(&mini_app(60, 16, false), 60, 16);
        assert!(text.contains("▶ Big Buck Bunny · finding the video streams…"), "{text}");
    }

    #[test]
    fn player_up_next() {
        use crate::app::player::PlayerEvent;
        let mut app = playing_app();
        let videos: Vec<Video> = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(v) if v.kind == VideoKind::Normal => Some(v),
                _ => None,
            })
            .collect();
        app.update(Message::RelatedLoaded { session: 1, result: Ok(videos) });
        app.update(Message::Player { session: 1, event: PlayerEvent::Ended });
        insta::assert_snapshot!(render(&app, 60, 12));
    }

    #[test]
    fn player_hints_leave_out_unbound_keys() {
        use crate::keymap::{KeyOverrides, PlayerAction, PlayerKeyOverrides};
        let mut app = playing_app();
        let text = render(&app, 80, 12);
        assert!(text.contains("space pause"), "{text}");
        let player = PlayerKeyOverrides::from([(PlayerAction::TogglePause, Vec::new())]);
        app.keymap = KeyMap::new(&KeyOverrides::new(), &player).unwrap();
        let text = render(&app, 80, 12);
        assert!(!text.contains("pause"), "{text}");
        assert!(text.contains("q back"), "{text}");
    }

    #[test]
    fn hints_leave_out_unbound_keys() {
        assert_eq!(hints([(Some("j"), "down"), (None, "up"), (Some("enter"), "play")]), "j down · enter play");
        assert_eq!(hints([(None::<&str>, "up")]), "");
    }

    #[test]
    fn history_tab() {
        use crate::store::HistoryEntry;
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "shift-tab");
        insta::assert_snapshot!("history_tab_empty", render(&app, 60, 8));
        let now = 1_793_373_867;
        app.clock = now;
        let entry = |id: &str, position_secs, finished, duration_secs, ago| HistoryEntry {
            video_id: id.into(),
            title: format!("Video {id}"),
            channel_id: None,
            channel_name: Some("Fireship".into()),
            duration_secs,
            position_secs,
            finished,
            watched_at: now - ago,
        };
        app.history = vec![
            entry("a", 252, false, Some(635), 90),
            entry("b", 0, true, Some(100), 3 * 3600),
            entry("c", 70, false, None, 40 * 86_400),
        ];
        insta::assert_snapshot!("history_tab", render(&app, 60, 10));
    }

    #[test]
    fn lists_mark_watched_videos() {
        use crate::store::HistoryEntry;
        let entry = |id: &str, position_secs, finished| HistoryEntry {
            video_id: id.into(),
            title: String::new(),
            channel_id: None,
            channel_name: None,
            duration_secs: Some(754),
            position_secs,
            finished,
            watched_at: 1,
        };
        let mut app = search_app(16, false).0;
        app.history = vec![entry("a", 252, false), entry("UCjon", 0, true)];
        insta::assert_snapshot!("search_results_watched", render(&app, 70, 16));
        app.history = vec![entry("a", 0, true), entry("b", 0, false)];
        let text = render(&app, 70, 16);
        assert!(text.contains("Fireship · watched · 12:34"), "{text}");
        assert!(text.contains("Lofi Girl · played"), "{text}");
    }

    #[test]
    fn feed_tab() {
        use crate::store::Subscription;
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "tab");
        insta::assert_snapshot!("feed_tab_empty", render(&app, 60, 8));
        app.subscriptions =
            vec![Subscription { channel_id: "UCF".into(), name: "Fireship".into(), handle: None, subscribed_at: 1 }];
        let videos: Vec<Video> = sample_results()
            .into_iter()
            .filter_map(|i| match i {
                Item::Video(mut v) => {
                    v.published_text = Some("1 day ago".into());
                    Some(v)
                }
                _ => None,
            })
            .collect();
        app.update(Message::FeedLoading { count: 2 });
        app.update(Message::FeedChannelLoaded { channel_id: "UCF".into(), fetched_at: 1_000_000, result: Ok(videos) });
        insta::assert_snapshot!("feed_tab", render(&app, 60, 12));
    }
}
