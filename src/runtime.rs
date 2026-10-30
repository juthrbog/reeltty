//! The event loop: messages in, effects out.

use std::ops::ControlFlow;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::app::{App, Effect, Message};
use crate::frontend::Frontend;
use crate::source::Source;

/// Runs until an `Effect::Quit`. Messages that arrive together are applied
/// before the next draw, so a burst of input gives one frame.
pub async fn run(app: &mut App, frontend: &mut impl Frontend, source: Arc<dyn Source>) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    frontend.start_input(tx.clone());
    watch_signals(tx.clone());
    frontend.draw(app)?;

    while let Some(msg) = rx.recv().await {
        let mut next = Some(msg);
        while let Some(msg) = next {
            for effect in app.update(msg) {
                if let ControlFlow::Break(result) = handle(effect, &tx, &source) {
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

/// Carries out one effect. Slow work runs in a task that sends a `Message`
/// when it is done. `Break` ends the loop with its result.
fn handle(effect: Effect, tx: &UnboundedSender<Message>, source: &Arc<dyn Source>) -> ControlFlow<Result<()>> {
    match effect {
        Effect::Quit => return ControlFlow::Break(Ok(())),
        Effect::Fail(why) => return ControlFlow::Break(Err(anyhow!(why))),
        Effect::Search { generation, query } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.search(&query).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::SearchResult { generation, append: false, result });
            });
        }
        Effect::SearchMore { generation, continuation } => {
            let (tx, source) = (tx.clone(), Arc::clone(source));
            tokio::spawn(async move {
                let result = source.search_more(&continuation).await.map_err(|e| e.to_string());
                let _ = tx.send(Message::SearchResult { generation, append: true, result });
            });
        }
    }
    ControlFlow::Continue(())
}
