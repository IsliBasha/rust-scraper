use serde::{Deserialize, Serialize};

/// Point-in-time snapshot of crawl gauges, emitted every 250 ms.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub urls_pending: u64,
    pub urls_in_progress: u64,
    pub urls_done: u64,
    pub urls_failed: u64,
    pub urls_skipped: u64,
    pub bytes_downloaded: u64,
    pub requests_per_second: f64,
    pub active_workers: u32,
}
