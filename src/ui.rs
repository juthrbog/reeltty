//! Draws the `App` state. Pure: the same state always gives the same frame.

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
    let key = |a| app.keymap.first_key(a).map(|k| k.to_string()).unwrap_or_default();
    let text = match app.tab {
        Tab::Search => format!("Press {} to search.", key(Action::Search)),
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
    let hints: Vec<Span> = [(Action::Help, "help"), (Action::Search, "search"), (Action::Quit, "quit")]
        .into_iter()
        .filter_map(|(a, l)| hint(a, l))
        .flatten()
        .collect();
    let hints = Line::from(hints).right_aligned();
    let hints_width = u16::try_from(hints.width()).unwrap_or(u16::MAX);
    let [left, right] = Layout::horizontal([Constraint::Min(0), Constraint::Length(hints_width)]).areas(area);
    if let Some(msg) = app.graphics.message() {
        f.render_widget(Paragraph::new(msg).yellow(), left);
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

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Input, Message};
    use crate::keymap::KeyMap;
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

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut app = App::new(KeyMap::default(), Graphics::KittyShm);
        press(&mut app, "?");
        render(&app, 5, 2);
    }
}
