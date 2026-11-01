use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use reeltty::app::App;
use reeltty::app::thumbnails::Thumbnails;
use reeltty::config::{Config, Paths};
use reeltty::frontend::Frontend as _;
use reeltty::frontend::terminal::TerminalFrontend;
use reeltty::runtime::Services;
use reeltty::source::innertube::InnerTube;
use reeltty::source::ytdlp::YtDlp;
use reeltty::source::{Fallback, Source};
use reeltty::store::Store;
use reeltty::takeout;
use reeltty::thumbnails::{self, Fetcher};

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
    /// Import subscriptions from Google Takeout: the subscriptions.csv file, or
    /// an unpacked Takeout folder.
    Import {
        /// subscriptions.csv, or a folder that holds it.
        path: PathBuf,
    },
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
        Some(Command::Import { path }) => import(&paths, &path),
        None => run_tui(&paths),
    }
}

fn import(paths: &Paths, path: &std::path::Path) -> Result<()> {
    let store = Store::open(&paths.database)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let s = takeout::import(&store, path, now)?;
    let (added, already) = (takeout::channels(s.added), takeout::channels(s.already));
    let were = if s.already == 1 { "was" } else { "were" };
    println!("Added {added}. {already} {were} there already.");
    if s.skipped > 0 {
        println!(
            "Skipped {} that are not channels.",
            if s.skipped == 1 { "1 row".into() } else { format!("{} rows", s.skipped) }
        );
    }
    Ok(())
}

fn run_tui(paths: &Paths) -> Result<()> {
    let _log = init_logging(paths)?;
    let (config, _) = Config::load(&paths.config_file)?;
    let keymap = config.keymap()?;
    let store = Store::open(&paths.database)?;
    let subscriptions = store.subscriptions()?;
    let saved_playlists = store.saved_playlists()?;
    let history = store.history()?;
    let (feed, fetched) = store.feed()?;

    let ytdlp = YtDlp { program: config.yt_dlp_program() };
    let source: Arc<dyn Source> = match InnerTube::new() {
        Ok(innertube) => Arc::new(Fallback { primary: innertube, secondary: ytdlp.clone() }),
        Err(e) => {
            tracing::warn!("cannot start the InnerTube client, using yt-dlp only: {e}");
            Arc::new(ytdlp.clone())
        }
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut frontend = TerminalFrontend::start();
        let mut app = App::new(keymap, frontend.graphics().clone());
        app.subscriptions = subscriptions;
        app.captions_language.clone_from(&config.captions.language);
        app.captions_on = config.captions.on;
        app.max_quality = config.max_quality.height();
        app.autoplay = config.autoplay.0;
        let sponsorblock = start_sponsorblock(config.sponsorblock.skipped());
        app.sponsorblock = sponsorblock.is_some();
        app.saved_playlists = saved_playlists;
        app.history = history;
        let filter = reeltty::app::feed::FeedFilter { shorts: config.feed.shorts, live: config.feed.live };
        app.feed = reeltty::app::feed::FeedState::new(feed, fetched, filter);
        let thumbnails = (config.thumbnails.0 && app.graphics.is_available())
            .then(|| start_thumbnails(&paths.thumbnail_cache))
            .flatten();
        app.thumbnails = Thumbnails::new(thumbnails.is_some());
        let services = Services { source, store, thumbnails, ytdlp, sponsorblock };
        reeltty::runtime::run(&mut app, &mut frontend, services).await
    })
}

/// The SponsorBlock client, or `None` when no category is skipped.
fn start_sponsorblock(categories: &[reeltty::sponsorblock::Category]) -> Option<reeltty::sponsorblock::Client> {
    if categories.is_empty() {
        return None;
    }
    reeltty::sponsorblock::Client::new(categories.to_vec())
        .inspect_err(|e| tracing::warn!("SponsorBlock is off: {e:#}"))
        .ok()
}

/// The thumbnail downloader. Old cache files are removed in the background.
fn start_thumbnails(cache: &std::path::Path) -> Option<Fetcher> {
    let dir = cache.to_path_buf();
    std::thread::spawn(move || match thumbnails::prune(&dir, thumbnails::MAX_CACHE_BYTES) {
        Ok(0) => {}
        Ok(n) => tracing::info!("removed {n} old thumbnails from the cache"),
        Err(e) => tracing::debug!("thumbnail cache not pruned: {e:#}"),
    });
    Fetcher::new(cache.to_path_buf()).inspect_err(|e| tracing::warn!("thumbnails are off: {e:#}")).ok()
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
