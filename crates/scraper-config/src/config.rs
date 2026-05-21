use scraper_core::{ExtractionRule, RenderOptions};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CrawlConfig {
    /// Seed URLs to begin crawling.
    pub seeds: Vec<String>,
    /// Maximum crawl depth (0 = seed URLs only).
    pub max_depth: u32,
    /// Restrict crawling to these hostnames and their subdomains.
    /// Empty list means no restriction (follow all discovered links).
    pub allowed_domains: Vec<String>,
    /// Maximum concurrent worker tasks.
    pub concurrency: usize,
    /// Global crawl timeout.
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
    pub rate_limit: RateLimitConfig,
    pub render: RenderConfig,
    pub extraction: ExtractionConfig,
    pub output: OutputConfig,
}

impl Default for CrawlConfig {
    fn default() -> Self {
        Self {
            seeds: Vec::new(),
            max_depth: 3,
            allowed_domains: Vec::new(),
            concurrency: 8,
            timeout: Duration::from_secs(300),
            rate_limit: RateLimitConfig::default(),
            render: RenderConfig::default(),
            extraction: ExtractionConfig::default(),
            output: OutputConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Requests per second per host.
    pub requests_per_second: f64,
    /// Maximum burst tokens per host.
    pub burst: u32,
    /// Per-request delay between retries.
    #[serde(with = "humantime_serde")]
    pub retry_delay: Duration,
    /// Maximum retry attempts before marking a URL failed.
    pub max_retries: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 2.0,
            burst: 5,
            retry_delay: Duration::from_secs(5),
            max_retries: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RenderConfig {
    /// Use headless browser for all pages when true.
    pub always_browser: bool,
    /// CSS selector patterns that trigger browser fallback when HTTP gets no JS content.
    pub browser_trigger_selectors: Vec<String>,
    pub options: RenderOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExtractionConfig {
    /// Named extraction rule sets, keyed by a URL pattern they apply to.
    pub rule_sets: Vec<RuleSet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSet {
    /// Glob or regex pattern matched against the page URL.
    pub url_pattern: String,
    pub rules: Vec<ExtractionRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputConfig {
    JsonLines {
        path: String,
    },
    Sqlite {
        path: String,
    },
    #[default]
    Stdout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = CrawlConfig::default();
        assert_eq!(cfg.max_depth, 3);
        assert_eq!(cfg.concurrency, 8);
        assert_eq!(cfg.rate_limit.requests_per_second, 2.0);
        assert_eq!(cfg.rate_limit.max_retries, 3);
    }

    #[test]
    fn config_round_trips_via_toml() {
        let cfg = CrawlConfig {
            seeds: vec!["https://example.com".into()],
            max_depth: 2,
            ..Default::default()
        };
        let toml_str = toml::to_string(&cfg).expect("serialise");
        let back: CrawlConfig = toml::from_str(&toml_str).expect("deserialise");
        assert_eq!(back.seeds, cfg.seeds);
        assert_eq!(back.max_depth, cfg.max_depth);
    }

    #[test]
    fn output_config_stdout_variant_serialises() {
        let out = OutputConfig::Stdout;
        let s = toml::to_string(&out).expect("serialise");
        assert!(s.contains("stdout"), "got: {s}");
    }
}
