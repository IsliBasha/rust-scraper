use serde::{Deserialize, Serialize};

/// Discrete crawl events broadcast to subscribers (TUI, dashboard, logger).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScrapeEvent {
    UrlDiscovered { url: String, depth: u32 },
    UrlStarted { url: String },
    UrlDone { url: String, status: u16, bytes: u64, elapsed_ms: u64 },
    UrlFailed { url: String, error: String, retryable: bool },
    UrlSkipped { url: String, reason: String },
    RateLimited { host: String, delay_ms: u64 },
    BrowserFallback { url: String },
    Shutdown,
}
