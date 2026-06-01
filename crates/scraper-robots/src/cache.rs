use std::collections::HashMap;

use async_trait::async_trait;
use scraper_core::{RobotsChecker, Url};
use tokio::sync::RwLock;
use tracing::{debug, warn};

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// Returns true if `user_agent` is allowed to fetch `url` according to `body`.
/// Treats any parse failure as permissive (RFC 9309 §2.3.1.1).
fn parse_and_check(body: &[u8], user_agent: &str, url: &str) -> bool {
    match texting_robots::Robot::new(user_agent, body) {
        Ok(robot) => robot.allowed(url),
        Err(_) => true,
    }
}

/// Constructs the robots.txt URL for a given crawl URL by reusing its origin.
fn make_robots_url(url: &Url) -> String {
    let mut p = url.parsed();
    p.set_path("/robots.txt");
    p.set_query(None);
    p.set_fragment(None);
    p.to_string()
}

/// Cache key: ASCII-serialized origin (scheme + host + port).
fn origin_key(url: &Url) -> String {
    url.parsed().origin().ascii_serialization()
}

// ── Cache types ───────────────────────────────────────────────────────────────

enum Entry {
    /// Fetch failed or returned 404/410 — treat as "allow all".
    Permissive,
    /// Raw robots.txt bytes fetched successfully.
    Fetched(Vec<u8>),
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Fetches and caches `robots.txt` per origin, then checks URL permission.
///
/// The first request to an origin fetches `{origin}/robots.txt` and stores
/// the result. Subsequent requests for the same origin reuse the cached bytes.
/// Failures (network, 4xx, parse) fall back to permissive per RFC 9309.
pub struct RobotsCache {
    client: reqwest::Client,
    user_agent: String,
    entries: RwLock<HashMap<String, Entry>>,
}

impl RobotsCache {
    /// Create a cache that identifies itself to servers as `user_agent_token`
    /// (e.g. `"rust-scraper"`). Use just the product token, not the full
    /// User-Agent header value.
    pub fn new(user_agent_token: impl Into<String>) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("rust-scraper/0.1")
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            user_agent: user_agent_token.into(),
            entries: RwLock::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl RobotsChecker for RobotsCache {
    async fn is_allowed(&self, url: &Url) -> bool {
        let key = origin_key(url);

        // Fast path: already cached for this origin.
        {
            let guard = self.entries.read().await;
            if let Some(entry) = guard.get(&key) {
                return match entry {
                    Entry::Permissive => true,
                    Entry::Fetched(body) => parse_and_check(body, &self.user_agent, url.as_str()),
                };
            }
        }

        // Slow path: fetch robots.txt for this origin.
        let robots_url = make_robots_url(url);
        debug!(url = %robots_url, "fetching robots.txt");

        let entry = match self.client.get(&robots_url).send().await {
            Err(e) => {
                warn!(url = %robots_url, error = %e, "robots.txt fetch failed — permissive");
                Entry::Permissive
            }
            Ok(resp) if matches!(resp.status().as_u16(), 404 | 410) => {
                debug!(url = %robots_url, "robots.txt absent — permissive");
                Entry::Permissive
            }
            Ok(resp) if !resp.status().is_success() => {
                warn!(
                    url = %robots_url,
                    status = resp.status().as_u16(),
                    "robots.txt non-success — permissive"
                );
                Entry::Permissive
            }
            Ok(resp) => match resp.bytes().await {
                Ok(body) => Entry::Fetched(body.to_vec()),
                Err(e) => {
                    warn!(url = %robots_url, error = %e, "robots.txt body read error — permissive");
                    Entry::Permissive
                }
            },
        };

        let allowed = match &entry {
            Entry::Permissive => true,
            Entry::Fetched(body) => parse_and_check(body, &self.user_agent, url.as_str()),
        };

        self.entries.write().await.insert(key, entry);
        allowed
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    // ── parse_and_check unit tests (no HTTP, no async) ────────────────────────

    #[test]
    fn disallow_all_blocks_any_path() {
        let body = b"User-agent: *\nDisallow: /\n";
        assert!(!parse_and_check(body, "testbot", "https://example.com/page"));
        assert!(!parse_and_check(body, "testbot", "https://example.com/"));
    }

    #[test]
    fn disallow_specific_path_blocks_only_that_subtree() {
        let body = b"User-agent: *\nDisallow: /secret\n";
        assert!(!parse_and_check(body, "testbot", "https://example.com/secret"));
        assert!(!parse_and_check(body, "testbot", "https://example.com/secret/sub"));
        assert!(parse_and_check(body, "testbot", "https://example.com/public"));
        assert!(parse_and_check(body, "testbot", "https://example.com/"));
    }

    #[test]
    fn empty_robots_txt_allows_everything() {
        assert!(parse_and_check(b"", "testbot", "https://example.com/anything"));
        assert!(parse_and_check(b"", "testbot", "https://example.com/"));
    }

    #[test]
    fn parse_failure_falls_back_to_permissive() {
        assert!(parse_and_check(b"\x00\xff\xfe", "testbot", "https://example.com/"));
    }

    #[test]
    fn specific_user_agent_overrides_wildcard() {
        // testbot: /bot-only blocked, everything else allowed.
        // *      : everything blocked.
        let body =
            b"User-agent: testbot\nDisallow: /bot-only\nUser-agent: *\nDisallow: /\n";
        assert!(!parse_and_check(body, "testbot", "https://example.com/bot-only"));
        assert!(parse_and_check(body, "testbot", "https://example.com/public"));
        assert!(!parse_and_check(body, "otherbot", "https://example.com/public"));
    }

    // ── URL helpers ────────────────────────────────────────────────────────────

    #[test]
    fn make_robots_url_strips_path_and_query() {
        assert_eq!(
            make_robots_url(&url("https://example.com/page/sub?q=1#anchor")),
            "https://example.com/robots.txt"
        );
    }

    #[test]
    fn make_robots_url_preserves_non_default_port() {
        assert_eq!(
            make_robots_url(&url("http://127.0.0.1:8080/page")),
            "http://127.0.0.1:8080/robots.txt"
        );
    }

    #[test]
    fn origin_key_differs_by_port() {
        let a = origin_key(&url("http://127.0.0.1:9000/a"));
        let b = origin_key(&url("http://127.0.0.1:9001/a"));
        assert_ne!(a, b);
    }

    #[test]
    fn origin_key_same_for_different_paths() {
        let a = origin_key(&url("https://example.com/foo"));
        let b = origin_key(&url("https://example.com/bar?x=1"));
        assert_eq!(a, b);
    }
}
