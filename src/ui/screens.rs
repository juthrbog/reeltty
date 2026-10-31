//! Screens on top of the tabs, and the Subscriptions and Playlists tabs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use super::items::{self, truncate};
use crate::app::comments::{CommentRow, CommentsState};
use crate::app::list::{ItemList, Loading};
use crate::app::player::clock;
use crate::app::screens::{ChannelScreen, PlaylistScreen, Screen, VideoPane, VideoScreen};
use crate::app::{App, SCREEN_HEADER_ROWS};
use crate::keymap::Action;
use crate::source::model::{Comment, Description, Item, LinkTarget, VideoDetails};

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

/// The status bar message for a screen, and whether it is an error.
pub fn status(screen: &Screen) -> Option<(String, bool)> {
    let list = match screen {
        Screen::Channel(s) => &s.list,
        Screen::Playlist(s) => &s.list,
        Screen::Video(s) if s.pane == VideoPane::Comments => return s.comments.as_deref().and_then(comments_status),
        Screen::Video(s) => {
            let link = s.link.and_then(|i| s.details.as_ref()?.description.links.get(i));
            if let Some(link) = link {
                return Some((link_status(&link.target, &s.id), false));
            }
            return match (&s.error, s.loading) {
                (Some(e), _) => Some((format!("Could not load the video: {e}"), true)),
                (None, true) => Some(("Loading...".into(), false)),
                (None, false) => None,
            };
        }
    };
    match (&list.error, list.loading) {
        (Some(e), _) => Some((format!("Could not load: {e}"), true)),
        (None, Loading::FirstPage) => Some(("Loading...".into(), false)),
        (None, Loading::MorePages) => Some(("Loading more...".into(), false)),
        (None, Loading::Idle) => None,
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

fn comments_status(c: &CommentsState) -> Option<(String, bool)> {
    if let Some(e) = &c.error {
        return Some((format!("Could not load the comments: {e}"), true));
    }
    let link = c.link.and_then(|i| c.selected_comment()?.text.links.get(i));
    // The video of a comment link is not known here, so timestamps say the time only.
    if let Some(link) = link {
        let text = match &link.target {
            LinkTarget::Video { start_secs: Some(secs), .. } => format!("enter: play from {}", duration(*secs)),
            target => link_status(target, ""),
        };
        return Some((text, false));
    }
    let text = match c.loading {
        Loading::FirstPage => "Loading comments...",
        Loading::MorePages => "Loading more comments...",
        Loading::Idle if c.loading_replies.is_some() => "Loading replies...",
        Loading::Idle => return None,
    };
    Some((text.into(), false))
}

/// A bordered block with a header of `SCREEN_HEADER_ROWS` rows above a list.
fn header_and_list(title: Line<'static>, f: &mut Frame, area: Rect) -> (Rect, Rect) {
    let block = Block::bordered().title(title);
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
        title.push(Span::from(" [Subscribed]").green());
    }
    let (header, list) = header_and_list(Line::from(title), f, area);
    let width = usize::from(header.width);
    let lines = match &s.info {
        Some(i) => {
            let facts: Vec<String> =
                i.handle.iter().chain(&i.subscribers_text).chain(&i.videos_text).cloned().collect();
            let about = i.description.as_deref().and_then(|d| d.lines().find(|l| !l.trim().is_empty())).unwrap_or("");
            vec![Line::from(truncate(&facts.join(" · "), width)), Line::from(truncate(about, width)).dim()]
        }
        None => Vec::new(),
    };
    f.render_widget(Paragraph::new(lines), header);
    draw_items(app, &s.list, "This channel has no videos.", f, list);
}

fn draw_playlist(app: &App, s: &PlaylistScreen, f: &mut Frame, area: Rect) {
    let mut title = vec![Span::from(format!("Playlist: {}", s.title))];
    if app.is_saved(&s.id) {
        title.push(Span::from(" [Saved]").green());
    }
    let (header, list) = header_and_list(Line::from(title), f, area);
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
            vec![Line::from(truncate(&facts.join(" · "), width)), Line::from(truncate(about, width)).dim()]
        }
        None => Vec::new(),
    };
    f.render_widget(Paragraph::new(lines), header);
    draw_items(app, &s.list, "This playlist has no videos.", f, list);
}

fn draw_items(app: &App, list: &ItemList, empty: &str, f: &mut Frame, area: Rect) {
    if list.items.is_empty() {
        let text = match (&list.error, list.loading) {
            (Some(e), _) => Paragraph::new(format!("Could not load: {e}")).red().wrap(Wrap { trim: true }),
            (None, Loading::FirstPage) => Paragraph::new("Loading...").dim(),
            (None, _) => Paragraph::new(empty.to_string()).dim(),
        };
        f.render_widget(text, area);
        return;
    }
    items::draw_list(&list.items, list.selection, &app.thumbnails, f, area);
}

fn draw_video(app: &App, s: &VideoScreen, f: &mut Frame, area: Rect) {
    if let (VideoPane::Comments, Some(c)) = (s.pane, &s.comments) {
        return draw_comments(app, s, c, f, area);
    }
    let block = Block::bordered().title(format!("Video: {}", s.title));
    let inner = block.inner(area);
    let (text, link_line) = match (&s.details, &s.error) {
        (Some(d), _) => {
            let (lines, description_start) = video_lines(app, d, s.link);
            // The wrapped line where the selected link starts.
            let link_line = s.link.and_then(|i| d.description.links.get(i)).map(|l| {
                let newlines = d.description.text.chars().take(l.start).filter(|&c| c == '\n').count();
                let before = Paragraph::new(lines[..description_start + newlines].to_vec()).wrap(Wrap { trim: false });
                u16::try_from(before.line_count(inner.width)).unwrap_or(u16::MAX)
            });
            (lines, link_line)
        }
        (None, Some(e)) => (vec![Line::from(format!("Could not load the video: {e}")).red()], None),
        (None, None) => (vec![Line::from("Loading...").dim()], None),
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
}

/// The lines of the details, with the selected link marked, and the index
/// of the first line of the description.
fn video_lines(app: &App, d: &VideoDetails, link: Option<usize>) -> (Vec<Line<'static>>, usize) {
    let mut lines = vec![Line::from(d.title.clone()).bold()];

    let mut channel = Vec::new();
    if let Some(c) = &d.channel {
        channel.push(Span::from(c.name.clone()).cyan());
        if let Some(subs) = &d.subscribers_text {
            channel.push(Span::from(format!(" · {subs}")));
        }
        if c.id.as_deref().is_some_and(|id| app.is_subscribed(id)) {
            channel.push(Span::from(" [Subscribed]").green());
        }
    }
    lines.push(Line::from(channel));

    let mut facts = Vec::new();
    if d.live {
        facts.push("LIVE".to_string());
    }
    facts.extend(d.views_text.clone());
    facts.extend(d.date_text.clone());
    facts.extend(d.likes_text.as_ref().map(|l| format!("{l} likes")));
    facts.extend(d.duration_secs.map(duration));
    lines.push(Line::from(facts.join(" · ")).dim());
    let mut hints = Vec::new();
    if let Some(key) = app.keymap.first_key(Action::Comments).filter(|_| !d.live) {
        let count = d.comments_count_text.as_ref().map(|c| format!(" ({c})")).unwrap_or_default();
        hints.push((format!("Comments{count}: press "), key));
    }
    if let Some(key) = app.keymap.first_key(Action::NextLink).filter(|_| !d.description.links.is_empty()) {
        hints.push(("Links: press ".to_string(), key));
    }
    if !hints.is_empty() {
        let mut spans = Vec::new();
        for (i, (text, key)) in hints.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::from(" · ").dim());
            }
            spans.extend([Span::from(text).dim(), Span::from(key.to_string()).bold()]);
        }
        lines.push(Line::from(spans));
    }

    if !d.chapters.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from("Chapters").bold());
        for c in &d.chapters {
            lines.push(Line::from(vec![
                Span::from(format!("{:>9}  ", duration(c.start_secs))).dim(),
                c.title.clone().into(),
            ]));
        }
    }

    lines.push(Line::default());
    let description_start = lines.len();
    lines.extend(description_lines(&d.description, link));
    (lines, description_start)
}

/// The comments of a video, with the selected one in full and marked with a
/// bar on the left.
fn draw_comments(app: &App, screen: &VideoScreen, list: &CommentsState, f: &mut Frame, area: Rect) {
    let count = list.count_text.clone().or_else(|| screen.details.as_ref().and_then(|d| d.comments_count_text.clone()));
    let mut title: Vec<String> = vec!["Comments".into()];
    title.extend(count);
    title.extend(list.sort().map(|o| o.label.clone()));
    let mut block = Block::bordered().title(title.join(" · "));
    let hints: Vec<String> =
        [(Action::Open, "replies"), (Action::NextLink, "link"), (Action::Sort, "order"), (Action::Comments, "details")]
            .into_iter()
            .filter_map(|(a, label)| app.keymap.first_key(a).map(|k| format!("{k} {label}")))
            .collect();
    if !hints.is_empty() {
        block = block.title_bottom(Line::from(format!(" {} ", hints.join(" · "))).dim());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if list.rows.is_empty() {
        let text = match (&list.error, list.loading, &list.note) {
            (Some(e), _, _) => Paragraph::new(format!("Could not load the comments: {e}")).red(),
            (None, Loading::FirstPage | Loading::MorePages, _) => Paragraph::new("Loading comments...").dim(),
            (None, _, Some(note)) => Paragraph::new(note.clone()).dim(),
            (None, _, None) => Paragraph::new("No comments.").dim(),
        };
        f.render_widget(text.wrap(Wrap { trim: true }), inner);
        return;
    }
    let rows: Vec<RowView> =
        list.rows.iter().enumerate().map(|(i, r)| row_view(list, r, i == list.selected, inner.width)).collect();
    let top = first_visible(&rows, list.selected, list.top.get(), inner.height);
    list.top.set(top);
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
                f.render_widget(Span::from("▌").cyan(), Rect::new(inner.x, row_y + dy, 1, 1));
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

fn row_view(c: &CommentsState, row: &CommentRow, selected: bool, width: u16) -> RowView {
    let (comment, expanded) = match row {
        CommentRow::MoreReplies { parent, .. } => {
            let label =
                if c.loading_replies.as_deref() == Some(parent) { "Loading replies..." } else { "Show more replies" };
            return RowView { header: Line::from(label).cyan(), text: None, text_lines: 0, stats: None, reply: true };
        }
        CommentRow::Comment { comment, expanded } => (comment, *expanded),
    };
    let reply = comment.reply_level > 0;
    let text = description_lines(&comment.text, if selected { c.link } else { None });
    let text_width = width.saturating_sub(2 + if reply { REPLY_INDENT } else { 0 }).max(1);
    let wrapped = u16::try_from(Paragraph::new(text.clone()).wrap(Wrap { trim: false }).line_count(text_width))
        .unwrap_or(u16::MAX);
    let text_lines = if selected { wrapped } else { wrapped.min(COMMENT_LINES) };
    let stats = comment_stats(comment, expanded, text_lines < wrapped);
    RowView { header: comment_header(comment), text: Some(text), text_lines, stats, reply }
}

fn comment_header(c: &Comment) -> Line<'static> {
    let mut author = Span::from(c.author.name.clone()).cyan();
    if c.by_uploader {
        author = author.reversed();
    }
    let mut spans = vec![author];
    if c.verified {
        spans.push(" ✓".into());
    }
    if let Some(p) = &c.published_text {
        spans.push(Span::from(format!(" · {p}")).dim());
    }
    if c.edited {
        spans.push(Span::from(" (edited)").dim());
    }
    if c.pinned {
        spans.push(Span::from(" [Pinned]").yellow());
    }
    if c.hearted {
        spans.push(Span::from(" ♥").red());
    }
    Line::from(spans)
}

/// "4.8M likes · 986 replies". `None` when there is nothing to say. With
/// `cut`, the text does not all show.
fn comment_stats(c: &Comment, expanded: bool, cut: bool) -> Option<Line<'static>> {
    let mut parts: Vec<String> = cut.then(|| "more…".to_string()).into_iter().collect();
    parts.extend(c.likes_text.iter().map(|l| if l == "1" { "1 like".to_string() } else { format!("{l} likes") }));
    if let Some(n) = &c.reply_count_text {
        parts.push(if n == "1" { "1 reply".to_string() } else { format!("{n} replies") });
    }
    if expanded {
        parts.push("open".into());
    }
    (!parts.is_empty()).then(|| Line::from(parts.join(" · ")).dim())
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
fn description_lines(d: &Description, selected: Option<usize>) -> Vec<Line<'static>> {
    let link_style = Style::new().cyan().add_modifier(Modifier::UNDERLINED);
    let style = |link: Option<usize>| match link {
        Some(i) if Some(i) == selected => link_style.add_modifier(Modifier::REVERSED),
        Some(_) => link_style,
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

/// "4:05", "1:02:03".
fn duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

pub fn draw_subscriptions(app: &App, f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(format!("Subscriptions ({})", app.subscriptions.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.subscriptions.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Toggle).map(|k| k.to_string()).unwrap_or_default();
        let text = format!("No subscriptions yet. Press {key} on a channel to subscribe.");
        f.render_widget(Paragraph::new(text).dim().wrap(Wrap { trim: true }), inner);
        return;
    }
    let rows = app
        .subscriptions
        .iter()
        .map(|s| {
            vec![
                Line::from(s.name.clone()).bold(),
                Line::from(format!("  {}", s.handle.clone().unwrap_or_default())).dim(),
            ]
        })
        .collect();
    items::draw_rows(rows, app.subscriptions_selection, f, inner);
}

pub fn draw_feed(app: &App, f: &mut Frame, area: Rect) {
    let feed = &app.feed;
    let block = Block::bordered().title(format!("Feed ({})", crate::takeout::channels(app.subscriptions.len())));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.subscriptions.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Toggle).map(|k| k.to_string()).unwrap_or_default();
        let text = format!(
            "No subscriptions yet. Press {key} on a channel to subscribe, or import your YouTube subscriptions \
             with reeltty import (see the README)."
        );
        f.render_widget(Paragraph::new(text).dim().wrap(Wrap { trim: true }), inner);
        return;
    }
    let [status, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let refresh = app.keymap.first_key(crate::keymap::Action::Refresh).map(|k| k.to_string()).unwrap_or_default();
    let line = match (&feed.error, feed.loading) {
        (_, n) if n > 0 => Line::from(format!("Loading {}…", crate::takeout::channels(n))).dim(),
        (Some(e), _) => Line::from(format!("Could not load {e}")).red(),
        (None, _) => {
            let n = feed.items.len();
            let videos = if n == 1 { "1 video".to_string() } else { format!("{n} videos") };
            Line::from(format!("{videos}. Press {refresh} to load all channels again.")).dim()
        }
    };
    f.render_widget(line, status);
    if feed.items.is_empty() {
        let text = if feed.loading > 0 { "Loading the feed…" } else { "No videos yet." };
        f.render_widget(Paragraph::new(text).dim(), list);
        return;
    }
    items::draw_list(&feed.items, feed.selection, &app.thumbnails, f, list);
}

pub fn draw_history(app: &App, f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(format!("History ({})", app.history.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.history.is_empty() {
        let text = "No watch history yet. Videos you play show here, and play again from where you stopped.";
        f.render_widget(Paragraph::new(text).dim().wrap(Wrap { trim: true }), inner);
        return;
    }
    let width = usize::from(inner.width).saturating_sub(2);
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
            parts.push(date(e.watched_at));
            vec![
                Line::from(e.title.clone()).bold(),
                Line::from(format!("  {}", items::truncate(&parts.join(" · "), width))).dim(),
            ]
        })
        .collect();
    items::draw_rows(rows, app.history_selection, f, inner);
}

/// `2026-10-30` from Unix seconds, in UTC.
fn date(unix: i64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02}")
}

pub fn draw_queue(app: &App, f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(format!("Queue ({})", app.queue.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.queue.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Enqueue).map(|k| k.to_string()).unwrap_or_default();
        let text = format!(
            "The queue is empty. Press {key} on a video to add it. Videos in the queue play after the one that plays now."
        );
        f.render_widget(Paragraph::new(text).dim().wrap(Wrap { trim: true }), inner);
        return;
    }
    let width = usize::from(inner.width);
    let rows = app.queue.iter().map(|v| items::rows(&Item::Video(v.clone()), width, false)).collect();
    items::draw_rows(rows, app.queue_selection, f, inner);
}

pub fn draw_saved(app: &App, f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(format!("Saved playlists ({})", app.saved_playlists.len()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.saved_playlists.is_empty() {
        let key = app.keymap.first_key(crate::keymap::Action::Toggle).map(|k| k.to_string()).unwrap_or_default();
        let text = format!("No saved playlists yet. Press {key} on a playlist to save it.");
        f.render_widget(Paragraph::new(text).dim().wrap(Wrap { trim: true }), inner);
        return;
    }
    let rows = app
        .saved_playlists
        .iter()
        .map(|p| {
            let parts: Vec<String> = p.channel_name.iter().chain(&p.videos_text).cloned().collect();
            vec![Line::from(p.title.clone()).bold(), Line::from(format!("  {}", parts.join(" · "))).dim()]
        })
        .collect();
    items::draw_rows(rows, app.saved_selection, f, inner);
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
        let lines = description_lines(&d, None);
        assert_eq!(lines.len(), 2);
        let first: Vec<(&str, bool)> = lines[0]
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style.add_modifier.contains(Modifier::UNDERLINED)))
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
        assert_eq!(description_lines(&d, None).len(), 1);
    }

    #[test]
    fn durations() {
        assert_eq!(duration(5), "0:05");
        assert_eq!(duration(3723), "1:02:03");
    }
}

#[cfg(test)]
mod date_tests {
    #[test]
    fn dates_from_unix_time() {
        assert_eq!(super::date(0), "1970-01-01");
        assert_eq!(super::date(951_782_400), "2000-02-29");
        assert_eq!(super::date(1_793_373_867), "2026-10-30");
        assert_eq!(super::date(-86_400), "1969-12-31");
    }
}
