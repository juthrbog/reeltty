use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use reeltty::app::App;
use reeltty::config::{Config, Paths};
use reeltty::frontend::Frontend as _;
use reeltty::frontend::terminal::TerminalFrontend;
use reeltty::runtime::Services;
use reeltty::source::innertube::InnerTube;
use reeltty::source::ytdlp::YtDlp;
use reeltty::source::{Fallback, Source};
use reeltty::store::Store;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Use this configuration file.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Check the programs and libraries that reeltty needs.
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut paths = Paths::from_env()?;
    if let Some(c) = cli.config {
        paths.config_file = c;
    }

    match cli.command {
        Some(Command::Doctor) => {
            let ok = reeltty::doctor::run(&paths);
            std::process::exit(i32::from(!ok));
        }
        None => run_tui(&paths),
    }
}

fn run_tui(paths: &Paths) -> Result<()> {
    let _log = init_logging(paths)?;
    let (config, _) = Config::load(&paths.config_file)?;
    let keymap = config.keymap()?;
    let store = Store::open(&paths.database)?;
    let subscriptions = store.subscriptions()?;
    let saved_playlists = store.saved_playlists()?;

    let ytdlp = YtDlp { program: config.yt_dlp_program() };
    let source: Arc<dyn Source> = match InnerTube::new() {
        Ok(innertube) => Arc::new(Fallback { primary: innertube, secondary: ytdlp }),
        Err(e) => {
            tracing::warn!("cannot start the InnerTube client, using yt-dlp only: {e}");
            Arc::new(ytdlp)
        }
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut frontend = TerminalFrontend::start();
        let mut app = App::new(keymap, frontend.graphics().clone());
        app.subscriptions = subscriptions;
        app.saved_playlists = saved_playlists;
        let services = Services { source, store };
        reeltty::runtime::run(&mut app, &mut frontend, services).await
    })
}

/// Logs go to a file, because the UI owns the terminal. `REELTTY_LOG` sets the filter.
fn init_logging(paths: &Paths) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let dir = paths.log_file.parent().context("log file has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let file =
        std::fs::File::create(&paths.log_file).with_context(|| format!("create {}", paths.log_file.display()))?;
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_env("REELTTY_LOG").unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    Ok(guard)
}
