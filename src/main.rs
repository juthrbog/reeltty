use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use reeltty::app::App;
use reeltty::config::{Config, Paths};
use reeltty::frontend::Frontend as _;
use reeltty::frontend::terminal::TerminalFrontend;
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
    let _store = Store::open(&paths.database)?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut frontend = TerminalFrontend::start();
        let mut app = App::new(keymap, frontend.graphics().clone());
        reeltty::runtime::run(&mut app, &mut frontend).await
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
