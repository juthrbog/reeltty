//! Draws the `App` state. Pure: the same state always gives the same frame.

mod search;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, Tabs, Wrap};

use crate::app::{App, Tab};
use crate::keymap::Action;

pub fn view(app: &App, f: &mut Frame) {
    let [tabs, body, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    draw_tabs(app, f, tabs);
    draw_body(app, f, body);
    draw_status(app, f, status);
    if app.show_help {
        draw_help(app, f, body);
    }
}

fn draw_tabs(app: &App, f: &mut Frame, area: Rect) {
    let titles = Tab::ALL.iter().map(|t| t.title());
    let selected = Tab::ALL.iter().position(|t| *t == app.tab);
    let tabs = Tabs::new(titles)
        .select(selected)
        .highlight_style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED))
        .divider(" ");
    f.render_widget(tabs, area);
}

fn draw_body(app: &App, f: &mut Frame, area: Rect) {
    let text = match app.tab {
        Tab::Search => return search::draw(app, f, area),
        Tab::Subscriptions => "No subscriptions yet.".to_string(),
        Tab::Playlists => "No saved playlists yet.".to_string(),
        Tab::History => "No watch history yet.".to_string(),
    };
    let block = Block::bordered().title(app.tab.title());
    f.render_widget(Paragraph::new(text).dim().block(block).wrap(Wrap { trim: true }), area);
}

fn draw_status(app: &App, f: &mut Frame, area: Rect) {
    let hint = |a: Action, label: &str| {
        app.keymap.first_key(a).map(|k| vec![Span::from(k.to_string()).bold(), Span::from(format!(" {label}  "))])
    };
    let mut wanted = vec![(Action::Help, "help"), (Action::Search, "search")];
    if app.tab == Tab::Search {
        wanted.push((Action::Filters, "filters"));
    }
    wanted.push((Action::Quit, "quit"));
    let hints: Vec<Span> = wanted.into_iter().filter_map(|(a, l)| hint(a, l)).flatten().collect();
    let hints = Line::from(hints).right_aligned();
    let hints_width = u16::try_from(hints.width()).unwrap_or(u16::MAX);
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(hints_width)]).spacing(2).areas(area);
    let message = app
        .status
        .clone()
        .map(|m| (m, false))
        .or_else(|| (app.tab == Tab::Search).then(|| search::status(&app.search)).flatten())
        .or_else(|| app.graphics.message().map(|m| (m, false)));
    match message {
        Some((m, true)) => f.render_widget(Paragraph::new(m).red(), left),
        Some((m, false)) => f.render_widget(Paragraph::new(m).yellow(), left),
        None => {}
    }
    f.render_widget(hints, right);
}

fn draw_help(app: &App, f: &mut Frame, area: Rect) {
    let rows: Vec<Row> = app
        .keymap
        .bindings()
        .iter()
        .map(|(action, keys)| {
            let keys = keys.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            Row::new([action.description().to_string(), keys])
        })
        .collect();
    let height = u16::try_from(rows.len()).unwrap_or(u16::MAX).saturating_add(2);
    let popup = centered(area, 48, height);
    let table = Table::new(rows, [Constraint::Length(20), Constraint::Min(0)]).block(Block::bordered().title("Keys"));
    f.render_widget(Clear, popup);
    f.render_widget(table, popup);
}

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Input, Message};
    use crate::keymap::{KeyCode, KeyMap, KeyPress};
    use crate::source::model::{Channel, ChannelRef, Item, Playlist, PlaylistKind, SearchPage, Video, VideoKind};
    use crate::terminal::detect::Graphics;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| view(app, f)).unwrap();
        let buf = t.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
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
        press(&mut app, "tab");
        insta::assert_snapshot!(render(&app, 100, 8));
    }

    #[test]
    fn help_overlay() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "?");
        insta::assert_snapshot!(render(&app, 60, 20));
    }

    fn sample_results() -> Vec<Item> {
        let chan = |name: &str| ChannelRef { id: Some(format!("UC{name}")), name: name.into(), handle: None };
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
                thumbnails: Vec::new(),
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
                thumbnails: Vec::new(),
            }),
            Item::Playlist(Playlist {
                id: "PLx".into(),
                title: "Crust of Rust".into(),
                kind: PlaylistKind::Playlist,
                channel: Some(chan("Jon Gjengset")),
                count_text: Some("44 videos".into()),
                thumbnails: Vec::new(),
                first_video_id: None,
            }),
        ]
    }

    fn app_with_results(height: u16) -> App {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
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
        app.update(Message::SearchResult { generation, append: false, result: Ok(page) });
        app
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

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut app = app_with_results(3);
        press(&mut app, "f");
        render(&app, 4, 3);
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "?");
        render(&app, 5, 2);
    }
}
