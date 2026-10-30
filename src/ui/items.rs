//! Drawing items in lists: two rows for each video, channel, or playlist.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::app::{ROWS_PER_ITEM, Selection};
use crate::source::model::{Channel, ChannelRef, Item, Playlist, PlaylistKind, Video, VideoKind};

/// Draws `items` with the selected one highlighted.
pub fn draw_list(items: &[Item], selection: Selection, f: &mut Frame, area: Rect) {
    let rows_of = |item: &Item| rows(item, usize::from(area.width));
    draw_rows(items.iter().map(rows_of).collect(), selection, f, area);
}

/// Draws prepared two-line rows with the selected one highlighted.
pub fn draw_rows(rows: Vec<Vec<Line<'static>>>, selection: Selection, f: &mut Frame, area: Rect) {
    // A list area of one row shows titles only, so that the selection is still visible.
    let two_rows = area.height >= ROWS_PER_ITEM;
    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|mut lines| {
            if !two_rows {
                lines.truncate(1);
            }
            ListItem::new(lines)
        })
        .collect();
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_offset(selection.offset).with_selected(Some(selection.index));
    f.render_stateful_widget(list, area, &mut state);
}

/// Two lines for one result: the title, then the details.
pub fn rows(item: &Item, width: usize) -> Vec<Line<'static>> {
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
