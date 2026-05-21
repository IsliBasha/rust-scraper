use std::collections::HashSet;
use std::sync::Arc;

use scraper_core::{
    CrawlError, CrawlJob, ExtractedData, Fetcher, ResultSink, SelectorEngine, StateStore, Url,
    UrlId, UrlStatus,
};
use scraper_metrics::{MetricsHub, ScrapeEvent};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::worker::WorkerResult;

/// Message sent from the coordinator to a worker.
pub enum CoordMsg {
    Job(CrawlJob),
    Shutdown,
}

/// Runs the crawl coordinator loop.
///
/// The coordinator owns the in-flight URL set and orchestrates job dispatch +
/// result processing. Workers live outside and communicate only via channels.
pub struct Coordinator {
    pub state: Arc<dyn StateStore>,
    pub sink: Arc<dyn ResultSink>,
    pub selector: Arc<dyn SelectorEngine>,
    pub metrics: MetricsHub,
    pub max_depth: u32,
    pub concurrency: usize,
}

impl Coordinator {
    /// Drives the crawl to completion (or until a `Shutdown` event is received).
    pub async fn run(
        self,
        fetcher: Arc<dyn Fetcher>,
        shutdown_rx: tokio::sync::broadcast::Receiver<ScrapeEvent>,
    ) -> Result<(), CrawlError> {
        // Bounded MPMC job channel — each worker clones rx and calls recv() directly.
        // Buffer sized generously to prevent coordinator stalling during bursty link discovery.
        let (job_tx, job_rx) = async_channel::bounded::<CoordMsg>(self.concurrency * 16);
        let (result_tx, mut result_rx) = mpsc::channel::<WorkerResult>(self.concurrency * 8);

        // Spawn worker pool.
        for _ in 0..self.concurrency {
            let jrx = job_rx.clone();
            let rtx = result_tx.clone();
            let f = Arc::clone(&fetcher);
            tokio::spawn(async move {
                crate::worker::run_worker(jrx, rtx, f).await;
            });
        }
        drop(result_tx); // coordinator holds the only remaining sender

        // Load pending URLs from state store (resume support).
        let pending = self.state.load_pending().await?;
        let reclaimed = self.state.reclaim_in_flight().await?;
        if reclaimed > 0 {
            info!(reclaimed, "reclaimed in-flight URLs from previous session");
        }

        let mut in_flight: HashSet<UrlId> = HashSet::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Prime the queue from persisted state.
        for p in pending {
            seen.insert(p.url.as_str().to_string());
            if in_flight.len() < self.concurrency {
                self.dispatch(&job_tx, &mut in_flight, &p.url, p.id, p.depth)
                    .await?;
            }
        }

        let mut shutdown_rx = shutdown_rx;

        loop {
            tokio::select! {
                // Worker result arrived.
                Some(result) = result_rx.recv() => {
                    self.handle_result(result, &job_tx, &mut in_flight, &mut seen).await?;
                }
                // External shutdown signal.
                Ok(ScrapeEvent::Shutdown) = shutdown_rx.recv() => {
                    info!("shutdown signal received — stopping coordinator");
                    job_tx.close(); // signals all workers to stop via Err(RecvError)
                    break;
                }
                // All workers done and no pending URLs.
                else => {
                    if in_flight.is_empty() {
                        info!("crawl complete — all URLs processed");
                        job_tx.close();
                        break;
                    }
                }
            }
        }

        self.sink.flush().await?;
        Ok(())
    }

    async fn dispatch(
        &self,
        job_tx: &async_channel::Sender<CoordMsg>,
        in_flight: &mut HashSet<UrlId>,
        url: &Url,
        id: UrlId,
        depth: u32,
    ) -> Result<(), CrawlError> {
        self.state.set_status(id, UrlStatus::InProgress).await?;
        self.metrics.inc_in_progress();
        self.metrics.dec_pending();
        self.metrics
            .emit(ScrapeEvent::UrlStarted { url: url.as_str().to_string() });

        in_flight.insert(id);
        job_tx
            .send(CoordMsg::Job(CrawlJob {
                id,
                url: url.clone(),
                depth,
                parent: None,
                attempt: 0,
                force_browser: false,
            }))
            .await
            .map_err(|_| CrawlError::Shutdown)?;
        Ok(())
    }

    async fn handle_result(
        &self,
        result: WorkerResult,
        job_tx: &async_channel::Sender<CoordMsg>,
        in_flight: &mut HashSet<UrlId>,
        seen: &mut HashSet<String>,
    ) -> Result<(), CrawlError> {
        in_flight.remove(&result.job_id);
        self.metrics.dec_in_progress();
        self.metrics.inc_requests();

        match result.outcome {
            Ok(data) => {
                self.state.mark_done(result.job_id).await?;
                self.metrics.inc_done();
                self.metrics.emit(ScrapeEvent::UrlDone {
                    url: data.url.as_str().to_string(),
                    status: 200,
                    bytes: 0,
                    elapsed_ms: 0,
                });

                // Run selector extraction.
                let extracted =
                    self.selector.extract(&data.html, &data.url, &data.rules)?;

                // Persist extracted data.
                let extracted_data = ExtractedData {
                    url: extracted.url,
                    fields: extracted.fields,
                    discovered_links: extracted.discovered_links.clone(),
                };
                if let Err(e) = self.sink.write(&extracted_data).await {
                    warn!("sink write error: {e}");
                }

                // Enqueue newly discovered links.
                if data.depth < self.max_depth {
                    for link in extracted.discovered_links {
                        let key = link.as_str().to_string();
                        if seen.insert(key) {
                            match self.state.enqueue(&link, data.depth + 1, Some(result.job_id)).await {
                                Ok(new_id) => {
                                    self.metrics.inc_pending(1);
                                    self.metrics.emit(ScrapeEvent::UrlDiscovered {
                                        url: link.as_str().to_string(),
                                        depth: data.depth + 1,
                                    });
                                    if in_flight.len() < self.concurrency {
                                        if let Err(e) = self
                                            .dispatch(job_tx, in_flight, &link, new_id, data.depth + 1)
                                            .await
                                        {
                                            debug!("dispatch error: {e}");
                                        }
                                    }
                                }
                                Err(e) => debug!("enqueue error: {e}"),
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let retryable = e.is_retryable();
                self.state.mark_failed(result.job_id, &e.to_string(), retryable).await?;
                if retryable {
                    self.metrics.inc_pending(1);
                } else {
                    self.metrics.inc_failed();
                }
                self.metrics.emit(ScrapeEvent::UrlFailed {
                    url: result.url.as_str().to_string(),
                    error: e.to_string(),
                    retryable,
                });
                error!(url = %result.url, error = %e, retryable, "URL failed");
            }
        }
        Ok(())
    }
}
