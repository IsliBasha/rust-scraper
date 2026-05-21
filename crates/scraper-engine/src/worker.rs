use std::sync::Arc;

use bytes::Bytes;
use scraper_core::{CrawlError, ExtractionRule, Fetcher, Url};
use tokio::sync::{mpsc, Mutex};

use crate::coordinator::CoordMsg;

/// Data a worker sends back after fetching a page.
pub struct WorkerResult {
    pub job_id: scraper_core::UrlId,
    pub url: Url,
    pub outcome: Result<FetchedPage, CrawlError>,
}

pub struct FetchedPage {
    pub url: Url,
    pub html: String,
    pub depth: u32,
    pub rules: Vec<ExtractionRule>,
}

/// Single worker loop: receive jobs, fetch, return results.
pub async fn run_worker(
    job_rx: Arc<Mutex<mpsc::Receiver<CoordMsg>>>,
    result_tx: mpsc::Sender<WorkerResult>,
    fetcher: Arc<dyn Fetcher>,
) {
    loop {
        let msg = {
            let mut rx = job_rx.lock().await;
            rx.recv().await
        };

        match msg {
            Some(CoordMsg::Job(job)) => {
                let url = job.url.clone();
                let id = job.id;
                let depth = job.depth;

                let outcome = match fetcher.fetch(&job).await {
                    Ok(resp) => {
                        let html = decode_html(resp.body);
                        Ok(FetchedPage {
                            url: resp.final_url,
                            html,
                            depth,
                            rules: Vec::new(), // rules injected by coordinator in future
                        })
                    }
                    Err(e) => Err(e),
                };

                if result_tx
                    .send(WorkerResult { job_id: id, url, outcome })
                    .await
                    .is_err()
                {
                    break; // coordinator dropped its receiver — shut down
                }
            }
            Some(CoordMsg::Shutdown) | None => break,
        }
    }
}

fn decode_html(body: Bytes) -> String {
    // Use lossy UTF-8 decoding; charset detection is a future enhancement.
    String::from_utf8_lossy(&body).into_owned()
}
