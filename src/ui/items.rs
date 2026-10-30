//! Drawing items in lists: two rows for each video, channel, or playlist.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::app::Selection;
use crate::app::thumbnails::Thumbnails;
use crate::source::model::{Channel, ChannelRef, Item, Playlist, PlaylistKind, Video, VideoKind};
use crate::terminal::kitty;

/// The text column must keep at least this many cells, or thumbnails are not drawn.
const MIN_TEXT_WIDTH: u16 = 20;

/// Draws `items` with the selected one highlighted, and with thumbnails if they are on.
pub fn draw_list(items: &[Item], selection: Selection, thumbnails: &Thumbnails, f: &mut Frame, area: Rect) {
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
    let rows_of = |item: &Item| rows(item, usize::from(text.width), three);
    let offset = draw_rows(items.iter().map(rows_of).collect(), selection, f, text);
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

/// Draws prepared rows (all with the same number of lines) with the selected
/// one highlighted. Returns the first visible row.
pub fn draw_rows(rows: Vec<Vec<Line<'static>>>, selection: Selection, f: &mut Frame, area: Rect) -> usize {
    // A list area too small for a whole item shows titles only, so that the selection is still visible.
    let lines = rows.first().map_or(1, Vec::len);
    let whole = usize::from(area.height) >= lines;
    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|mut lines| {
            if !whole {
                lines.truncate(1);
            }
            ListItem::new(lines)
        })
        .collect();
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
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

/// The lines for one result: the title, then the details. `three` puts the
/// details on two lines, beside a thumbnail.
pub fn rows(item: &Item, width: usize, three: bool) -> Vec<Line<'static>> {
    let (title, mut first, mut second) = match item {
        Item::Video(v) => video_rows(v),
        Item::Channel(c) => channel_rows(c),
        Item::Playlist(p) => playlist_rows(p),
    };
    let detail = |parts: &[String]| {
        Line::from(vec![Span::from("  "), Span::from(truncate(&parts.join(" · "), width.saturating_sub(2))).dim()])
    };
    if !three {
        first.append(&mut second);
        return vec![title, detail(&first)];
    }
    if first.is_empty() {
        std::mem::swap(&mut first, &mut second);
    }
    vec![title, detail(&first), detail(&second)]
}

/// The title line and two groups of details: who made it, then the rest.
type Rows = (Line<'static>, Vec<String>, Vec<String>);

fn video_rows(v: &Video) -> Rows {
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
    let by = if v.channels.is_empty() { Vec::new() } else { vec![channel_names(&v.channels)] };
    let mut rest = Vec::new();
    rest.extend(v.duration_text.clone());
    rest.extend(v.views_text.clone());
    rest.extend(v.published_text.clone());
    rest.extend(v.badges.iter().cloned());
    (Line::from(title), by, rest)
}

fn channel_rows(c: &Channel) -> Rows {
    let mut title = vec![Span::from("CHANNEL ").cyan().bold(), Span::from(c.name.clone()).bold()];
    if c.verified {
        title.push(" ✓".into());
    }
    let by: Vec<String> = c.handle.iter().chain(&c.subscribers_text).cloned().collect();
    (Line::from(title), by, c.description_snippet.iter().cloned().collect())
}

fn playlist_rows(p: &Playlist) -> Rows {
    let label = match p.kind {
        PlaylistKind::Playlist => "PLAYLIST ",
        PlaylistKind::Course => "COURSE ",
    };
    let title = vec![Span::from(label).green().bold(), Span::from(p.title.clone()).bold()];
    let by = p.channel.iter().map(|c| c.name.clone()).collect();
    (Line::from(title), by, p.count_text.iter().cloned().collect())
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

pub fn thousands(n: u64) -> String {
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
        // Wide characters take two columns.
        assert_eq!(truncate("日本語チャンネル", 6), "日本…");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("日本語", 5), "本語");
        assert_eq!(tail("ab", 0), "");
    }
}
