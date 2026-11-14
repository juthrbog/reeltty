//! Screens on top of the tabs, and the Subscriptions and Playlists tabs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};

use super::items::{self, truncate};
use super::{bordered, error_line};
use crate::app::comments::{CommentRow, CommentsState};
use crate::app::list::{ItemList, Loading};
use crate::app::player::clock;
use crate::app::screens::{ChannelScreen, PlaylistScreen, Screen, VideoPane, VideoScreen};
use crate::app::{App, SCREEN_HEADER_ROWS, StatusMessage};
use crate::keymap::Action;
use crate::source::model::{Comment, Description, Item, LinkTarget, VideoDetails};
use crate::theme::Theme;

/// A comment that is not selected shows at most this many lines of text.
const COMMENT_LINES: u16 = 6;
/// Replies start this many columns to the right of comments.
const REPLY_INDENT: u16 = 4;

pub fn draw(app: &App, screen: &Screen, f: &mut Frame, area: Rect) {
    match screen {
        Screen::Channel(s) => draw_channel(app, s, f, area),
        Screen::Playlist(s) => draw_playlist(app, s, f, area),
        Screen::Video(s) => draw_video(app, s, f, area),
    }
}

/// The status bar message for a screen. An error that the body shows is
/// left out, so each error shows in one place.
pub fn status(screen: &Screen) -> Option<StatusMessage> {
    let list = match screen {
        Screen::Channel(s) => &s.list,
        Screen::Playlist(s) => &s.list,
        Screen::Video(s) if s.pane == VideoPane::Comments => {
            return s.comments.as_deref().and_then(|c| comments_status(c, &s.id));
        }
        Screen::Video(s) => {
            let link = s.link.and_then(|i| s.details.as_ref()?.description.links.get(i));
            if let Some(link) = link {
                return Some(StatusMessage::info(link_status(&link.target, &s.id)));
            }
            return match (&s.error, s.loading, &s.details) {
                (Some(e), _, Some(_)) => Some(StatusMessage::error(format!("Could not load the video: {e}"))),
                (None, true, _) => Some(StatusMessage::progress("Loading…")),
                // Without details, the body shows the error.
                (Some(_), _, None) | (None, false, _) => None,
            };
        }
    };
    match (&list.error, list.loading) {
        (Some(e), _) if !list.items.is_empty() => Some(StatusMessage::error(format!("Could not load: {e}"))),
        (None, Loading::FirstPage) => Some(StatusMessage::progress("Loading…")),
        (None, Loading::MorePages) => Some(StatusMessage::progress("Loading more…")),
        // Without items, the body shows the error.
        (Some(_), _) | (None, Loading::Idle) => None,
    }
}

/// What `enter` does with a selected link.
fn link_status(target: &LinkTarget, video_id: &str) -> String {
    match target {
        LinkTarget::Url(url) => format!("enter: open {url}"),
        LinkTarget::Video { id, start_secs: Some(secs) } if id == video_id => {
            format!("enter: play from {}", duration(*secs))
        }
        LinkTarget::Video { id, start_secs: Some(secs) } => format!("enter: play video {id} from {}", duration(*secs)),
        LinkTarget::Video { id, start_secs: None } => format!("enter: play video {id}"),
        LinkTarget::Channel { .. } => "enter: open the channel".into(),
    }
}

fn comments_status(c: &CommentsState, video_id: &str) -> Option<StatusMessage> {
    // Without comments, the body shows the error.
    if let Some(e) = c.error.as_ref().filter(|_| !c.rows.is_empty()) {
        return Some(StatusMessage::error(format!("Could not load the comments: {e}")));
    }
    let link = c.link.and_then(|i| c.selected_comment()?.text.links.get(i));
    if let Some(link) = link {
        return Some(StatusMessage::info(link_status(&link.target, video_id)));
    }
    let text = match c.loading {
        Loading::FirstPage => "Loading comments…",
        Loading::MorePages => "Loading more comments…",
        Loading::Idle if c.loading_replies.is_some() => "Loading replies…",
        Loading::Idle => return None,
    };
    Some(StatusMessage::progress(text))
}

/// A bordered block with a header of `SCREEN_HEADER_ROWS` rows above a list.
fn header_and_list(theme: &Theme, mut title: Line<'static>, f: &mut Frame, area: Rect) -> (Rect, Rect) {
    title.spans.insert(0, " ".into());
    title.spans.push(" ".into());
    let block = bordered(theme, title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [header, list] = Layout::vertical([Constraint::Length(SCREEN_HEADER_ROWS), Constraint::Min(0)]).areas(inner);
    (header, list)
}

fn draw_channel(app: &App, s: &ChannelScreen, f: &mut Frame, area: Rect) {
    let mut title = vec![Span::from(format!("Channel: {}", s.name))];
    if s.info.as_ref().is_some_and(|i| i.verified) {
        title.push(" ✓".into());
    }
    if app.is_subscribed(&s.id) {
        title.push(Span::styled(" [Subscribed]", app.theme.saved));
    }
    let (header, list) = header_and_list(&app.theme, Line::from(title), f, area);
    let width = usize::from(header.width);
    let lines = match &s.info {
        Some(i) => {
            let facts: Vec<String> =
                i.handle.iter().chain(&i.subscribers_text).chain(&i.videos_text).cloned().collect();
            let about = i.description.as_deref().and_then(|d| d.lines().find(|l| !l.trim().is_empty())).unwrap_or("");
            vec![Line::from(truncate(&facts.join(" · "), width)), Line::styled(truncate(about, width), app.theme.muted)]
        }
        None => Vec::new(),
    };
    f.render_widget(Paragraph::new(lines), header);
    draw_items(app, &s.list, "This channel has no videos.", f, list);
}

fn draw_playlist(app: &App, s: &PlaylistScreen, f: &mut Frame, area: Rect) {
    let mut title = vec![Span::from(format!("Playlist: {}", s.title))];
    if app.is_saved(&s.id) {
        title.push(Span::styled(" [Saved]", app.theme.saved));
    }
    let (header, list) = header_and_list(&app.theme, Line::from(title), f, area);
    let width = usize::from(header.width);
    let lines = match &s.info {
        Some(i) => {
            let by = i.channel.as_ref().map(|c| format!("by {}", c.name));
            let facts: Vec<String> = by
                .into_iter()
                .chain(i.videos_text.clone())
                .chain(i.views_text.clone())
                .chain(i.updated_text.clone())
                .collect();
            let about = i.description.as_deref().and_then(|d| d.lines().find(|l| !l.trim().is_empty())).unwrap_or("");
            vec![Line::from(truncate(&facts.join(" · "), width)), Line::styled(truncate(about, width), app.theme.muted)]
        }
        None => Vec::new(),
    };
    f.render_widget(Paragraph::new(lines), header);
    draw_items(app, &s.list, "This playlist has no videos.", f, list);
}

fn draw_items(app: &App, list: &ItemList, empty: &str, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    if list.items.is_empty() {
        let text = match (&list.error, list.loading) {
            (Some(e), _) => Paragraph::new(error_line(t, format!("Could not load: {e}"))).wrap(Wrap { trim: true }),
            (None, Loading::FirstPage) => Paragraph::new(Line::styled("Loading…", t.status_progress)),
            (None, _) => Paragraph::new(Line::styled(empty.to_string(), t.muted)),
        };
        f.render_widget(text, area);
        return;
    }
    items::draw_list(t, &list.items, list.selection, &app.thumbnails, &app.history, f, area);
}

fn draw_video(app: &App, s: &VideoScreen, f: &mut Frame, area: Rect) {
    if let (VideoPane::Comments, Some(c)) = (s.pane, &s.comments) {
        return draw_comments(app, s, c, f, area);
    }
    // The details show the title in full, so the box title is short.
    let block = bordered(&app.theme, " Video ");
    let inner = block.inner(area);
    let (text, link_line) = match (&s.details, &s.error) {
        (Some(d), _) => {
            let (lines, description_start) = video_lines(app, d, s.link);
            // The wrapped line where the selected link ends: the lines before
            // its own, and its own up to the end of the link.
            let link_line = s.link.and_then(|i| d.description.links.get(i)).map(|l| {
                let head: Vec<char> = d.description.text.chars().take(l.start + l.len).collect();
                let newlines = head.iter().filter(|&&c| c == '\n').count();
                let own: String =
                    head.iter().rev().take_while(|&&c| c != '\n').collect::<Vec<_>>().into_iter().rev().collect();
                let mut before = lines[..description_start + newlines].to_vec();
                before.push(Line::from(own));
                let rows = Paragraph::new(before).wrap(Wrap { trim: false }).line_count(inner.width);
                u16::try_from(rows.saturating_sub(1)).unwrap_or(u16::MAX)
            });
            (lines, link_line)
        }
        (None, Some(e)) => (vec![error_line(&app.theme, format!("Could not load the video: {e}"))], None),
        (None, None) => (vec![Line::styled("Loading…", app.theme.status_progress)], None),
    };
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let lines = u16::try_from(paragraph.line_count(inner.width)).unwrap_or(u16::MAX);
    let max = lines.saturating_sub(inner.height);
    s.max_scroll.set(max);
    // `n` or `N` selected a link: show it, a third from the top.
    if let Some(line) = link_line.filter(|_| s.reveal.replace(false)) {
        let scroll = s.scroll.get();
        if line < scroll || line >= scroll.saturating_add(inner.height) {
            s.scroll.set(line.saturating_sub(inner.height / 3).min(max));
        }
    }
    f.render_widget(block, area);
    f.render_widget(paragraph.scroll((s.scroll.get().min(max), 0)), inner);
    if max > 0 {
        let content = usize::from(max) + usize::from(inner.height);
        draw_scrollbar(&app.theme, f, area, content, usize::from(s.scroll.get().min(max)), usize::from(inner.height));
    }
}

/// The lines of the details, with the selected link marked, and the index
/// of the first line of the description.
fn video_lines(app: &App, d: &VideoDetails, link: Option<usize>) -> (Vec<Line<'static>>, usize) {
    let t = &app.theme;
    let mut lines = vec![Line::styled(d.title.clone(), t.title)];

    let mut channel = Vec::new();
    if let Some(c) = &d.channel {
        channel.push(Span::styled(c.name.clone(), t.title));
        if let Some(subs) = &d.subscribers_text {
            channel.push(Span::from(format!(" · {subs}")));
        }
        if c.id.as_deref().is_some_and(|id| app.is_subscribed(id)) {
            channel.push(Span::styled(" [Subscribed]", t.saved));
        }
    }
    lines.push(Line::from(channel));

    let mut facts = Vec::new();
    facts.extend(d.views_text.clone());
    facts.extend(d.date_text.clone());
    facts.extend(d.likes_text.as_ref().map(|l| format!("{l} likes")));
    facts.extend(d.duration_secs.map(duration));
    let mut spans = Vec::new();
    if d.live {
        spans.push(Span::styled("LIVE", t.badge_live));
        if !facts.is_empty() {
            spans.push(Span::styled(" · ", t.muted));
        }
    }
    spans.push(Span::styled(facts.join(" · "), t.muted));
    lines.push(Line::from(spans));
    // YouTube's count already says "Comments", as in "1.2K Comments".
    let comments = match &d.comments_count_text {
        Some(c) if c.to_lowercase().contains("comment") => c.to_lowercase(),
        Some(c) => format!("comments ({c})"),
        None => "comments".into(),
    };
    let comments_key = app.keymap.first_key(Action::Comments).filter(|_| !d.live);
    let links_key = app.keymap.first_key(Action::NextLink).filter(|_| !d.description.links.is_empty());
    let hints = super::hint_spans(t, [(comments_key, comments.as_str()), (links_key, "links")]);
    if !hints.is_empty() {
        lines.push(Line::from(hints));
    }

    if !d.chapters.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled("Chapters", t.title));
        for c in &d.chapters {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>9}  ", duration(c.start_secs)), t.muted),
                c.title.clone().into(),
            ]));
        }
    }

    lines.push(Line::default());
    let description_start = lines.len();
    lines.extend(description_lines(t, &d.description, link));
    (lines, description_start)
}

/// The comments of a video, with the selected one in full and marked with a
/// bar on the left.
fn draw_comments(app: &App, screen: &VideoScreen, list: &CommentsState, f: &mut Frame, area: Rect) {
    let count = list.count_text.clone().or_else(|| screen.details.as_ref().and_then(|d| d.comments_count_text.clone()));
    // YouTube's count already says "Comments", as in "1.2K Comments".
    let mut title = vec![match count {
        Some(c) if c.to_lowercase().contains("comment") => c,
        Some(c) => format!("Comments · {c}"),
        None => "Comments".into(),
    }];
    title.extend(list.sort().map(|o| o.label.clone()));
    let t = &app.theme;
    let mut block = bordered(t, format!(" {} ", title.join(" · ")));
    let key = |a| app.keymap.first_key(a);
    let hints = super::hint_title(
        t,
        [
            (key(Action::Open), "replies"),
            (key(Action::NextLink), "link"),
            (key(Action::Sort), "order"),
            (key(Action::Comments), "details"),
        ],
    );
    if let Some(hints) = hints {
        block = block.title_bottom(hints);
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if list.rows.is_empty() {
        let text = match (&list.error, list.loading, &list.note) {
            (Some(e), _, _) => Paragraph::new(error_line(t, format!("Could not load the comments: {e}"))),
            (None, Loading::FirstPage | Loading::MorePages, _) => {
                Paragraph::new(Line::styled("Loading comments…", t.status_progress))
            }
            (None, _, Some(note)) => Paragraph::new(Line::styled(note.clone(), t.muted)),
            (None, _, None) => Paragraph::new(Line::styled("No comments.", t.muted)),
        };
        f.render_widget(text.wrap(Wrap { trim: true }), inner);
        return;
    }
    let rows: Vec<RowView> =
        list.rows.iter().enumerate().map(|(i, r)| row_view(t, list, r, i == list.selected, inner.width)).collect();
    let top = first_visible(&rows, list.selected, list.top.get(), inner.height);
    list.top.set(top);
    // Each row and the blank line after it.
    let heights: Vec<usize> = rows.iter().map(|r| usize::from(r.height()) + 1).collect();
    let content = heights.iter().sum::<usize>().saturating_sub(1);
    if content > usize::from(inner.height) {
        let above = heights[..top].iter().sum();
        draw_scrollbar(t, f, area, content, above, usize::from(inner.height));
    }
    let mut row_y = inner.y;
    for (i, row) in rows.iter().enumerate().skip(top) {
        if row_y >= inner.bottom() {
            break;
        }
        let height = row.height().min(inner.bottom() - row_y);
        let left = inner.x + 2 + if row.reply { REPLY_INDENT } else { 0 };
        let width = inner.right().saturating_sub(left);
        if i == list.selected {
            for dy in 0..height {
                f.render_widget(Span::styled("▌", t.accent), Rect::new(inner.x, row_y + dy, 1, 1));
            }
        }
        let mut line_y = row_y;
        let bottom = row_y + height;
        let mut put = |widget: Paragraph<'static>, lines: u16, f: &mut Frame| {
            let h = lines.min(bottom.saturating_sub(line_y));
            if h > 0 {
                f.render_widget(widget, Rect::new(left, line_y, width, h));
            }
            line_y += lines;
        };
        put(Paragraph::new(row.header.clone()), 1, f);
        if let Some(text) = &row.text {
            put(Paragraph::new(text.clone()).wrap(Wrap { trim: false }), row.text_lines, f);
        }
        if let Some(stats) = &row.stats {
            put(Paragraph::new(stats.clone()), 1, f);
        }
        // A blank line between rows.
        row_y += row.height() + 1;
    }
}

/// One row of the comments, ready to draw.
struct RowView {
    header: Line<'static>,
    text: Option<Vec<Line<'static>>>,
    /// The text lines that show, after wrapping and the limit.
    text_lines: u16,
    stats: Option<Line<'static>>,
    reply: bool,
}

impl RowView {
    fn height(&self) -> u16 {
        1 + self.text_lines + u16::from(self.stats.is_some())
    }
}

fn row_view(t: &Theme, c: &CommentsState, row: &CommentRow, selected: bool, width: u16) -> RowView {
    let (comment, expanded) = match row {
        CommentRow::MoreReplies { parent, .. } => {
            let header = if c.loading_replies.as_deref() == Some(parent) {
                Line::styled("Loading replies…", t.status_progress)
            } else {
                Line::styled("Show more replies", t.accent)
            };
            return RowView { header, text: None, text_lines: 0, stats: None, reply: true };
        }
        CommentRow::Comment { comment, expanded } => (comment, *expanded),
    };
    let reply = comment.reply_level > 0;
    let text = description_lines(t, &comment.text, if selected { c.link } else { None });
    let text_width = width.saturating_sub(2 + if reply { REPLY_INDENT } else { 0 }).max(1);
    let wrapped = u16::try_from(Paragraph::new(text.clone()).wrap(Wrap { trim: false }).line_count(text_width))
        .unwrap_or(u16::MAX);
    let text_lines = if selected { wrapped } else { wrapped.min(COMMENT_LINES) };
    let stats = comment_stats(t, comment, expanded, text_lines < wrapped);
    RowView { header: comment_header(t, comment), text: Some(text), text_lines, stats, reply }
}

fn comment_header(t: &Theme, c: &Comment) -> Line<'static> {
    let author = Span::styled(c.author.name.clone(), if c.by_uploader { t.badge_owner } else { t.title });
    let mut spans = vec![author];
    if c.verified {
        spans.push(" ✓".into());
    }
    if let Some(p) = &c.published_text {
        spans.push(Span::styled(format!(" · {p}"), t.muted));
    }
    if c.edited {
        spans.push(Span::styled(" (edited)", t.muted));
    }
    if c.pinned {
        spans.push(Span::styled(" [Pinned]", t.pinned));
    }
    if c.hearted {
        spans.push(Span::styled(" ♥", t.accent));
    }
    Line::from(spans)
}

/// "4.8M likes · 986 replies". `None` when there is nothing to say. With
/// `cut`, the text does not all show.
fn comment_stats(t: &Theme, c: &Comment, expanded: bool, cut: bool) -> Option<Line<'static>> {
    let mut parts: Vec<String> = cut.then(|| "more…".to_string()).into_iter().collect();
    parts.extend(c.likes_text.iter().map(|l| if l == "1" { "1 like".to_string() } else { format!("{l} likes") }));
    if let Some(n) = &c.reply_count_text {
        parts.push(if n == "1" { "1 reply".to_string() } else { format!("{n} replies") });
    }
    if expanded {
        parts.push("open".into());
    }
    (!parts.is_empty()).then(|| Line::styled(parts.join(" · "), t.muted))
}

/// The first row to draw, so that the selected row shows. It keeps `top`
/// when it can, so the list does not jump.
fn first_visible(rows: &[RowView], selected: usize, top: usize, height: u16) -> usize {
    let mut top = top.min(selected);
    // The rows from `top` to the selected one, with a blank line after each but the last.
    let span = |top: usize| rows[top..=selected].iter().map(|r| u32::from(r.height()) + 1).sum::<u32>() - 1;
    while top < selected && span(top) > u32::from(height) {
        top += 1;
    }
    top
}

/// The description, one `Line` for each text line, with links in color. The
/// link `selected` is reversed.
fn description_lines(t: &Theme, d: &Description, selected: Option<usize>) -> Vec<Line<'static>> {
    let style = |link: Option<usize>| match link {
        Some(i) if Some(i) == selected => t.link_selected(),
        Some(_) => t.link,
        None => Style::new(),
    };
    // The link of each char.
    let mut owner = vec![None; d.text.chars().count()];
    for (i, l) in d.links.iter().enumerate() {
        for slot in owner.iter_mut().skip(l.start).take(l.len) {
            *slot = Some(i);
        }
    }
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_link = None;
    for (c, link) in d.text.chars().zip(owner) {
        if c == '\n' || link != run_link {
            if !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), style(run_link)));
            }
            run_link = link;
        }
        if c == '\n' {
            lines.push(Line::from(std::mem::take(&mut spans)));
        } else {
            run.push(c);
        }
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, style(run_link)));
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// A thin scrollbar on the right border of `area`, a bordered block, for
/// `content` lines of which `viewport` show from `position`.
fn draw_scrollbar(t: &Theme, f: &mut Frame, area: Rect, content: usize, position: usize, viewport: usize) {
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(None)
        .thumb_symbol("┃")
        .thumb_style(t.scrollbar);
    // The scrollbar counts the positions that the top line can take.
    let mut state =
        ScrollbarState::new(content.saturating_sub(viewport) + 1).viewport_content_length(viewport).position(position);
    f.render_stateful_widget(bar, area.inner(Margin { vertical: 1, horizontal: 0 }), &mut state);
}

/// "4:05", "1:02:03".
fn duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

pub fn draw_subscriptions(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let block = bordered(t, format!(" Subscriptions ({}) ", app.subscriptions.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.subscriptions.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Toggle).map(|k| k.to_string()).unwrap_or_default();
        let text = format!("No subscriptions yet. Press {key} on a channel to subscribe.");
        f.render_widget(Paragraph::new(Line::styled(text, t.muted)).wrap(Wrap { trim: true }), inner);
        return;
    }
    let rows = app
        .subscriptions
        .iter()
        .map(|s| {
            vec![
                Line::styled(s.name.clone(), t.title),
                Line::styled(format!("  {}", s.handle.clone().unwrap_or_default()), t.muted),
            ]
        })
        .collect();
    items::draw_rows(t, rows, app.subscriptions_selection, f, inner);
}

pub fn draw_feed(app: &App, f: &mut Frame, area: Rect) {
    let feed = &app.feed;
    let t = &app.theme;
    let block = bordered(t, format!(" Feed ({}) ", crate::takeout::channels(app.subscriptions.len())));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.subscriptions.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Toggle).map(|k| k.to_string()).unwrap_or_default();
        let text = format!(
            "No subscriptions yet. Press {key} on a channel to subscribe, or import your YouTube subscriptions \
             with reeltty import (see the README)."
        );
        f.render_widget(Paragraph::new(Line::styled(text, t.muted)).wrap(Wrap { trim: true }), inner);
        return;
    }
    let [status, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let refresh = app.keymap.first_key(crate::keymap::Action::Refresh).map(|k| k.to_string()).unwrap_or_default();
    let line = match (&feed.error, feed.loading) {
        (_, n) if n > 0 => Line::styled(format!("Loading {}…", crate::takeout::channels(n)), t.status_progress),
        (Some(e), _) => error_line(t, format!("Could not load {e}")),
        (None, _) => {
            let n = feed.items.len();
            let videos = if n == 1 { "1 video".to_string() } else { format!("{n} videos") };
            Line::styled(format!("{videos}. Press {refresh} to load all channels again."), t.muted)
        }
    };
    f.render_widget(line, status);
    if feed.items.is_empty() {
        let text = if feed.loading > 0 {
            Line::styled("Loading the feed…", t.status_progress)
        } else {
            Line::styled("No videos yet.", t.muted)
        };
        f.render_widget(Paragraph::new(text), list);
        return;
    }
    items::draw_list(t, &feed.items, feed.selection, &app.thumbnails, &app.history, f, list);
}

pub fn draw_history(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let block = bordered(t, format!(" History ({}) ", app.history.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.history.is_empty() {
        let text = "No watch history yet. Videos you play show here, and play again from where you stopped.";
        f.render_widget(Paragraph::new(Line::styled(text, t.muted)).wrap(Wrap { trim: true }), inner);
        return;
    }
    let width = usize::from(inner.width).saturating_sub(2);
    let now = app.clock;
    let rows = app
        .history
        .iter()
        .map(|e| {
            let mut parts: Vec<String> = e.channel_name.iter().cloned().collect();
            parts.push(match (e.finished, e.duration_secs) {
                (true, _) => "watched".to_string(),
                (false, Some(d)) => format!("{} of {}", clock(e.position_secs), clock(d)),
                (false, None) => format!("stopped at {}", clock(e.position_secs)),
            });
            parts.push(ago(now, e.watched_at));
            vec![
                Line::styled(e.title.clone(), t.title),
                Line::styled(format!("  {}", items::truncate(&parts.join(" · "), width)), t.muted),
            ]
        })
        .collect();
    items::draw_rows(t, rows, app.history_selection, f, inner);
}

/// How long before `now` the Unix time `then` was, as YouTube writes it:
/// "just now", "5 minutes ago", "1 day ago", "3 years ago".
fn ago(now: i64, then: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let secs = (now - then).max(0);
    let (n, unit) = if secs < MINUTE {
        return "just now".into();
    } else if secs < HOUR {
        (secs / MINUTE, "minute")
    } else if secs < DAY {
        (secs / HOUR, "hour")
    } else if secs < 7 * DAY {
        (secs / DAY, "day")
    } else if secs < 30 * DAY {
        (secs / (7 * DAY), "week")
    } else if secs < 365 * DAY {
        (secs / (30 * DAY), "month")
    } else {
        (secs / (365 * DAY), "year")
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

pub fn draw_queue(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let block = bordered(t, format!(" Queue ({}) ", app.queue.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.queue.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Enqueue).map(|k| k.to_string()).unwrap_or_default();
        let text = format!(
            "The queue is empty. Press {key} on a video to add it. Videos in the queue play after the one that plays now."
        );
        f.render_widget(Paragraph::new(Line::styled(text, t.muted)).wrap(Wrap { trim: true }), inner);
        return;
    }
    let width = usize::from(inner.width);
    let rows = app.queue.iter().map(|v| items::rows(t, &Item::Video(v.clone()), None, width, false)).collect();
    items::draw_rows(t, rows, app.queue_selection, f, inner);
}

pub fn draw_saved(app: &App, f: &mut Frame, area: Rect) {
    // The same name as the tab.
    let t = &app.theme;
    let block = bordered(t, format!(" Playlists ({}) ", app.saved_playlists.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.saved_playlists.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Toggle).map(|k| k.to_string()).unwrap_or_default();
        let text = format!("No saved playlists yet. Press {key} on a playlist to save it.");
        f.render_widget(Paragraph::new(Line::styled(text, t.muted)).wrap(Wrap { trim: true }), inner);
        return;
    }
    let rows = app
        .saved_playlists
        .iter()
        .map(|p| {
            let parts: Vec<String> = p.channel_name.iter().chain(&p.videos_text).cloned().collect();
            vec![Line::styled(p.title.clone(), t.title), Line::styled(format!("  {}", parts.join(" · ")), t.muted)]
        })
        .collect();
    items::draw_rows(t, rows, app.saved_selection, f, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::model::{Link, LinkTarget};

    #[test]
    fn links_are_styled_across_lines() {
        let d = Description {
            text: "Hi ✓ see x.com\nnext".into(),
            links: vec![Link { start: 9, len: 5, target: LinkTarget::Url("https://x.com".into()) }],
        };
        let lines = description_lines(&Theme::ansi(), &d, None);
        assert_eq!(lines.len(), 2);
        let first: Vec<(&str, bool)> = lines[0]
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style.add_modifier.contains(ratatui::style::Modifier::UNDERLINED)))
            .collect();
        assert_eq!(first, [("Hi ✓ see ", false), ("x.com", true)]);
        assert_eq!(lines[1].spans[0].content, "next");
    }

    #[test]
    fn links_past_the_end_do_not_panic() {
        let d = Description {
            text: "ab".into(),
            links: vec![Link { start: 1, len: 50, target: LinkTarget::Channel { id: "UC".into() } }],
        };
        assert_eq!(description_lines(&Theme::ansi(), &d, None).len(), 1);
    }

    #[test]
    fn durations() {
        assert_eq!(duration(5), "0:05");
        assert_eq!(duration(3723), "1:02:03");
    }
}

#[cfg(test)]
mod date_tests {
    use super::ago;

    #[test]
    fn how_long_ago() {
        let now = 1_793_373_867;
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now, now + 30), "just now", "a time in the future");
        assert_eq!(ago(now, now - 59), "just now");
        assert_eq!(ago(now, now - 60), "1 minute ago");
        assert_eq!(ago(now, now - 5 * 60), "5 minutes ago");
        assert_eq!(ago(now, now - 3600), "1 hour ago");
        assert_eq!(ago(now, now - 23 * 3600), "23 hours ago");
        assert_eq!(ago(now, now - 86_400), "1 day ago");
        assert_eq!(ago(now, now - 6 * 86_400), "6 days ago");
        assert_eq!(ago(now, now - 7 * 86_400), "1 week ago");
        assert_eq!(ago(now, now - 29 * 86_400), "4 weeks ago");
        assert_eq!(ago(now, now - 30 * 86_400), "1 month ago");
        assert_eq!(ago(now, now - 364 * 86_400), "12 months ago");
        assert_eq!(ago(now, now - 365 * 86_400), "1 year ago");
        assert_eq!(ago(now, now - 3 * 365 * 86_400), "3 years ago");
    }
}
