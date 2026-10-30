//! The event loop: messages in, effects out.

use std::ops::ControlFlow;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::app::{App, Effect, Message};
use crate::frontend::Frontend;

/// Runs until an `Effect::Quit`. Messages that arrive together are applied
/// before the next draw, so a burst of input gives one frame.
pub async fn run(app: &mut App, frontend: &mut impl Frontend) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    frontend.start_input(tx.clone());
    watch_signals(tx);
    frontend.draw(app)?;

    while let Some(msg) = rx.recv().await {
        let mut next = Some(msg);
        while let Some(msg) = next {
            for effect in app.update(msg) {
                if let ControlFlow::Break(result) = handle(effect) {
                    return result;
                }
            }
            next = rx.try_recv().ok();
        }
        frontend.draw(app)?;
    }
    Err(anyhow!("all message senders stopped"))
}

/// Turns SIGTERM and SIGHUP into `Message::Terminate`, so the terminal is
/// restored on the normal exit path.
fn watch_signals(tx: UnboundedSender<Message>) {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async move {
        let (Ok(mut term), Ok(mut hup)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup())) else {
            tracing::warn!("cannot watch for signals");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
        let _ = tx.send(Message::Terminate);
    });
}

/// Carries out one effect. `Break` ends the loop with its result.
fn handle(effect: Effect) -> ControlFlow<Result<()>> {
    match effect {
        Effect::Quit => ControlFlow::Break(Ok(())),
        Effect::Fail(why) => ControlFlow::Break(Err(anyhow!(why))),
    }
}
