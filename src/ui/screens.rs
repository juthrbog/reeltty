//! Screens on top of the tabs, and the Subscriptions and Playlists tabs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use super::items::{self, truncate};
use crate::app::list::{ItemList, Loading};
use crate::app::player::clock;
use crate::app::screens::{ChannelScreen, PlaylistScreen, Screen, VideoScreen};
use crate::app::{App, SCREEN_HEADER_ROWS};
use crate::source::model::{Description, Item, VideoDetails};

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
        Screen::Video(s) => {
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
    let block = Block::bordered().title(format!("Video: {}", s.title));
    let text = match (&s.details, &s.error) {
        (Some(d), _) => video_lines(app, d),
        (None, Some(e)) => vec![Line::from(format!("Could not load the video: {e}")).red()],
        (None, None) => vec![Line::from("Loading...").dim()],
    };
    let inner = block.inner(area);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let lines = u16::try_from(paragraph.line_count(inner.width)).unwrap_or(u16::MAX);
    let max = lines.saturating_sub(inner.height);
    s.max_scroll.set(max);
    f.render_widget(block, area);
    f.render_widget(paragraph.scroll((s.scroll.min(max), 0)), inner);
}

fn video_lines(app: &App, d: &VideoDetails) -> Vec<Line<'static>> {
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
    lines.extend(description_lines(&d.description));
    lines
}

/// The description, one `Line` for each text line, with links in color.
fn description_lines(d: &Description) -> Vec<Line<'static>> {
    let link_style = Style::new().cyan().add_modifier(Modifier::UNDERLINED);
    let mut is_link = vec![false; d.text.chars().count()];
    for l in &d.links {
        for flag in is_link.iter_mut().skip(l.start).take(l.len) {
            *flag = true;
        }
    }
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_link = false;
    for (c, link) in d.text.chars().zip(is_link) {
        if c == '\n' || link != run_link {
            if !run.is_empty() {
                let text = std::mem::take(&mut run);
                spans.push(if run_link { Span::styled(text, link_style) } else { Span::from(text) });
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
        spans.push(if run_link { Span::styled(run, link_style) } else { Span::from(run) });
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
        let lines = description_lines(&d);
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
        assert_eq!(description_lines(&d).len(), 1);
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
