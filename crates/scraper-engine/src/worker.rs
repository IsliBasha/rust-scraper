use std::sync::Arc;

use bytes::Bytes;
use scraper_core::{CrawlError, ExtractionRule, Fetcher, Url};
use tokio::sync::mpsc;

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
///
/// Uses an `async_channel::Receiver` so multiple workers can pull from the
/// same channel concurrently without serialising on a `Mutex`.
pub async fn run_worker(
    job_rx: async_channel::Receiver<CoordMsg>,
    result_tx: mpsc::Sender<WorkerResult>,
    fetcher: Arc<dyn Fetcher>,
) {
    loop {
        let msg = match job_rx.recv().await {
            Ok(msg) => msg,
            Err(_) => break, // channel closed — coordinator is done
        };

        match msg {
            CoordMsg::Job(job) => {
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
                    .send(WorkerResult {
                        job_id: id,
                        url,
                        outcome,
                    })
                    .await
                    .is_err()
                {
                    break; // coordinator dropped its receiver — shut down
                }
            }
            CoordMsg::Shutdown => break,
        }
    }
}

fn decode_html(body: Bytes) -> String {
    // For valid UTF-8 (the vast majority of modern web pages), `from_utf8` reuses the
    // Vec's allocation — no second copy. `from_utf8_lossy().into_owned()` always
    // allocates a new String even when the bytes were already valid UTF-8.
    let vec: Vec<u8> = body.into();
    match String::from_utf8(vec) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}
