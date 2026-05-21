use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream;
use scraper_metrics::{MetricsHub, MetricsSnapshot};
use serde_json::json;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::WatchStream;
use tokio_stream::StreamExt;

/// SSE endpoint: sends a `metrics` event every time the snapshot changes.
pub async fn metrics_sse(
    metrics: MetricsHub,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let watch_rx = metrics.watch_snapshots();
    let stream = WatchStream::new(watch_rx).map(|snap: MetricsSnapshot| {
        let data = json!({
            "pending":     snap.urls_pending,
            "in_progress": snap.urls_in_progress,
            "done":        snap.urls_done,
            "failed":      snap.urls_failed,
            "skipped":     snap.urls_skipped,
            "bytes":       snap.bytes_downloaded,
            "rps":         snap.requests_per_second,
            "workers":     snap.active_workers,
        });
        Ok::<Event, Infallible>(Event::default().event("metrics").data(data.to_string()))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
