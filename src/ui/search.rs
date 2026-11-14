//! The Search tab.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Row, Table, Wrap};

use super::items::tail;
use super::{bordered, centered, error_line, items};
use crate::app::search::{FilterRow, Loading, SearchState};
use crate::app::thumbnails::Thumbnails;
use crate::app::{App, StatusMessage};
use crate::keymap::Action;
use crate::source::filters::SearchFilters;
use crate::source::model::Spelling;
use crate::source::ytdlp::compact;
use crate::store::HistoryEntry;
use crate::theme::Theme;

pub fn draw(app: &App, f: &mut Frame, area: Rect) {
    let s = &app.search;
    // The query line shows the query, so the title does not.
    let title = match (&s.query, s.estimated_results) {
        (Some(_), Some(n)) => format!(" Search · {} results ", compact(n)),
        _ => " Search ".to_string(),
    };
    let block = bordered(&app.theme, title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [query, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    f.render_widget(query_line(app, usize::from(query.width)), query);
    draw_results(&app.theme, s, &app.thumbnails, &app.history, f, list);
    if s.filter_menu.is_some() {
        draw_filter_menu(app, f, area);
    }
}

fn query_line(app: &App, width: usize) -> Paragraph<'static> {
    let s = &app.search;
    let t = &app.theme;
    let key = |a| app.keymap.first_key(a).map(|k| k.to_string()).unwrap_or_default();
    if let Some(input) = &s.input {
        // Show the end of a long query, so the cursor stays on screen.
        let shown = tail(input, width.saturating_sub(3));
        return Paragraph::new(Line::from(vec![Span::styled("/ ", t.key), Span::from(shown), "█".into()]));
    }
    // Only the filters that differ from the defaults.
    let defaults = SearchFilters::default();
    let filters: Vec<&str> = FilterRow::ALL
        .iter()
        .filter(|row| row.value(&s.filters) != row.value(&defaults))
        .map(|row| row.value(&s.filters))
        .collect();
    let mut spans = match &s.query {
        Some(q) => vec![Span::styled(q.text.clone(), t.title)],
        None => vec![Span::styled(
            format!("Press {} to search, {} for filters.", key(Action::Search), key(Action::Filters)),
            t.muted,
        )],
    };
    if !filters.is_empty() {
        spans.push(Span::styled(format!("  [{}]", filters.join(", ")), t.accent));
    }
    Paragraph::new(Line::from(spans))
}

fn draw_results(
    t: &Theme,
    s: &SearchState,
    thumbnails: &Thumbnails,
    history: &[HistoryEntry],
    f: &mut Frame,
    area: Rect,
) {
    if s.results.is_empty()
        && let Some(e) = &s.error
    {
        let text = Paragraph::new(error_line(t, format!("Search failed: {e}"))).wrap(Wrap { trim: true });
        f.render_widget(text, area);
        return;
    }
    if s.results.is_empty() {
        let text = match (s.loading, &s.query, &s.error) {
            (Loading::FirstPage, _, _) => Line::styled("Searching…", t.status_progress),
            (_, Some(_), None) => Line::styled("No results.", t.muted),
            _ => Line::default(),
        };
        f.render_widget(Paragraph::new(text), area);
        return;
    }
    items::draw_list(t, &s.results, s.selection, thumbnails, history, f, area);
}

fn draw_filter_menu(app: &App, f: &mut Frame, area: Rect) {
    let s = &app.search;
    let Some(menu) = s.filter_menu else { return };
    let rows: Vec<Row> = FilterRow::ALL
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let r = Row::new([row.title().to_string(), row.value(&s.filters).to_string()]);
            if i == menu.row { r.style(app.theme.selection) } else { r }
        })
        .collect();
    let height = u16::try_from(FilterRow::ALL.len()).unwrap_or(u16::MAX) + 2;
    // 38 columns inside the border and the padding.
    let popup = centered(area, 42, height);
    let mut block = bordered(&app.theme, " Filters ");
    let keys = [(app.keymap.first_key(Action::Open), "change"), (app.keymap.first_key(Action::Back), "done")];
    if let Some(hints) = super::hint_title(&app.theme, keys) {
        block = block.title_bottom(hints);
    }
    let table = Table::new(rows, [Constraint::Length(14), Constraint::Min(0)]).block(block);
    f.render_widget(Clear, popup);
    f.render_widget(table, popup);
}

/// The status bar message for the Search tab, if any.
pub fn status(s: &SearchState) -> Option<StatusMessage> {
    // Without results, the body shows the error, so it shows in one place.
    if let Some(e) = s.error.as_ref().filter(|_| !s.results.is_empty()) {
        return Some(StatusMessage::error(format!("Search failed: {e}")));
    }
    match s.loading {
        Loading::FirstPage => return Some(StatusMessage::progress("Searching…")),
        Loading::MorePages => return Some(StatusMessage::progress("Loading more…")),
        Loading::Idle => {}
    }
    if let Some(n) = &s.note {
        return Some(StatusMessage::info(n.clone()));
    }
    match &s.spelling {
        Some(Spelling::ShowingResultsFor { corrected, original }) => {
            Some(StatusMessage::info(format!("Showing results for \"{corrected}\" (you searched for \"{original}\")")))
        }
        Some(Spelling::DidYouMean { corrected }) => Some(StatusMessage::info(format!("Did you mean \"{corrected}\"?"))),
        None => None,
    }
}
