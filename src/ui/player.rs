//! The full-screen player: the video, a progress bar, and a status line.
//!
//! Ghostty draws placeholder images below text, so nothing else is drawn
//! where the video is.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Stylize as _;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr as _;

use super::items::{draw_image, truncate};
use crate::app::App;
use crate::app::player::{Phase, PlayerState};
use crate::keymap::PlayerAction;
use crate::terminal::kitty::VIDEO_ID;

pub fn draw(app: &App, p: &PlayerState, f: &mut Frame) {
    let [video, progress, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Length(1)]).areas(f.area());
    if p.is_running() {
        let g = p.geometry;
        let area = Rect::new(video.x + g.x, video.y + g.y, g.cols, g.rows).intersection(video);
        draw_image(f.buffer_mut(), area, VIDEO_ID);
    } else {
        draw_message(p, f, video);
    }
    draw_progress(p, f, progress);
    draw_status(app, p, f, status);
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
    let height = u16::try_from(lines.len()).unwrap_or(0);
    let [_, middle, _] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height + 1), Constraint::Fill(1)]).areas(area);
    let text = Paragraph::new(lines).alignment(Alignment::Center).wrap(Wrap { trim: true });
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
    let state = match p.phase {
        Phase::Ended => "Ended",
        Phase::Loading => "Loading…",
        _ if p.buffering => "Buffering…",
        Phase::Playing if p.paused => "Paused",
        Phase::Playing => "Playing",
        Phase::Resolving | Phase::Waiting { .. } | Phase::Failed(_) => "",
    };
    let volume = if p.muted { "muted".to_string() } else { format!("vol {}%", p.volume) };
    let key = |a| app.keymap.player.first_key(a).map(|k| k.to_string()).unwrap_or_default();
    let mut right = vec![Span::from(key(PlayerAction::Back)).bold(), Span::from(" back")];
    if p.is_running() {
        right.splice(0..0, [Span::from(format!("{state}  {volume}  ")).dim()]);
        right.extend([Span::from("  "), Span::from(key(PlayerAction::TogglePause)).bold(), Span::from(" pause")]);
    }
    let right = Line::from(right);
    let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX).saturating_add(1).min(area.width);
    let [title, hints] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    let width = usize::from(title.width).saturating_sub(1);
    f.render_widget(Line::from(format!(" {}", truncate(&p.title, width))).bold(), title);
    f.render_widget(right, hints);
}
