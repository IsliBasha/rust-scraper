use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, watch};
use tokio::time::interval;

use crate::event::ScrapeEvent;
use crate::snapshot::MetricsSnapshot;

const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(250);
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Shared counters incremented by worker tasks. All accesses are atomic.
#[derive(Default)]
struct Counters {
    pending: AtomicU64,
    in_progress: AtomicU64,
    done: AtomicU64,
    failed: AtomicU64,
    skipped: AtomicU64,
    bytes: AtomicU64,
    requests: AtomicU64,
    active_workers: AtomicU64,
}

/// Central metrics hub. Clone cheaply — all state lives behind an `Arc`.
#[derive(Clone)]
pub struct MetricsHub {
    counters: Arc<Counters>,
    event_tx: broadcast::Sender<ScrapeEvent>,
    snapshot_rx: watch::Receiver<MetricsSnapshot>,
    // Held to keep the snapshot task alive.
    _snapshot_tx: Arc<watch::Sender<MetricsSnapshot>>,
}

impl MetricsHub {
    pub fn new() -> Self {
        let counters = Arc::new(Counters::default());
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (snapshot_tx, snapshot_rx) = watch::channel(MetricsSnapshot::default());
        let snapshot_tx = Arc::new(snapshot_tx);

        // Background task: re-sample counters every 250 ms.
        let c = Arc::clone(&counters);
        let tx = Arc::clone(&snapshot_tx);
        let start = Instant::now();
        let mut req_prev = 0u64;
        let mut prev_time = Instant::now();

        tokio::spawn(async move {
            let mut ticker = interval(SNAPSHOT_INTERVAL);
            loop {
                ticker.tick().await;
                let now = Instant::now();
                let elapsed_secs = now.duration_since(prev_time).as_secs_f64().max(0.001);

                let req_now = c.requests.load(Ordering::Relaxed);
                let rps = (req_now - req_prev) as f64 / elapsed_secs;
                req_prev = req_now;
                prev_time = now;
                let _ = start; // suppress unused warning

                let snap = MetricsSnapshot {
                    urls_pending: c.pending.load(Ordering::Relaxed),
                    urls_in_progress: c.in_progress.load(Ordering::Relaxed),
                    urls_done: c.done.load(Ordering::Relaxed),
                    urls_failed: c.failed.load(Ordering::Relaxed),
                    urls_skipped: c.skipped.load(Ordering::Relaxed),
                    bytes_downloaded: c.bytes.load(Ordering::Relaxed),
                    requests_per_second: rps,
                    active_workers: c.active_workers.load(Ordering::Relaxed) as u32,
                };

                if tx.send(snap).is_err() {
                    break;
                }
            }
        });

        Self {
            counters,
            event_tx,
            snapshot_rx,
            _snapshot_tx: snapshot_tx,
        }
    }

    // --- Counter mutations ---

    pub fn inc_pending(&self, n: u64) {
        self.counters.pending.fetch_add(n, Ordering::Relaxed);
    }

    pub fn dec_pending(&self) {
        self.counters.pending.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn inc_in_progress(&self) {
        self.counters.in_progress.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_in_progress(&self) {
        self.counters.in_progress.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn inc_done(&self) {
        self.counters.done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_failed(&self) {
        self.counters.failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_skipped(&self) {
        self.counters.skipped.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_bytes(&self, n: u64) {
        self.counters.bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub fn inc_requests(&self) {
        self.counters.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_active_workers(&self, n: u32) {
        self.counters.active_workers.store(n as u64, Ordering::Relaxed);
    }

    // --- Event broadcasting ---

    pub fn emit(&self, event: ScrapeEvent) {
        // send() only fails if there are no receivers; that's fine at startup/shutdown.
        let _ = self.event_tx.send(event);
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ScrapeEvent> {
        self.event_tx.subscribe()
    }

    // --- Snapshot ---

    pub fn snapshot(&self) -> MetricsSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    pub fn watch_snapshots(&self) -> watch::Receiver<MetricsSnapshot> {
        self.snapshot_rx.clone()
    }

    /// Returns a `ControlHandle` that can pause/resume/stop the crawl.
    pub fn control_handle(&self) -> ControlHandle {
        ControlHandle {
            event_tx: self.event_tx.clone(),
        }
    }
}

impl Default for MetricsHub {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle passed to external controllers (TUI, dashboard, CLI) to send control events.
#[derive(Clone)]
pub struct ControlHandle {
    event_tx: broadcast::Sender<ScrapeEvent>,
}

impl ControlHandle {
    pub fn shutdown(&self) {
        let _ = self.event_tx.send(ScrapeEvent::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counters_increment_correctly() {
        let hub = MetricsHub::new();
        hub.inc_pending(5);
        hub.inc_in_progress();
        hub.inc_done();
        hub.inc_failed();
        hub.inc_skipped();
        hub.add_bytes(1024);

        // Give the snapshot task one tick.
        tokio::time::sleep(SNAPSHOT_INTERVAL * 2).await;

        let snap = hub.snapshot();
        assert_eq!(snap.urls_pending, 5);
        assert_eq!(snap.urls_in_progress, 1);
        assert_eq!(snap.urls_done, 1);
        assert_eq!(snap.urls_failed, 1);
        assert_eq!(snap.urls_skipped, 1);
        assert_eq!(snap.bytes_downloaded, 1024);
    }

    #[tokio::test]
    async fn events_reach_subscriber() {
        let hub = MetricsHub::new();
        let mut rx = hub.subscribe_events();

        hub.emit(ScrapeEvent::UrlDiscovered {
            url: "https://example.com".into(),
            depth: 0,
        });

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, ScrapeEvent::UrlDiscovered { .. }));
    }

    #[tokio::test]
    async fn control_handle_emits_shutdown() {
        let hub = MetricsHub::new();
        let mut rx = hub.subscribe_events();
        let ctrl = hub.control_handle();

        ctrl.shutdown();

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, ScrapeEvent::Shutdown));
    }
}
