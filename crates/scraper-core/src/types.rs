use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CrawlError;

pub type UrlId = u64;

/// A validated, normalized HTTP/HTTPS URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Url(String);

impl Url {
    pub fn parse(raw: &str) -> Result<Self, CrawlError> {
        let parsed = url::Url::parse(raw).map_err(|e| CrawlError::InvalidUrl {
            message: e.to_string(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CrawlError::InvalidUrl {
                message: format!("unsupported scheme: {}", parsed.scheme()),
            });
        }
        Ok(Self(parsed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns a parsed `url::Url`. Panics only if the internal invariant is violated.
    pub fn parsed(&self) -> url::Url {
        url::Url::parse(&self.0).expect("Url invariant: stored string is always valid")
    }

    pub fn host(&self) -> String {
        self.parsed().host_str().unwrap_or("").to_string()
    }

    pub fn join(&self, path: &str) -> Result<Self, CrawlError> {
        self.parsed()
            .join(path)
            .map_err(|e| CrawlError::InvalidUrl {
                message: e.to_string(),
            })
            .and_then(|u| Self::parse(u.as_str()))
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for Url {
    type Error = CrawlError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::parse(s)
    }
}

impl TryFrom<String> for Url {
    type Error = CrawlError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

/// A single unit of work dispatched to a worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlJob {
    pub id: UrlId,
    pub url: Url,
    pub depth: u32,
    pub parent: Option<UrlId>,
    pub attempt: u32,
    /// When true, skip HTTP and use the BrowserBackend directly.
    pub force_browser: bool,
}

/// Raw response returned by a Fetcher or BrowserBackend.
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Bytes,
    /// URL after following any redirects.
    pub final_url: Url,
    pub elapsed: Duration,
    pub rendered_via_browser: bool,
}

/// Structured data extracted from one page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedData {
    pub url: Url,
    /// Named fields produced by the SelectorEngine.
    pub fields: BTreeMap<String, Value>,
    /// Outbound links discovered during extraction.
    pub discovered_links: Vec<Url>,
    /// SHA-256 hex digest of the raw HTML. Set by the coordinator, not the extractor.
    pub content_hash: Option<String>,
}

/// The kind of change detected between successive crawl runs for a URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    New,
    Modified,
    Removed,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::New => write!(f, "new"),
            Self::Modified => write!(f, "modified"),
            Self::Removed => write!(f, "removed"),
        }
    }
}

/// A change detected between two successive crawl runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub url: String,
    pub change_type: ChangeType,
    /// Unix timestamp (seconds) when this change was detected.
    pub detected_at: i64,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

/// A URL row loaded from the StateStore for resume/recovery.
#[derive(Debug, Clone)]
pub struct PersistedUrl {
    pub id: UrlId,
    pub url: Url,
    pub depth: u32,
    pub parent: Option<UrlId>,
    pub status: UrlStatus,
    pub attempt: u32,
}

/// Lifecycle states a URL passes through in the frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlStatus {
    Pending,
    InProgress,
    Done,
    Failed,
    Skipped,
}

impl std::fmt::Display for UrlStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Done => write!(f, "done"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// A single extraction rule applied by a SelectorEngine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionRule {
    /// Key name in the resulting `ExtractedData.fields`.
    pub name: String,
    pub selector: SelectorKind,
    /// HTML attribute to read; `None` means text content.
    pub attr: Option<String>,
    /// Collect all matches into a JSON array if true; otherwise take only the first.
    pub many: bool,
}

/// A CSS or XPath selector expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorKind {
    Css(String),
    XPath(String),
}

/// Options forwarded to a BrowserBackend when rendering a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderOptions {
    pub wait_for: WaitStrategy,
    pub timeout: Duration,
    pub capture_screenshot: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            wait_for: WaitStrategy::Load,
            timeout: Duration::from_secs(30),
            capture_screenshot: false,
        }
    }
}

/// When to consider a browser navigation complete.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitStrategy {
    Load,
    NetworkIdle,
    Selector(String),
    Delay(Duration),
}
