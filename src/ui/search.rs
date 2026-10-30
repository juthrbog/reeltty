//! The Search tab.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, Wrap};

use super::items::{tail, thousands};
use super::{centered, items};
use crate::app::App;
use crate::app::search::{FilterRow, Loading, SearchState};
use crate::app::thumbnails::Thumbnails;
use crate::keymap::Action;
use crate::source::filters::SearchFilters;
use crate::source::model::Spelling;

pub fn draw(app: &App, f: &mut Frame, area: Rect) {
    let s = &app.search;
    let title = match (&s.query, s.estimated_results) {
        (Some(q), Some(n)) => format!("Search: {} ({} results)", q.text, thousands(n)),
        (Some(q), None) => format!("Search: {}", q.text),
        (None, _) => "Search".to_string(),
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [query, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    f.render_widget(query_line(app, usize::from(query.width)), query);
    draw_results(s, &app.thumbnails, f, list);
    if s.filter_menu.is_some() {
        draw_filter_menu(s, f, area);
    }
}

fn query_line(app: &App, width: usize) -> Paragraph<'static> {
    let s = &app.search;
    let key = |a| app.keymap.first_key(a).map(|k| k.to_string()).unwrap_or_default();
    if let Some(input) = &s.input {
        // Show the end of a long query, so the cursor stays on screen.
        let shown = tail(input, width.saturating_sub(3));
        return Paragraph::new(Line::from(vec![Span::from("/ ").bold(), Span::from(shown), "█".into()]));
    }
    // Only the filters that differ from the defaults.
    let defaults = SearchFilters::default();
    let filters: Vec<&str> = FilterRow::ALL
        .iter()
        .filter(|row| row.value(&s.filters) != row.value(&defaults))
        .map(|row| row.value(&s.filters))
        .collect();
    let mut spans = match &s.query {
        Some(q) => vec![Span::from(q.text.clone()).bold()],
        None => vec![
            Span::from(format!("Press {} to search, {} for filters.", key(Action::Search), key(Action::Filters))).dim(),
        ],
    };
    if !filters.is_empty() {
        spans.push(Span::from(format!("  [{}]", filters.join(", "))).cyan());
    }
    Paragraph::new(Line::from(spans))
}

fn draw_results(s: &SearchState, thumbnails: &Thumbnails, f: &mut Frame, area: Rect) {
    if s.results.is_empty()
        && let Some(e) = &s.error
    {
        let text = Paragraph::new(format!("Search failed: {e}")).red().wrap(Wrap { trim: true });
        f.render_widget(text, area);
        return;
    }
    if s.results.is_empty() {
        let text = match (s.loading, &s.query, &s.error) {
            (Loading::FirstPage, _, _) => "Searching...",
            (_, Some(_), None) => "No results.",
            _ => "",
        };
        f.render_widget(Paragraph::new(text).dim(), area);
        return;
    }
    items::draw_list(&s.results, s.selection, thumbnails, f, area);
}

fn draw_filter_menu(s: &SearchState, f: &mut Frame, area: Rect) {
    let Some(menu) = s.filter_menu else { return };
    let rows: Vec<Row> = FilterRow::ALL
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let r = Row::new([row.title().to_string(), row.value(&s.filters).to_string()]);
            if i == menu.row { r.style(Style::new().add_modifier(Modifier::REVERSED)) } else { r }
        })
        .collect();
    let height = u16::try_from(FilterRow::ALL.len()).unwrap_or(u16::MAX) + 2;
    let popup = centered(area, 40, height);
    let block = Block::bordered().title("Filters").title_bottom(Line::from(" enter: change  esc: done ").dim());
    let table = Table::new(rows, [Constraint::Length(14), Constraint::Min(0)]).block(block);
    f.render_widget(Clear, popup);
    f.render_widget(table, popup);
}

/// The status bar message for the Search tab, if any, and whether it is an error.
pub fn status(s: &SearchState) -> Option<(String, bool)> {
    if let Some(e) = &s.error {
        return Some((format!("Search failed: {e}"), true));
    }
    match s.loading {
        Loading::FirstPage => return Some(("Searching...".into(), false)),
        Loading::MorePages => return Some(("Loading more...".into(), false)),
        Loading::Idle => {}
    }
    if let Some(n) = &s.note {
        return Some((n.clone(), false));
    }
    match &s.spelling {
        Some(Spelling::ShowingResultsFor { corrected, original }) => {
            Some((format!("Showing results for \"{corrected}\" (you searched for \"{original}\")"), false))
        }
        Some(Spelling::DidYouMean { corrected }) => Some((format!("Did you mean \"{corrected}\"?"), false)),
        None => None,
    }
}
