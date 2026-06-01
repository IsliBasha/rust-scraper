use std::sync::Arc;

use scraper_core::{CrawlError, Fetcher, ResultSink, RobotsChecker, SelectorEngine, StateStore};
use scraper_metrics::MetricsHub;
use tracing::info;

use crate::coordinator::Coordinator;

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
}

impl Engine {
    pub async fn run(self) -> Result<(), CrawlError> {
        info!(
            concurrency = self.concurrency,
            max_depth = self.max_depth,
            allowed_domains = ?self.allowed_domains,
            "engine starting"
        );
        let shutdown_rx = self.metrics.subscribe_events();

        let coordinator = Coordinator {
            state: Arc::clone(&self.state),
            sink: Arc::clone(&self.sink),
            selector: Arc::clone(&self.selector),
            metrics: self.metrics.clone(),
            max_depth: self.max_depth,
            allowed_domains: self.allowed_domains,
            concurrency: self.concurrency,
            robots: self.robots,
        };

        coordinator.run(self.fetcher, shutdown_rx).await
    }
}
