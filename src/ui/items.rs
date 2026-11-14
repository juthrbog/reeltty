//! Drawing items in lists: two rows for each video, channel, or playlist.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::app::player::clock;
use crate::app::{App, Selection};
use crate::source::model::{Channel, ChannelRef, Item, Playlist, PlaylistKind, Video, VideoKind};
use crate::store::HistoryEntry;
use crate::terminal::kitty;
use crate::theme::Theme;

/// The text column must keep at least this many cells, or thumbnails are not drawn.
const MIN_TEXT_WIDTH: u16 = 20;

/// The columns before each row: the selection bar and a space.
pub const MARK_WIDTH: usize = 2;

/// Draws `items` with the selected one marked, and with thumbnails if they
/// are on. Videos in the history show as watched, and the video that plays
/// has a `▶`.
pub fn draw_list(app: &App, items: &[Item], selection: Selection, f: &mut Frame, area: Rect) {
    let (theme, thumbnails) = (&app.theme, &app.thumbnails);
    let playing = app.player.as_ref().map(|p| p.id.as_str());
    let slot = thumbnails.slot();
    let three = thumbnails.enabled;
    let show = three && area.width >= slot.cols + 1 + MIN_TEXT_WIDTH && area.height >= slot.rows;
    let (pictures, text) = if show {
        let [pictures, _, text] =
            Layout::horizontal([Constraint::Length(slot.cols), Constraint::Length(1), Constraint::Min(0)]).areas(area);
        (Some(pictures), text)
    } else {
        (None, area)
    };
    let watched: HashMap<&str, &HistoryEntry> = app.history.iter().map(|e| (e.video_id.as_str(), e)).collect();
    let width = usize::from(text.width).saturating_sub(MARK_WIDTH);
    let rows_of = |(i, item): (usize, &Item)| {
        let (entry, plays) = match item {
            Item::Video(v) => (watched.get(v.id.as_str()).copied(), playing == Some(v.id.as_str())),
            Item::Channel(_) | Item::Playlist(_) => (None, false),
        };
        let state = RowState { selected: i == selection.index, playing: plays };
        rows(theme, item, entry, width, three, state)
    };
    let offset = draw_rows(theme, items.iter().enumerate().map(rows_of).collect(), selection, f, text);
    let Some(pictures) = pictures else { return };
    for (n, item) in items.iter().skip(offset).enumerate() {
        // The List draws only whole items, so a picture is drawn only where it fits whole.
        let Some(y) = u16::try_from(n)
            .ok()
            .and_then(|n| n.checked_mul(slot.rows))
            .filter(|y| y.saturating_add(slot.rows) <= pictures.height)
        else {
            break;
        };
        if let Some(id) = thumbnails.image(item) {
            draw_image(f.buffer_mut(), Rect { y: pictures.y + y, height: slot.rows, ..pictures }, id);
        }
    }
}

/// Draws prepared rows (all with the same number of lines). The selected
/// row gets a bar before each of its lines, and the others a space, so the
/// rows must be `MARK_WIDTH` narrower than `area`. Returns the first visible
/// row.
pub fn draw_rows(
    theme: &Theme,
    rows: Vec<Vec<Line<'static>>>,
    selection: Selection,
    f: &mut Frame,
    area: Rect,
) -> usize {
    // A list area too small for a whole item shows titles only, so that the selection is still visible.
    let lines = rows.first().map_or(1, Vec::len);
    let whole = usize::from(area.height) >= lines;
    let items: Vec<ListItem> = rows
        .into_iter()
        .enumerate()
        .map(|(i, mut lines)| {
            if !whole {
                lines.truncate(1);
            }
            let mark = if i == selection.index { Span::styled("▌ ", theme.list_bar) } else { Span::from("  ") };
            for line in &mut lines {
                line.spans.insert(0, mark.clone());
            }
            ListItem::new(lines)
        })
        .collect();
    let list = List::new(items);
    let mut state = ListState::default().with_offset(selection.offset).with_selected(Some(selection.index));
    f.render_stateful_widget(list, area, &mut state);
    state.offset()
}

/// Fills `area` with the placeholder cells of image `id`.
pub fn draw_image(buf: &mut Buffer, area: Rect, id: u32) {
    let (r, g, b) = kitty::id_color(id);
    let style = Style::reset().fg(Color::Rgb(r, g, b));
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(symbol) = kitty::cell(row, col) else { continue };
            if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                cell.set_symbol(&symbol).set_style(style);
            }
        }
    }
}

/// How a row shows.
#[derive(Debug, Clone, Copy, Default)]
pub struct RowState {
    pub selected: bool,
    /// The video plays now.
    pub playing: bool,
}

/// The style of a row title.
pub fn title_style(theme: &Theme, selected: bool) -> Style {
    if selected { theme.title.patch(theme.list_title) } else { theme.title }
}

/// A title line `width` columns wide: `lead` (badges and marks), the title,
/// cut to fit, and `right` at the right end. `right` is left out when the
/// line is too narrow for it.
pub fn title_line(
    theme: &Theme,
    mut lead: Vec<Span<'static>>,
    title: &str,
    right: Option<&str>,
    width: usize,
    selected: bool,
) -> Line<'static> {
    let lead_width: usize = lead.iter().map(Span::width).sum();
    let room = width.saturating_sub(lead_width);
    // The title keeps at least this many columns before `right` is left out.
    let right = right.filter(|r| room >= r.width() + 1 + 8);
    let title_room = room.saturating_sub(right.map_or(0, |r| r.width() + 1));
    let title = truncate(title, title_room);
    let gap = title_room.saturating_sub(title.width());
    lead.push(Span::styled(title, title_style(theme, selected)));
    if let Some(right) = right {
        lead.push(Span::from(" ".repeat(gap + 1)));
        lead.push(Span::styled(right.to_string(), theme.muted));
    }
    Line::from(lead)
}

/// A detail line `width` columns wide: `who` (the channel) in plain text,
/// then the `rest` dim, with " · " between them.
pub fn detail_line(theme: &Theme, who: Option<&str>, rest: &[String], width: usize) -> Line<'static> {
    let parts: Vec<&str> = who.into_iter().chain(rest.iter().map(String::as_str)).collect();
    let text = truncate(&parts.join(" · "), width);
    match who {
        Some(who) if text.len() > who.len() && text.starts_with(who) => {
            let rest = text[who.len()..].to_string();
            Line::from(vec![Span::from(who.to_string()), Span::styled(rest, theme.muted)])
        }
        Some(_) => Line::from(text),
        // On the span, not the line: a line style would also dim the selection bar.
        None => Line::from(Span::styled(text, theme.muted)),
    }
}

/// The lines for one result, `width` columns wide: the title, then the
/// details. `watched` is the history entry of a video. `three` puts the
/// details on two lines, beside a thumbnail.
pub fn rows(
    theme: &Theme,
    item: &Item,
    watched: Option<&HistoryEntry>,
    width: usize,
    three: bool,
    state: RowState,
) -> Vec<Line<'static>> {
    let parts = match item {
        Item::Video(v) => video_parts(theme, v, watched, state.playing),
        Item::Channel(c) => channel_parts(theme, c),
        Item::Playlist(p) => playlist_parts(theme, p),
    };
    let title = title_line(theme, parts.lead, &parts.title, parts.right.as_deref(), width, state.selected);
    let who = parts.who.as_deref();
    if !three {
        let details = [parts.rest, parts.more].concat();
        return vec![title, detail_line(theme, who, &details, width)];
    }
    // Beside a thumbnail: the first two groups of details, one on each line.
    let mut details: Vec<Line<'static>> = Vec::new();
    if who.is_some() {
        details.push(detail_line(theme, who, &[], width));
    }
    for group in [parts.rest, parts.more] {
        if !group.is_empty() {
            details.push(detail_line(theme, None, &group, width));
        }
    }
    details.resize(2, Line::default());
    let mut lines = vec![title];
    lines.extend(details.into_iter().take(2));
    lines
}

/// The parts of a row.
struct Parts {
    /// Badges and marks before the title.
    lead: Vec<Span<'static>>,
    title: String,
    /// At the right end of the title line, such as the duration.
    right: Option<String>,
    /// Who made it, in plain text.
    who: Option<String>,
    /// The other details, dim.
    rest: Vec<String>,
    /// More details, on their own line beside a thumbnail.
    more: Vec<String>,
}

/// A badge before a title, such as LIVE, then one space.
fn badge(text: &'static str, style: Style) -> [Span<'static>; 2] {
    [Span::styled(text, style), Span::from(" ")]
}

fn video_parts(theme: &Theme, v: &Video, watched: Option<&HistoryEntry>, playing: bool) -> Parts {
    let mut lead = Vec::new();
    if playing {
        lead.extend(badge("▶", theme.now_playing));
    }
    match v.kind {
        VideoKind::Live => lead.extend(badge("LIVE", theme.badge_live)),
        VideoKind::Short => lead.extend(badge("SHORT", theme.badge_short)),
        VideoKind::Normal | VideoKind::PastStream => {}
    }
    if v.members_only {
        lead.extend(badge("MEMBERS", theme.badge_members));
    }
    let who = (!v.channels.is_empty()).then(|| channel_names(&v.channels));
    let mut rest = Vec::new();
    // First, so that a narrow list does not cut it off.
    rest.extend(watched.map(watched_text));
    rest.extend(v.views_text.clone());
    rest.extend(v.published_text.clone());
    rest.extend(v.badges.iter().cloned());
    Parts { lead, title: v.title.clone(), right: v.duration_text.clone(), who, rest, more: Vec::new() }
}

/// How much of a video the user watched, for lists.
fn watched_text(e: &HistoryEntry) -> String {
    match (e.finished, e.position_secs) {
        (true, _) => "watched".to_string(),
        (false, 0) => "played".to_string(),
        (false, secs) => format!("stopped at {}", clock(secs)),
    }
}

fn channel_parts(theme: &Theme, c: &Channel) -> Parts {
    let lead = badge("CHANNEL", theme.badge_channel).to_vec();
    let title = if c.verified { format!("{} ✓", c.name) } else { c.name.clone() };
    let rest = c.handle.iter().chain(&c.subscribers_text).cloned().collect();
    let more = c.description_snippet.iter().cloned().collect();
    Parts { lead, title, right: None, who: None, rest, more }
}

fn playlist_parts(theme: &Theme, p: &Playlist) -> Parts {
    let label = match p.kind {
        PlaylistKind::Playlist => "PLAYLIST",
        PlaylistKind::Course => "COURSE",
    };
    let lead = badge(label, theme.badge_playlist).to_vec();
    let who = p.channel.as_ref().map(|c| c.name.clone());
    Parts { lead, title: p.title.clone(), right: p.count_text.clone(), who, rest: Vec::new(), more: Vec::new() }
}

pub fn channel_names(channels: &[ChannelRef]) -> String {
    match channels {
        [one] => one.name.clone(),
        [first, second] => format!("{} and {}", first.name, second.name),
        [first, rest @ ..] => format!("{} and {} more", first.name, rest.len()),
        [] => String::new(),
    }
}

/// Cuts `s` to `width` columns and ends it with `…` if it was cut.
pub fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let budget = width.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// The end of `s` that fits in `width` columns.
pub fn tail(s: &str, width: usize) -> String {
    let mut used = 0;
    let mut start = s.len();
    for (i, c) in s.char_indices().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        start = i;
    }
    s[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn titles_put_the_duration_at_the_end() {
        let t = Theme::ansi();
        let line = title_line(&t, Vec::new(), "Rust in 100 seconds", Some("2:24"), 30, false);
        assert_eq!(text(&line), "Rust in 100 seconds       2:24");
        assert_eq!(line.width(), 30);
        // A long title is cut before the duration.
        let line = title_line(&t, Vec::new(), "A very long title that does not fit", Some("2:24"), 30, true);
        assert_eq!(text(&line), "A very long title that d… 2:24");
        assert_eq!(line.width(), 30);
        assert_eq!(line.spans[0].style, t.title.patch(t.list_title));
        // Too narrow for the duration: the title gets the room.
        let line = title_line(&t, badge("LIVE", t.badge_live).to_vec(), "Lofi beats", Some("1:02:03"), 18, false);
        assert_eq!(text(&line), "LIVE Lofi beats");
    }

    #[test]
    fn details_show_who_in_plain_text() {
        let t = Theme::ansi();
        let rest = ["1.2M views".to_string(), "3 years ago".to_string()];
        let line = detail_line(&t, Some("Fireship"), &rest, 40);
        assert_eq!(text(&line), "Fireship · 1.2M views · 3 years ago");
        assert_eq!((line.spans[0].style, line.spans[1].style), (Style::new(), t.muted));
        // Cut inside the name: all plain.
        let line = detail_line(&t, Some("Fireship"), &rest, 6);
        assert_eq!((text(&line), line.spans.len()), ("Fires…".to_string(), 1));
        let line = detail_line(&t, None, &rest, 40);
        assert_eq!((line.style, line.spans[0].style), (Style::new(), t.muted));
    }

    #[test]
    fn formats_numbers_and_truncates() {
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
        assert_eq!(truncate("short", 6), "short");
        // Wide characters take two columns.
        assert_eq!(truncate("日本語チャンネル", 6), "日本…");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("日本語", 5), "本語");
        assert_eq!(tail("ab", 0), "");
    }
}
