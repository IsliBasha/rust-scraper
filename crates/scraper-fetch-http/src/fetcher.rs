use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use scraper_core::{CrawlError, CrawlJob, FetchResponse, Fetcher, Url};
use std::net::IpAddr;
use std::time::Instant;
use tracing::{debug, warn};

use crate::rate_limiter::PerHostRateLimiter;

/// Returns true for any IP that must never be a redirect target.
/// Covers RFC-1918 private ranges, loopback, link-local (169.254/16 — cloud metadata
/// endpoints), broadcast, and unspecified; plus IPv6 loopback, unique-local, link-local.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()       // 10/8, 172.16/12, 192.168/16
                || v4.is_loopback()   // 127/8
                || v4.is_link_local() // 169.254/16 — AWS/GCP/Azure metadata
                || v4.is_broadcast()  // 255.255.255.255
                || v4.is_unspecified() // 0.0.0.0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()       // ::1
                || v6.is_unspecified() // ::
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

/// Redirect policy that enforces a 10-hop limit and rejects redirects to
/// private/loopback addresses to prevent SSRF attacks via server-side redirects.
fn ssrf_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error(CrawlError::network("too many redirects"));
        }
        if let Some(host) = attempt.url().host_str() {
            if let Ok(ip) = host.parse::<IpAddr>() {
                if is_private_ip(ip) {
                    return attempt.error(CrawlError::network(format!(
                        "SSRF guard: redirect to private address blocked ({ip})"
                    )));
                }
            }
        }
        attempt.follow()
    })
}

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
            .redirect(ssrf_redirect_policy())
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
                    CrawlError::Timeout {
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    }
                } else {
                    CrawlError::network(e.to_string())
                }
            })?;

        let status = response.status();
        let final_url = Url::parse(response.url().as_str()).unwrap_or_else(|_| job.url.clone());

        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(CrawlError::RateLimited { host });
        }

        if !status.is_success() && !status.is_redirection() {
            warn!(url = %job.url, status = status.as_u16(), "non-success status");
            return Err(CrawlError::HttpStatus {
                status: status.as_u16(),
            });
        }

        // Stream the body and stop reading once max_response_bytes is reached.
        // This prevents OOM on pathological responses — we never buffer more than the cap.
        let cap = self.max_response_bytes;
        let mut stream = response.bytes_stream();
        let mut buf = BytesMut::with_capacity(cap.min(256 * 1024));
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| CrawlError::network(e.to_string()))?;
            let remaining = cap.saturating_sub(buf.len());
            if remaining == 0 {
                break;
            }
            let take = chunk.len().min(remaining);
            buf.extend_from_slice(&chunk[..take]);
            if take < chunk.len() {
                break;
            }
        }
        let body: Bytes = buf.freeze();

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

    #[test]
    fn ssrf_guard_blocks_private_ips() {
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("169.254.169.254".parse().unwrap())); // AWS metadata
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("93.184.216.34".parse().unwrap()));
    }
}
