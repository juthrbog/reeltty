//! The full-screen player: the video, a progress bar, and a status line.
//!
//! Ghostty draws placeholder images below text, so no text is drawn on video
//! cells. The quality menu replaces the cells it covers, so the video does
//! not show there.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr as _;

use super::items::{draw_image, truncate};
use crate::app::App;
use crate::app::player::{Phase, PlayerState};
use crate::keymap::PlayerAction;
use crate::terminal::kitty::VIDEO_ID;

/// The status line keeps this many columns for the title and the chapter,
/// and drops key hints for them.
const MIN_TITLE_WIDTH: usize = 20;

pub fn draw(app: &App, p: &PlayerState, f: &mut Frame) {
    let [video, progress, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Length(1)]).areas(f.area());
    if p.is_running() {
        let g = p.geometry;
        let area = Rect::new(video.x + g.x, video.y + g.y, g.cols, g.rows).intersection(video);
        draw_image(f.buffer_mut(), area, VIDEO_ID);
        draw_quality_menu(p, f, video);
    } else {
        draw_message(p, f, video);
    }
    draw_progress(p, f, progress);
    draw_status(app, p, f, status);
}

fn draw_quality_menu(p: &PlayerState, f: &mut Frame, area: Rect) {
    let Some(menu) = &p.quality_menu else { return };
    let items: Vec<ListItem> = p
        .qualities
        .iter()
        .map(|q| {
            let mark = if Some(q.res) == p.quality { "●" } else { " " };
            ListItem::new(format!(" {mark} {}p", q.res))
        })
        .collect();
    let height = u16::try_from(items.len()).unwrap_or(u16::MAX).saturating_add(2).min(area.height);
    let width = 16.min(area.width);
    let [_, column, _] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width), Constraint::Fill(1)]).areas(area);
    let [_, popup, _] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height), Constraint::Fill(1)]).areas(column);
    let list = List::new(items)
        .block(Block::bordered().title(" Quality "))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_widget(Clear, popup);
    f.render_stateful_widget(list, popup, &mut ListState::default().with_selected(Some(menu.selection)));
}

/// The text in place of the video before it plays, or after it failed.
fn draw_message(p: &PlayerState, f: &mut Frame, area: Rect) {
    let message = match &p.phase {
        Phase::Resolving => Line::from("Finding the video streams…").dim(),
        Phase::Waiting { .. } => Line::from(format!(
            "YouTube makes guests wait for the ads. The video starts in {} s.",
            p.wait_left().unwrap_or(0)
        )),
        Phase::Failed(e) => Line::from(format!("Cannot play this video: {e}")).red(),
        Phase::Loading | Phase::Playing | Phase::Ended => Line::default(),
    };
    let lines = vec![Line::from(p.title.clone()).bold(), Line::default(), message];
    let text = Paragraph::new(lines).alignment(Alignment::Center).wrap(Wrap { trim: true });
    // The height after wrapping, so a long error is not cut off.
    let height = u16::try_from(text.line_count(area.width)).unwrap_or(u16::MAX).min(area.height);
    let [_, middle, _] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height), Constraint::Fill(1)]).areas(area);
    f.render_widget(text, middle);
}

/// `1:05`, or `1:02:05` from one hour.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn draw_progress(p: &PlayerState, f: &mut Frame, area: Rect) {
    if !p.is_running() {
        return;
    }
    let left = clock(p.position_ms);
    let right = p.duration_ms.map(clock).unwrap_or_default();
    let bar_width = usize::from(area.width).saturating_sub(left.width() + right.width() + 4);
    let filled = match p.duration_ms {
        Some(d) if d > 0 => {
            let filled = u128::from(p.position_ms.min(d)) * bar_width as u128 / u128::from(d);
            usize::try_from(filled).unwrap_or(bar_width)
        }
        _ => 0,
    };
    let line = Line::from(vec![
        Span::from(format!(" {left} ")),
        Span::from("━".repeat(filled)).red(),
        Span::from("─".repeat(bar_width - filled)).dim(),
        Span::from(format!(" {right} ")),
    ]);
    f.render_widget(line, area);
}

fn draw_status(app: &App, p: &PlayerState, f: &mut Frame, area: Rect) {
    // A message from the last key takes the whole line, because it is the
    // only answer to that key.
    if let Some(status) = &app.status {
        let width = usize::from(area.width).saturating_sub(1);
        f.render_widget(Line::from(format!(" {}", truncate(status, width))).yellow(), area);
        return;
    }
    let state = match p.phase {
        Phase::Ended => "Ended",
        Phase::Loading => "Loading…",
        _ if p.buffering => "Buffering…",
        Phase::Playing if p.paused => "Paused",
        Phase::Playing => "Playing",
        Phase::Resolving | Phase::Waiting { .. } | Phase::Failed(_) => "",
    };
    let mut parts = Vec::new();
    if p.is_running() {
        parts.push(state.to_string());
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
    }
    let key = |a| app.keymap.player.first_key(a).map(|k| k.to_string()).unwrap_or_default();
    let hint = |a, label: &'static str| vec![Span::from("  "), Span::from(key(a)).bold(), Span::from(label)];
    let mut pause = p.is_running().then(|| hint(PlayerAction::TogglePause, " pause"));
    let mut back = Some(hint(PlayerAction::Back, " back"));
    // Leave room for the title. Drop the pause hint first, then the state
    // from its end, and the back hint last.
    let right = loop {
        let mut spans = vec![Span::from(parts.join("  ")).dim()];
        spans.extend(back.iter().chain(&pause).flatten().cloned());
        let line = Line::from(spans);
        if line.width() + MIN_TITLE_WIDTH <= usize::from(area.width) {
            break line;
        }
        if pause.take().is_some() {
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
    f.render_widget(Line::from(format!(" {}", truncate(&title, width))).bold(), left);
    f.render_widget(right, hints);
}
