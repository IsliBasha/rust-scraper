use async_trait::async_trait;
use bytes::Bytes;
use reqwest::{Client, StatusCode};
use scraper_core::{CrawlError, CrawlJob, FetchResponse, Fetcher, Url};
use std::time::Instant;
use tracing::{debug, warn};

use crate::rate_limiter::PerHostRateLimiter;

/// HTTP fetcher backed by `reqwest`, with per-host governor rate limiting.
pub struct HttpFetcher {
    client: Client,
    rate_limiter: PerHostRateLimiter,
    max_response_bytes: usize,
}

impl HttpFetcher {
    pub fn new(
        requests_per_second: f64,
        burst: u32,
        max_response_bytes: usize,
    ) -> Result<Self, CrawlError> {
        let client = Client::builder()
            .user_agent("rust-scraper/0.1 (+https://github.com/example/rust-scraper)")
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| CrawlError::network(e.to_string()))?;

        Ok(Self {
            client,
            rate_limiter: PerHostRateLimiter::new(requests_per_second, burst),
            max_response_bytes,
        })
    }
}

#[async_trait]
impl Fetcher for HttpFetcher {
    async fn fetch(&self, job: &CrawlJob) -> Result<FetchResponse, CrawlError> {
        let host = job.url.host();
        self.rate_limiter.wait(&host).await;

        let start = Instant::now();
        debug!(url = %job.url, "fetching");

        let response = self
            .client
            .get(job.url.as_str())
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    CrawlError::Timeout { elapsed_ms: start.elapsed().as_millis() as u64 }
                } else if e.is_connect() || e.is_request() {
                    CrawlError::network(e.to_string())
                } else {
                    CrawlError::network(e.to_string())
                }
            })?;

        let status = response.status();
        let final_url = Url::parse(response.url().as_str())
            .unwrap_or_else(|_| job.url.clone());

        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(CrawlError::RateLimited { host });
        }

        if !status.is_success() && !status.is_redirection() {
            warn!(url = %job.url, status = status.as_u16(), "non-success status");
            return Err(CrawlError::HttpStatus { status: status.as_u16() });
        }

        // Read body with size cap to avoid OOM on pathological responses.
        let raw_bytes = response
            .bytes()
            .await
            .map_err(|e| CrawlError::network(e.to_string()))?;

        let body: Bytes = if raw_bytes.len() > self.max_response_bytes {
            raw_bytes.slice(..self.max_response_bytes)
        } else {
            raw_bytes
        };

        let elapsed = start.elapsed();
        debug!(url = %job.url, status = status.as_u16(), bytes = body.len(), "fetched");

        Ok(FetchResponse {
            status: status.as_u16(),
            body,
            final_url,
            elapsed,
            rendered_via_browser: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetcher_builds_without_error() {
        HttpFetcher::new(2.0, 5, 10 * 1024 * 1024).unwrap();
    }
}
