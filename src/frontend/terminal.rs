//! Terminal mode: ratatui for the UI, crossterm for input.

use std::io::Write as _;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt as _;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::UnboundedSender;

use super::Frontend;
use crate::app::{App, Input, Message};
use crate::keymap::{KeyCode, KeyPress};
use crate::terminal::detect::{self, Graphics};
use crate::terminal::kitty::Uploads;

pub struct TerminalFrontend {
    terminal: DefaultTerminal,
    graphics: Graphics,
    uploads: Uploads,
}

impl TerminalFrontend {
    /// Detects the graphics support, then takes over the terminal.
    /// The terminal is restored on drop and on panic.
    pub fn start() -> Self {
        let graphics = detect::detect();
        tracing::info!(?graphics, "terminal graphics");
        let terminal = ratatui::init();
        let uploads = Uploads::default();
        // ratatui's panic hook leaves the alternate screen. Images live on the
        // alternate screen, so they must be deleted before that.
        let cleanup = uploads.panic_cleanup();
        let restore = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            cleanup();
            restore(info);
        }));
        Self { terminal, graphics, uploads }
    }
}

impl Drop for TerminalFrontend {
    fn drop(&mut self) {
        let mut out = String::new();
        self.uploads.clear(&mut out);
        let backend = self.terminal.backend_mut();
        let _ = backend.write_all(out.as_bytes()).and_then(|()| backend.flush());
        ratatui::restore();
    }
}

impl Frontend for TerminalFrontend {
    fn start_input(&mut self, tx: UnboundedSender<Message>) {
        if let Ok(size) = self.terminal.size() {
            let _ = tx.send(Message::Input(Input::Resize { width: size.width, height: size.height }));
        }
        send_cell_size(&tx);
        tokio::spawn(async move {
            let mut events = EventStream::new();
            let why = loop {
                let input = match events.next().await {
                    Some(Ok(Event::Key(k))) => key_press(k).map(Input::Key),
                    Some(Ok(Event::Resize(width, height))) => {
                        // A font size change also gives a resize.
                        send_cell_size(&tx);
                        Some(Input::Resize { width, height })
                    }
                    Some(Ok(_)) => None,
                    Some(Err(e)) => break e.to_string(),
                    None => break "end of input".to_string(),
                };
                if let Some(input) = input
                    && tx.send(Message::Input(input)).is_err()
                {
                    return;
                }
            };
            tracing::error!("terminal input stopped: {why}");
            let _ = tx.send(Message::InputClosed(why));
        });
    }

    fn draw(&mut self, app: &App) -> Result<()> {
        // Images go out before the cells that show them. Both go through the
        // same writer, so the escapes never mix.
        let mut out = String::new();
        self.uploads.sync(app.thumbnails.ready(), &mut out);
        if !out.is_empty() {
            self.terminal.backend_mut().write_all(out.as_bytes())?;
        }
        self.terminal.draw(|f| crate::ui::view(app, f))?;
        Ok(())
    }

    fn graphics(&self) -> &Graphics {
        &self.graphics
    }
}

/// Sends the pixel size of one cell, if the terminal reports it.
fn send_cell_size(tx: &UnboundedSender<Message>) {
    let Ok(size) = crossterm::terminal::window_size() else { return };
    if size.columns == 0 || size.rows == 0 {
        return;
    }
    let (width, height) = (size.width / size.columns, size.height / size.rows);
    let _ = tx.send(Message::Input(Input::CellSize { width, height }));
}

fn key_press(k: KeyEvent) -> Option<KeyPress> {
    use crossterm::event::KeyCode as C;

    if k.kind == KeyEventKind::Release {
        return None;
    }
    let code = match k.code {
        C::Char(c) => KeyCode::Char(c),
        C::Enter => KeyCode::Enter,
        C::Esc => KeyCode::Esc,
        C::Tab => KeyCode::Tab,
        C::BackTab => KeyCode::BackTab,
        C::Backspace => KeyCode::Backspace,
        C::Up => KeyCode::Up,
        C::Down => KeyCode::Down,
        C::Left => KeyCode::Left,
        C::Right => KeyCode::Right,
        C::PageUp => KeyCode::PageUp,
        C::PageDown => KeyCode::PageDown,
        C::Home => KeyCode::Home,
        C::End => KeyCode::End,
        C::F(n) => KeyCode::F(n),
        _ => return None,
    };
    Some(KeyPress {
        code,
        ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
        alt: k.modifiers.contains(KeyModifiers::ALT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode as C;

    #[test]
    fn maps_keys() {
        let k = |code, mods| key_press(KeyEvent::new(code, mods));
        assert_eq!(k(C::Char('G'), KeyModifiers::SHIFT), Some("G".parse().unwrap()));
        assert_eq!(k(C::Char('c'), KeyModifiers::CONTROL), Some("ctrl-c".parse().unwrap()));
        assert_eq!(k(C::BackTab, KeyModifiers::SHIFT), Some("shift-tab".parse().unwrap()));
        assert_eq!(k(C::Null, KeyModifiers::NONE), None);
    }
}
