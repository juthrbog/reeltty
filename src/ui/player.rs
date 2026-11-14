//! The full-screen player: the video, a progress bar, and a status line.
//! And the mini-player, in a dock on the right of the browse screen.
//!
//! Ghostty draws placeholder images below text, so no text is drawn on video
//! cells. The quality menu replaces the cells it covers, so the video does
//! not show there.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr as _;

use super::items::{draw_image, truncate};
use super::{bordered, error_line};
use crate::app::player::{MINI_INFO_ROWS, Phase, PlayerState, UpNext};
use crate::app::{App, StatusMessage};
use crate::keymap::Action;
use crate::keymap::PlayerAction;
use crate::source::model::Item;
use crate::terminal::kitty::VIDEO_ID;
use crate::theme::Theme;

/// The status line keeps this many columns for the title and the chapter,
/// and drops key hints for them.
const MIN_TITLE_WIDTH: usize = 20;

pub fn draw(app: &App, p: &PlayerState, f: &mut Frame) {
    let [video, progress, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Length(1)]).areas(f.area());
    if let (Phase::Ended, Some(up)) = (&p.phase, &p.up_next) {
        draw_up_next(app, up, f, video);
    } else if p.is_running() {
        let g = p.geometry;
        let area = Rect::new(video.x + g.x, video.y + g.y, g.cols, g.rows).intersection(video);
        draw_image(f.buffer_mut(), area, VIDEO_ID);
        draw_quality_menu(&app.theme, p, f, video);
    } else {
        draw_message(&app.theme, p, super::spinner(app), f, video);
    }
    draw_progress(&app.theme, p, f, progress);
    draw_status(app, p, f, status);
}

/// The list of videos that can play next, after the video ended. It takes
/// the place of the last frame.
fn draw_up_next(app: &App, up: &UpNext, f: &mut Frame, area: Rect) {
    let key = |a| app.keymap.first_key(a);
    let header = match up.countdown {
        Some(n) => format!("Up next in {n} s"),
        None => "Up next".to_string(),
    };
    // Both keys or no "choose" hint, so it never shows as `j/ choose`.
    let choose = key(Action::Down).zip(key(Action::Up)).map(|(d, u)| format!("{d}/{u}"));
    let t = &app.theme;
    let help = super::hint_title(
        t,
        [
            (choose, "choose"),
            (key(Action::Open).map(|k| k.to_string()), "play"),
            (app.keymap.player.first_key(PlayerAction::Back).map(|k| k.to_string()), "back"),
        ],
    );
    let width = area.width.min(100);
    let [_, column, _] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width), Constraint::Fill(1)]).areas(area);
    let mut block = bordered(t, format!(" {header} "));
    if let Some(help) = help {
        block = block.title_bottom(help);
    }
    let inner = block.inner(column);
    f.render_widget(block, column);
    let width = usize::from(inner.width).saturating_sub(super::items::MARK_WIDTH);
    let rows = up
        .videos
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let state = super::items::RowState {
                selected: i == up.selection.min(up.videos.len().saturating_sub(1)),
                playing: false,
            };
            super::items::rows(t, &Item::Video(v.clone()), None, width, false, state)
        })
        .collect();
    let selection = crate::app::Selection { index: up.selection, offset: 0 };
    super::items::draw_rows(t, rows, selection, f, inner);
}

fn draw_quality_menu(t: &Theme, p: &PlayerState, f: &mut Frame, area: Rect) {
    let Some(menu) = &p.quality_menu else { return };
    let items: Vec<ListItem> = p
        .qualities
        .iter()
        .map(|q| {
            let mark = if Some(q.res) == p.quality { Span::styled("●", t.accent) } else { Span::from(" ") };
            ListItem::new(Line::from(vec![Span::from(" "), mark, Span::from(format!(" {}p", q.res))]))
        })
        .collect();
    let height = u16::try_from(items.len()).unwrap_or(u16::MAX).saturating_add(2).min(area.height);
    let width = 16.min(area.width);
    let [_, column, _] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width), Constraint::Fill(1)]).areas(area);
    let [_, popup, _] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height), Constraint::Fill(1)]).areas(column);
    let list = List::new(items).block(bordered(t, " Quality ")).highlight_style(t.selection);
    f.render_widget(Clear, popup);
    f.render_stateful_widget(list, popup, &mut ListState::default().with_selected(Some(menu.selection)));
}

/// The mini-player in the `dock`: the video in a box, with the title, the
/// progress bar, and the state under it. The video geometry is from the top
/// left of the screen.
pub fn draw_mini(app: &App, p: &PlayerState, f: &mut Frame, dock: Rect) {
    let g = p.geometry;
    let frame = Rect::new(dock.x, g.y.saturating_sub(1), dock.width, g.rows.saturating_add(2)).intersection(dock);
    // Only the border takes a style: the video is inside the block.
    let theme = &app.theme;
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(theme.border);
    let inner = block.inner(frame);
    f.render_widget(block, frame);
    if let (Phase::Ended, Some(up)) = (&p.phase, &p.up_next) {
        let header = match up.countdown {
            Some(n) => format!("Up next in {n} s"),
            None => "Up next".to_string(),
        };
        let title = up.videos.get(up.selection).map(|v| v.title.clone()).unwrap_or_default();
        draw_centered(f, inner, vec![Line::styled(header, theme.muted), Line::styled(title, theme.title)]);
    } else if p.is_running() {
        let area = Rect::new(g.x, g.y, g.cols, g.rows).intersection(inner);
        draw_image(f.buffer_mut(), area, VIDEO_ID);
    } else {
        draw_centered(f, inner, vec![phase_message(theme, p, super::spinner(app))]);
    }
    let info = Rect::new(dock.x, frame.bottom(), dock.width, MINI_INFO_ROWS).intersection(dock);
    let [title, progress, state] = Layout::vertical([Constraint::Length(1); 3]).areas(info);
    let width = usize::from(title.width).saturating_sub(1);
    f.render_widget(Line::styled(format!(" {}", truncate(&p.title, width)), theme.title), title);
    draw_progress(theme, p, f, progress);
    draw_mini_state(app, p, f, state);
}

/// The state of the mini-player, and the keys for full size and stop.
fn draw_mini_state(app: &App, p: &PlayerState, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let mut parts = status_parts(p);
    // Only the keys that the configuration file did not take away.
    let mut hints = Some(
        [(Action::ShowPlayer, " full"), (Action::ClosePlayer, " stop")]
            .into_iter()
            .filter_map(|(a, label)| {
                app.keymap.first_key(a).map(|k| [Span::styled(k.to_string(), t.key), Span::styled(label, t.key_label)])
            })
            .collect::<Vec<_>>()
            .join(&Span::from("  ")),
    );
    // Drop the state from its end first, but keep the first part, then the hints.
    let line = loop {
        let mut spans = vec![Span::from(" ")];
        if !parts.is_empty() {
            spans.push(Span::styled(format!("{}  ", parts.join("  ")), t.muted));
        }
        spans.extend(hints.iter().flatten().cloned());
        let line = Line::from(spans);
        if line.width() <= usize::from(area.width) {
            break line;
        }
        if parts.len() > 1 {
            parts.pop();
            continue;
        }
        if hints.take().is_some() {
            continue;
        }
        break line;
    };
    f.render_widget(line, area);
}

/// `lines` in the middle of `area`, wrapped.
fn draw_centered(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let text = Paragraph::new(lines).alignment(Alignment::Center).wrap(Wrap { trim: true });
    // The height after wrapping, so a long error is not cut off.
    let height = u16::try_from(text.line_count(area.width)).unwrap_or(u16::MAX).min(area.height);
    let [_, middle, _] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height), Constraint::Fill(1)]).areas(area);
    f.render_widget(text, middle);
}

/// What happens before the video plays, or why it failed.
fn phase_message(t: &Theme, p: &PlayerState, spin: Option<&str>) -> Line<'static> {
    match &p.phase {
        Phase::Resolving => {
            let text = Span::styled("Finding the video streams…", t.status_progress);
            match spin {
                Some(frame) => Line::from(vec![Span::styled(frame.to_string(), t.accent), Span::from(" "), text]),
                None => Line::from(text),
            }
        }
        Phase::Waiting { .. } => Line::from(format!(
            "YouTube makes guests wait for the ads. The video starts in {} s.",
            p.wait_left().unwrap_or(0)
        )),
        Phase::Failed(e) => error_line(t, format!("Cannot play this video: {e}")),
        Phase::Loading | Phase::Playing | Phase::Ended => Line::default(),
    }
}

/// The mini-player as a status bar message, for a screen with no room for
/// the dock.
pub fn status_message(p: &PlayerState) -> StatusMessage {
    let state = match &p.phase {
        Phase::Failed(e) => return StatusMessage::error(format!("Cannot play {}: {e}", p.title)),
        Phase::Resolving => " · finding the video streams…".to_string(),
        Phase::Waiting { .. } => format!(" · starts in {} s", p.wait_left().unwrap_or(0)),
        Phase::Loading | Phase::Playing | Phase::Ended => match p.duration_ms {
            Some(d) => format!(" {} / {}", clock(p.position_ms), clock(d)),
            None => format!(" {}", clock(p.position_ms)),
        },
    };
    StatusMessage::info(format!("▶ {}{state}", p.title))
}

/// The text in place of the video before it plays, or after it failed.
fn draw_message(t: &Theme, p: &PlayerState, spin: Option<&str>, f: &mut Frame, area: Rect) {
    draw_centered(f, area, vec![Line::styled(p.title.clone(), t.title), Line::default(), phase_message(t, p, spin)]);
}

/// `1:05`, or `1:02:05` from one hour.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn draw_progress(t: &Theme, p: &PlayerState, f: &mut Frame, area: Rect) {
    if !p.is_running() {
        return;
    }
    let left = clock(p.position_ms);
    let right = p.duration_ms.map(clock).unwrap_or_default();
    let bar_width = usize::from(area.width).saturating_sub(left.width() + right.width() + 4);
    let mut spans = vec![Span::from(format!(" {left} "))];
    spans.extend(bar(t, p, bar_width));
    spans.push(Span::from(format!(" {right} ")));
    f.render_widget(Line::from(spans), area);
}

/// The progress bar, `width` cells wide. Parts that SponsorBlock skips have
/// a color and glyphs of their own: `┅` played and `┄` not played.
fn bar(t: &Theme, p: &PlayerState, width: usize) -> Vec<Span<'static>> {
    let Some(d) = p.duration_ms.filter(|d| *d > 0) else {
        return vec![Span::styled("─".repeat(width), t.progress_track)];
    };
    let segments = p.skip_segments();
    // The time at the start of cell `i`.
    let at = |i: usize| u64::try_from(u128::from(d) * i as u128 / width.max(1) as u128).unwrap_or(d);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style: Option<(Style, bool)> = None;
    for i in 0..width {
        let (start, end) = (at(i), at(i + 1));
        let played = end <= p.position_ms.min(d);
        let skipped = segments.iter().any(|s| s.start_ms < end && start < s.end_ms);
        let style = match (played, skipped) {
            (_, true) => t.progress_sponsor,
            (true, false) => t.progress_played,
            (false, false) => t.progress_track,
        };
        if run_style.is_some_and(|s| s != (style, played)) {
            spans.push(Span::styled(std::mem::take(&mut run), run_style.map_or_else(Style::new, |s| s.0)));
        }
        run.push(match (played, skipped) {
            (true, true) => '┅',
            (false, true) => '┄',
            (true, false) => '━',
            (false, false) => '─',
        });
        run_style = Some((style, played));
    }
    if let Some((style, _)) = run_style {
        spans.push(Span::styled(run, style));
    }
    spans
}

fn draw_status(app: &App, p: &PlayerState, f: &mut Frame, area: Rect) {
    // A message from the last key takes the whole line, because it is the
    // only answer to that key.
    if let Some(status) = &app.status {
        // The mark ("! ", "✓ ", "✗ ") takes room too.
        let spin = super::spinner(app);
        let mark = super::status_line(&app.theme, &StatusMessage::new(status.kind, ""), spin).width();
        let width = usize::from(area.width).saturating_sub(1 + mark);
        let shown = StatusMessage::new(status.kind, truncate(status, width));
        let mut line = super::status_line(&app.theme, &shown, spin);
        line.spans.insert(0, Span::from(" "));
        f.render_widget(line, area);
        return;
    }
    let mut parts = status_parts(p);
    // A hint whose key is not bound is left out. Back is always bound.
    let hint = |a, label: &'static str| {
        let key = app.keymap.player.first_key(a)?;
        Some(vec![
            Span::from("  "),
            Span::styled(key.to_string(), app.theme.key),
            Span::styled(label, app.theme.key_label),
        ])
    };
    let mut pause = p.is_running().then(|| hint(PlayerAction::TogglePause, " pause")).flatten();
    let mut mini = hint(PlayerAction::Minimize, " mini");
    let mut back = hint(PlayerAction::Back, " back");
    // Leave room for the title. Drop the mini and pause hints first, then
    // the state from its end, and the back hint last.
    let right = loop {
        let mut spans = vec![Span::styled(parts.join("  "), app.theme.muted)];
        spans.extend(back.iter().chain(&mini).chain(&pause).flatten().cloned());
        let line = Line::from(spans);
        if line.width() + MIN_TITLE_WIDTH <= usize::from(area.width) {
            break line;
        }
        if mini.take().is_some() || pause.take().is_some() {
            continue;
        }
        if parts.len() > 1 {
            parts.pop();
            continue;
        }
        if back.take().is_some() {
            continue;
        }
        break line;
    };
    let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX).saturating_add(1).min(area.width);
    let [left, hints] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    // A space before the title and one after it.
    let width = usize::from(left.width).saturating_sub(2);
    // The chapter goes first, because it changes and a long title would hide it.
    let title = match p.chapter() {
        Some(c) => format!("{} · {}", c.title, p.title),
        None => p.title.clone(),
    };
    f.render_widget(Line::styled(format!(" {}", truncate(&title, width)), app.theme.title), left);
    f.render_widget(right, hints);
}

/// The state of the player, such as "Paused", the speed, and the volume.
/// Empty until mpv runs.
fn status_parts(p: &PlayerState) -> Vec<String> {
    if !p.is_running() {
        return Vec::new();
    }
    let state = match p.phase {
        Phase::Ended => "Ended",
        Phase::Loading => "Loading…",
        _ if p.buffering => "Buffering…",
        Phase::Playing if p.paused => "Paused",
        _ => "Playing",
    };
    let mut parts = vec![state.to_string()];
    if p.speed != 100 {
        parts.push(format!("{}.{:02}x", p.speed / 100, p.speed % 100));
    }
    if p.caption.is_some() {
        parts.push("CC".into());
    }
    if let Some(q) = p.quality {
        parts.push(format!("{q}p"));
    }
    parts.push(if p.muted { "muted".to_string() } else { format!("vol {}%", p.volume) });
    parts
}
