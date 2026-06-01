use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::{Parser, Subcommand};
use scraper_browser::StubBrowser;
use scraper_config::load;
use scraper_core::StateStore;
use scraper_engine::Engine;
use scraper_extractor::CompositeEngine;
use scraper_fetch_http::HttpFetcher;
use scraper_metrics::MetricsHub;
use scraper_robots::RobotsCache;
use scraper_config::OutputConfig;
use scraper_core::ResultSink;
use scraper_storage::{JsonLinesSink, SqliteResultSink, SqliteStateStore};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser)]
#[command(name = "rscrape", version, about = "Production-grade Rust web scraper")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a crawl session.
    Crawl {
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1:8080")]
        dashboard: Option<SocketAddr>,
        #[arg(long)]
        tui: bool,
        #[arg(long, default_value = "scraper.db")]
        db: PathBuf,
        seeds: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Crawl {
            config,
            dashboard,
            tui,
            db,
            seeds,
        } => {
            let mut cfg = load(config.as_deref()).context("failed to load config")?;
            if !seeds.is_empty() {
                cfg.seeds = seeds;
            }
            anyhow::ensure!(!cfg.seeds.is_empty(), "no seed URLs provided");

            let db_path = db.to_string_lossy().into_owned();
            let state =
                Arc::new(SqliteStateStore::open(&db_path).context("failed to open state DB")?);

            let sink: Arc<dyn ResultSink> = match &cfg.output {
                OutputConfig::JsonLines { path } => Arc::new(
                    JsonLinesSink::create(path).context("failed to open JSON Lines output")?,
                ),
                OutputConfig::Sqlite { path } => Arc::new(
                    SqliteResultSink::open(path).context("failed to open results DB")?,
                ),
                OutputConfig::Stdout => {
                    Arc::new(SqliteResultSink::open(&db_path).context("failed to open results DB")?)
                }
            };

            for seed in &cfg.seeds {
                let url = scraper_core::Url::parse(seed)
                    .with_context(|| format!("invalid seed URL: {seed}"))?;
                state.enqueue(&url, 0, None).await?;
            }

            let metrics = MetricsHub::new();

            let fetcher = Arc::new(
                HttpFetcher::new(
                    cfg.rate_limit.requests_per_second,
                    cfg.rate_limit.burst,
                    16 * 1024 * 1024,
                )
                .context("failed to build HTTP fetcher")?,
            );

            let selector = Arc::new(CompositeEngine::new());
            let _browser = Arc::new(StubBrowser);

            if let Some(addr) = dashboard {
                let m = metrics.clone();
                tokio::spawn(async move {
                    if let Err(e) = scraper_dashboard::serve(m, addr).await {
                        tracing::error!("dashboard error: {e}");
                    }
                });
            }

            let tui_handle = if tui {
                let m = metrics.clone();
                Some(tokio::spawn(async move {
                    let app = scraper_tui::TuiApp::new(m);
                    if let Err(e) = app.run().await {
                        tracing::error!("TUI error: {e}");
                    }
                }))
            } else {
                None
            };

            let engine = Engine {
                fetcher,
                selector,
                state,
                sink,
                metrics,
                max_depth: cfg.max_depth,
                allowed_domains: cfg.allowed_domains.clone(),
                concurrency: cfg.concurrency,
                robots: Some(Arc::new(
                    RobotsCache::new("rust-scraper").context("failed to build robots cache")?,
                )),
                extraction_config: cfg.extraction.clone(),
            };

            engine.run().await.context("crawl engine error")?;

            if let Some(h) = tui_handle {
                h.await.ok();
            }

            println!("crawl complete.");
        }
    }

    Ok(())
}
