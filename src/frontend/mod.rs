//! Frontends: where input comes from and where the UI goes.
//!
//! Terminal mode is the only frontend for now. A native window can come
//! later (see "Window mode, later" in `DESIGN.md`) and uses the same `App`.

pub mod terminal;

use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{App, Message};
use crate::terminal::detect::Graphics;

pub trait Frontend {
    /// Starts to send input to the app as `Message::Input`.
    fn start_input(&mut self, tx: UnboundedSender<Message>);

    /// Draws the whole UI for the current state.
    fn draw(&mut self, app: &App) -> Result<()>;

    /// What this frontend can show for video and images.
    fn graphics(&self) -> &Graphics;
}
