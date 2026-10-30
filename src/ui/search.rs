//! The Search tab.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Row, Table};

use super::centered;
use crate::app::App;
use crate::app::search::{FilterRow, Loading, SearchState};
use crate::keymap::Action;
use crate::source::filters::SearchFilters;
use crate::source::model::{Channel, ChannelRef, Item, Playlist, PlaylistKind, Spelling, Video, VideoKind};

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

    f.render_widget(query_line(app), query);
    draw_results(s, f, list);
    if s.filter_menu.is_some() {
        draw_filter_menu(s, f, area);
    }
}

fn query_line(app: &App) -> Paragraph<'static> {
    let s = &app.search;
    let key = |a| app.keymap.first_key(a).map(|k| k.to_string()).unwrap_or_default();
    if let Some(input) = &s.input {
        return Paragraph::new(Line::from(vec![Span::from("/ ").bold(), Span::from(input.clone()), "█".into()]));
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

fn draw_results(s: &SearchState, f: &mut Frame, area: Rect) {
    if s.results.is_empty() {
        let text = match (s.loading, &s.query, &s.error) {
            (Loading::FirstPage, _, _) => "Searching...",
            (_, Some(_), None) => "No results.",
            _ => "",
        };
        f.render_widget(Paragraph::new(text).dim(), area);
        return;
    }
    let width = usize::from(area.width);
    let items: Vec<ListItem> = s.results.iter().map(|item| ListItem::new(rows(item, width))).collect();
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_offset(s.selection.offset).with_selected(Some(s.selection.index));
    f.render_stateful_widget(list, area, &mut state);
}

/// Two lines for one result: the title, then the details.
fn rows(item: &Item, width: usize) -> Vec<Line<'static>> {
    let (title, details) = match item {
        Item::Video(v) => video_rows(v),
        Item::Channel(c) => channel_rows(c),
        Item::Playlist(p) => playlist_rows(p),
    };
    let details = Line::from(vec![Span::from("  "), Span::from(truncate(&details, width.saturating_sub(2))).dim()]);
    vec![title, details]
}

fn video_rows(v: &Video) -> (Line<'static>, String) {
    let mut title = Vec::new();
    match v.kind {
        VideoKind::Live => title.push(Span::from("LIVE ").red().bold()),
        VideoKind::Short => title.push(Span::from("SHORT ").magenta().bold()),
        VideoKind::Normal | VideoKind::PastStream => {}
    }
    if v.members_only {
        title.push(Span::from("MEMBERS ").yellow().bold());
    }
    title.push(Span::from(v.title.clone()).bold());
    let mut parts = Vec::new();
    if !v.channels.is_empty() {
        parts.push(channel_names(&v.channels));
    }
    parts.extend(v.duration_text.clone());
    parts.extend(v.views_text.clone());
    parts.extend(v.published_text.clone());
    parts.extend(v.badges.iter().cloned());
    (Line::from(title), parts.join(" · "))
}

fn channel_rows(c: &Channel) -> (Line<'static>, String) {
    let mut title = vec![Span::from("CHANNEL ").cyan().bold(), Span::from(c.name.clone()).bold()];
    if c.verified {
        title.push(" ✓".into());
    }
    let parts: Vec<String> =
        c.handle.iter().chain(&c.subscribers_text).chain(&c.description_snippet).cloned().collect();
    (Line::from(title), parts.join(" · "))
}

fn playlist_rows(p: &Playlist) -> (Line<'static>, String) {
    let label = match p.kind {
        PlaylistKind::Playlist => "PLAYLIST ",
        PlaylistKind::Course => "COURSE ",
    };
    let title = vec![Span::from(label).green().bold(), Span::from(p.title.clone()).bold()];
    let parts: Vec<String> = p.channel.iter().map(|c| c.name.clone()).chain(p.count_text.clone()).collect();
    (Line::from(title), parts.join(" · "))
}

fn channel_names(channels: &[ChannelRef]) -> String {
    match channels {
        [one] => one.name.clone(),
        [first, second] => format!("{} and {}", first.name, second.name),
        [first, rest @ ..] => format!("{} and {} more", first.name, rest.len()),
        [] => String::new(),
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_and_truncates() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(6_192_373), "6,192,373");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
        assert_eq!(truncate("short", 6), "short");
    }
}
