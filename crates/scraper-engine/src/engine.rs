use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use scraper_config::ExtractionConfig;
use scraper_core::{CrawlError, DeltaTracker, Fetcher, ResultSink, RobotsChecker, SelectorEngine, StateStore};
use scraper_metrics::MetricsHub;
use tracing::info;

use crate::coordinator::Coordinator;

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// High-level facade that wires together all components and starts the crawl.
pub struct Engine {
    pub fetcher: Arc<dyn Fetcher>,
    pub selector: Arc<dyn SelectorEngine>,
    pub state: Arc<dyn StateStore>,
    pub sink: Arc<dyn ResultSink>,
    pub metrics: MetricsHub,
    pub max_depth: u32,
    pub allowed_domains: Vec<String>,
    pub concurrency: usize,
    /// Optional robots.txt checker. `None` disables robots.txt enforcement.
    pub robots: Option<Arc<dyn RobotsChecker>>,
    /// URL-pattern-driven extraction rules. Empty config means no config-driven rules.
    pub extraction_config: ExtractionConfig,
    /// Re-crawl interval. `None` runs one-shot; `Some(d)` loops with that delay between runs.
    pub interval: Option<Duration>,
    /// Optional delta tracker for content-hash comparison across crawl runs.
    pub delta_store: Option<Arc<dyn DeltaTracker>>,
}

impl Engine {
    pub async fn run(self) -> Result<(), CrawlError> {
        let Engine {
            fetcher,
            selector,
            state,
            sink,
            metrics,
            max_depth,
            allowed_domains,
            concurrency,
            robots,
            extraction_config,
            interval,
            delta_store,
        } = self;

        loop {
            let crawl_start = unix_now();

            info!(
                concurrency,
                max_depth,
                allowed_domains = ?allowed_domains,
                "engine starting crawl run"
            );

            let shutdown_rx = metrics.subscribe_events();
            let coordinator = Coordinator {
                state: Arc::clone(&state),
                sink: Arc::clone(&sink),
                selector: Arc::clone(&selector),
                metrics: metrics.clone(),
                max_depth,
                allowed_domains: allowed_domains.clone(),
                concurrency,
                robots: robots.clone(),
                extraction_config: extraction_config.clone(),
                delta_store: delta_store.clone(),
            };

            coordinator.run(Arc::clone(&fetcher), shutdown_rx).await?;

            // After each run, detect URLs from previous crawls that are now gone.
            if let Some(ds) = &delta_store {
                let removed = ds.detect_removed(crawl_start).await?;
                if !removed.is_empty() {
                    info!(count = removed.len(), "detected removed URLs");
                }
            }

            match interval {
                None => break,
                Some(dur) => {
                    info!(
                        interval_secs = dur.as_secs(),
                        "crawl complete — sleeping until next scheduled run"
                    );
                    state.reset_for_recrawl().await?;
                    tokio::time::sleep(dur).await;
                }
            }
        }

        Ok(())
    }
}
