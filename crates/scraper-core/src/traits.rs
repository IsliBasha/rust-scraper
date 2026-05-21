use async_trait::async_trait;

use crate::{
    CrawlError, CrawlJob, ExtractedData, ExtractionRule, FetchResponse, PersistedUrl,
    RenderOptions, SelectorKind, Url, UrlId, UrlStatus,
};

/// Fetches a URL over HTTP. Implementations own rate limiting, retry, and proxy logic.
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, job: &CrawlJob) -> Result<FetchResponse, CrawlError>;

    /// Returns false if the fetcher is temporarily unavailable (e.g. offline mode).
    async fn ready(&self) -> bool {
        true
    }
}

/// Renders a URL using a headless browser; returns the post-JS-execution DOM as HTML bytes.
#[async_trait]
pub trait BrowserBackend: Send + Sync {
    async fn render(&self, url: &Url, opts: &RenderOptions) -> Result<FetchResponse, CrawlError>;

    /// Gracefully shut down the browser process. Called once on engine shutdown.
    async fn shutdown(&self) -> Result<(), CrawlError>;
}

/// Applies extraction rules to raw HTML. Synchronous and CPU-bound; runs via `spawn_blocking`.
pub trait SelectorEngine: Send + Sync {
    fn extract(
        &self,
        html: &str,
        base_url: &Url,
        rules: &[ExtractionRule],
    ) -> Result<ExtractedData, CrawlError>;

    /// Returns true if this engine understands the given selector syntax.
    fn supports(&self, kind: &SelectorKind) -> bool;
}

/// Persists crawl frontier state so sessions can be resumed after a crash or restart.
#[async_trait]
pub trait StateStore: Send + Sync {
    /// Load all URLs not in a terminal state. Called once on startup/resume.
    async fn load_pending(&self) -> Result<Vec<PersistedUrl>, CrawlError>;

    /// Record a newly discovered URL. Idempotent on the normalized URL (INSERT OR IGNORE).
    async fn enqueue(
        &self,
        url: &Url,
        depth: u32,
        parent: Option<UrlId>,
    ) -> Result<UrlId, CrawlError>;

    /// Transition a URL to a new status.
    async fn set_status(&self, id: UrlId, status: UrlStatus) -> Result<(), CrawlError>;

    /// Mark a URL as successfully crawled.
    async fn mark_done(&self, id: UrlId) -> Result<(), CrawlError>;

    /// Mark a URL as failed. `retryable` controls whether the coordinator may re-enqueue it.
    async fn mark_failed(&self, id: UrlId, error: &str, retryable: bool) -> Result<(), CrawlError>;

    /// Crash-recovery: reset any rows stuck InProgress back to Pending.
    async fn reclaim_in_flight(&self) -> Result<u64, CrawlError>;
}

/// Writes extracted data to an output sink (JSON Lines file, SQLite table, stdout, etc.).
#[async_trait]
pub trait ResultSink: Send + Sync {
    async fn write(&self, data: &ExtractedData) -> Result<(), CrawlError>;

    /// Flush buffered records and finalize output. Called once on shutdown.
    async fn flush(&self) -> Result<(), CrawlError>;
}
