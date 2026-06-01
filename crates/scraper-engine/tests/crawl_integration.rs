//! Full-pipeline integration tests: real axum HTTP server → Engine → assertions.
//!
//! Each test binds to port 0 (OS-assigned) so tests can run in parallel without
//! conflicts. `flavor = "multi_thread"` is required because the Engine spawns
//! worker tasks and the MetricsHub snapshot task concurrently.

use std::{collections::HashMap, sync::{Arc, Mutex as StdMutex}, time::Duration};

use async_trait::async_trait;
use axum::{extract::State, Router};
use scraper_config::{ExtractionConfig, RuleSet};
use scraper_core::{
    ChangeType, CrawlError, DeltaTracker, ExtractedData, ExtractionRule, ResultSink,
    RobotsChecker, SelectorKind, StateStore, Url,
};
use scraper_engine::Engine;
use scraper_extractor::CompositeEngine;
use scraper_fetch_http::HttpFetcher;
use scraper_metrics::MetricsHub;
use scraper_robots::RobotsCache;
use scraper_storage::{DeltaStore, SqliteResultSink, SqliteStateStore};
use tokio::net::TcpListener;

/// In-memory sink that captures written records for assertion.
struct CaptureSink(std::sync::Mutex<Vec<ExtractedData>>);

impl CaptureSink {
    fn new() -> Arc<Self> {
        Arc::new(Self(std::sync::Mutex::new(Vec::new())))
    }
    fn records(&self) -> Vec<ExtractedData> {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl ResultSink for CaptureSink {
    async fn write(&self, data: &ExtractedData) -> Result<(), CrawlError> {
        self.0.lock().unwrap().push(data.clone());
        Ok(())
    }
    async fn flush(&self) -> Result<(), CrawlError> {
        Ok(())
    }
}

type Pages = HashMap<&'static str, &'static str>;

/// Bind a random port, serve `pages` as HTML, return `(base_url, abort_handle)`.
async fn start_server(pages: Pages) -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        State(pages): State<Arc<Pages>>,
        req: axum::extract::Request,
    ) -> axum::response::Html<String> {
        axum::response::Html(
            pages
                .get(req.uri().path())
                .copied()
                .unwrap_or("")
                .to_string(),
        )
    }

    let app = Router::new().fallback(handler).with_state(Arc::new(pages));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

/// Seed `url`, run the engine to completion, sleep 350 ms so the MetricsHub
/// snapshot task has time to tick, then return the hub for assertions.
async fn run_engine(
    seed: String,
    max_depth: u32,
    allowed_domains: Vec<String>,
    robots: Option<Arc<dyn RobotsChecker>>,
) -> MetricsHub {
    let state = Arc::new(SqliteStateStore::open_memory().unwrap());
    let sink = Arc::new(SqliteResultSink::open_memory().unwrap());

    state
        .enqueue(&Url::parse(&seed).unwrap(), 0, None)
        .await
        .unwrap();

    let metrics = MetricsHub::new();
    let out = metrics.clone();

    Engine {
        fetcher: Arc::new(HttpFetcher::new(100.0, 50, 1024 * 1024).unwrap()),
        selector: Arc::new(CompositeEngine::new()),
        state,
        sink,
        metrics,
        max_depth,
        allowed_domains,
        concurrency: 2,
        robots,
        extraction_config: ExtractionConfig::default(),
        interval: None,
        delta_store: None,
    }
    .run()
    .await
    .unwrap();

    // MetricsHub snapshot is updated every 250 ms by a background task.
    // Sleep one extra tick so the final counter values are visible.
    tokio::time::sleep(Duration::from_millis(350)).await;
    out
}

// ── Test 1 ────────────────────────────────────────────────────────────────────

/// Engine follows links transitively within the depth limit.
///
/// Site: `/` → `/a` → `/b` (leaf).  With max_depth=2 all three pages are
/// reachable and the coordinator should visit each exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follows_links_within_depth() {
    let (base, server) = start_server(HashMap::from([
        ("/", "<html><body><a href=\"/a\">a</a></body></html>"),
        ("/a", "<html><body><a href=\"/b\">b</a></body></html>"),
        ("/b", "<html><body>leaf</body></html>"),
    ]))
    .await;

    let metrics = run_engine(format!("{base}/"), 2, vec!["127.0.0.1".into()], None).await;

    server.abort();

    let snap = metrics.snapshot();
    assert_eq!(snap.urls_done, 3, "/, /a, and /b should all be visited");
    assert_eq!(snap.urls_failed, 0);
}

// ── Test 2 ────────────────────────────────────────────────────────────────────

/// max_depth=0 means only the seed URL is fetched; no links are followed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn depth_zero_crawls_only_seed() {
    let (base, server) = start_server(HashMap::from([
        (
            "/",
            "<html><body><a href=\"/child\">child</a></body></html>",
        ),
        ("/child", "<html><body>leaf</body></html>"),
    ]))
    .await;

    let metrics = run_engine(format!("{base}/"), 0, vec!["127.0.0.1".into()], None).await;

    server.abort();

    let snap = metrics.snapshot();
    assert_eq!(snap.urls_done, 1, "only the seed should be crawled");
    assert_eq!(snap.urls_failed, 0);
}

// ── Test 3 ────────────────────────────────────────────────────────────────────

/// Links to hosts outside `allowed_domains` are silently dropped; the crawl
/// finishes without attempting any external requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domain_filter_blocks_external_links() {
    let (base, server) = start_server(HashMap::from([
        (
            "/",
            "<html><body>\
             <a href=\"/local\">local</a>\
             <a href=\"https://evil.example.com/bad\">evil</a>\
             </body></html>",
        ),
        ("/local", "<html><body>safe leaf</body></html>"),
    ]))
    .await;

    let metrics = run_engine(
        format!("{base}/"),
        2,
        vec!["127.0.0.1".into()], // evil.example.com is not in this list
        None,
    )
    .await;

    server.abort();

    let snap = metrics.snapshot();
    assert_eq!(snap.urls_done, 2, "only / and /local should be visited");
    assert_eq!(snap.urls_failed, 0, "external link must not be attempted");
}

// ── Test 4 ────────────────────────────────────────────────────────────────────

/// robots.txt `Disallow: /secret` prevents the coordinator from enqueuing
/// `/secret`, while `/allowed` is still crawled normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn robots_txt_blocks_disallowed_paths() {
    let (base, server) = start_server(HashMap::from([
        (
            "/robots.txt",
            "User-agent: *\nDisallow: /secret\n",
        ),
        (
            "/",
            r#"<html><body><a href="/secret">s</a><a href="/allowed">a</a></body></html>"#,
        ),
        ("/allowed", "<html><body>safe leaf</body></html>"),
        ("/secret", "<html><body>should never be crawled</body></html>"),
    ]))
    .await;

    let robots = Arc::new(RobotsCache::new("rust-scraper").unwrap());
    let metrics = run_engine(
        format!("{base}/"),
        2,
        vec!["127.0.0.1".into()],
        Some(robots as Arc<dyn RobotsChecker>),
    )
    .await;

    server.abort();

    let snap = metrics.snapshot();
    assert_eq!(snap.urls_done, 2, "only / and /allowed should be crawled");
    assert_eq!(snap.urls_failed, 0, "/secret must not be attempted");
}

// ── Test 5 ────────────────────────────────────────────────────────────────────

/// ExtractionConfig rules are applied to pages whose URL matches the pattern.
/// JSON-LD and Open Graph fields are auto-extracted on every page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extraction_rules_and_auto_extract_populate_fields() {
    let (base, server) = start_server(HashMap::from([
        (
            "/",
            r#"<html><head>
                <script type="application/ld+json">
                {"@type":"Product","name":"Blue Widget"}
                </script>
                <meta property="og:title" content="Blue Widget OG">
            </head><body><h1>Blue Widget</h1></body></html>"#,
        ),
    ]))
    .await;

    let extraction_config = ExtractionConfig {
        rule_sets: vec![RuleSet {
            url_pattern: "*/".into(),
            rules: vec![ExtractionRule {
                name: "heading".into(),
                selector: SelectorKind::Css("h1".into()),
                attr: None,
                many: false,
            }],
        }],
    };

    let sink = CaptureSink::new();
    let state = Arc::new(SqliteStateStore::open_memory().unwrap());
    let url_str = format!("{base}/");
    state
        .enqueue(&Url::parse(&url_str).unwrap(), 0, None)
        .await
        .unwrap();

    let metrics = MetricsHub::new();
    Engine {
        fetcher: Arc::new(HttpFetcher::new(100.0, 50, 1024 * 1024).unwrap()),
        selector: Arc::new(CompositeEngine::new()),
        state,
        sink: Arc::clone(&sink) as Arc<dyn ResultSink>,
        metrics,
        max_depth: 0,
        allowed_domains: vec!["127.0.0.1".into()],
        concurrency: 2,
        robots: None,
        extraction_config,
        interval: None,
        delta_store: None,
    }
    .run()
    .await
    .unwrap();

    server.abort();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let records = sink.records();
    assert_eq!(records.len(), 1, "one page crawled");

    let fields = &records[0].fields;
    assert_eq!(
        fields.get("heading").and_then(|v| v.as_str()),
        Some("Blue Widget"),
        "CSS rule extracted h1 text"
    );
    assert_eq!(
        fields.get("ld_type").and_then(|v| v.as_str()),
        Some("Product"),
        "JSON-LD @type auto-extracted"
    );
    assert_eq!(
        fields.get("ld_name").and_then(|v| v.as_str()),
        Some("Blue Widget"),
        "JSON-LD name auto-extracted"
    );
    assert_eq!(
        fields.get("og_title").and_then(|v| v.as_str()),
        Some("Blue Widget OG"),
        "OG title auto-extracted"
    );
}

// ── Test 6 ────────────────────────────────────────────────────────────────────

/// Delta detection: first crawl emits New events; second crawl on changed content
/// emits Modified events. Both are recorded in the shared DeltaStore.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delta_detection_records_new_and_modified_between_runs() {
    // Dynamic handler: serves whatever string is in `content` at the time of request.
    async fn dynamic_handler(
        State(c): State<Arc<StdMutex<String>>>,
    ) -> axum::response::Html<String> {
        axum::response::Html(c.lock().unwrap().clone())
    }

    let content = Arc::new(StdMutex::new(
        "<html><body><h1>Version One</h1></body></html>".to_string(),
    ));

    let app = Router::new()
        .fallback(dynamic_handler)
        .with_state(Arc::clone(&content));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    let base = format!("http://127.0.0.1:{port}");

    // Shared delta store — both engine runs write into the same SQLite instance.
    let delta = Arc::new(DeltaStore::open_memory().unwrap());

    // ── Run 1 ──
    let state1 = Arc::new(SqliteStateStore::open_memory().unwrap());
    state1
        .enqueue(&Url::parse(&format!("{base}/")).unwrap(), 0, None)
        .await
        .unwrap();

    Engine {
        fetcher: Arc::new(HttpFetcher::new(100.0, 50, 1024 * 1024).unwrap()),
        selector: Arc::new(CompositeEngine::new()),
        state: state1,
        sink: Arc::new(SqliteResultSink::open_memory().unwrap()),
        metrics: MetricsHub::new(),
        max_depth: 0,
        allowed_domains: vec!["127.0.0.1".into()],
        concurrency: 2,
        robots: None,
        extraction_config: ExtractionConfig::default(),
        interval: None,
        delta_store: Some(Arc::clone(&delta) as Arc<dyn DeltaTracker>),
    }
    .run()
    .await
    .unwrap();

    let after_run1 = delta.query_changes().unwrap();
    assert_eq!(after_run1.len(), 1, "run 1 should produce exactly one New event");
    assert_eq!(after_run1[0].change_type, ChangeType::New, "first visit is New");

    // Change the page content before run 2.
    *content.lock().unwrap() =
        "<html><body><h1>Version Two (changed)</h1></body></html>".to_string();

    // ── Run 2 ──
    let state2 = Arc::new(SqliteStateStore::open_memory().unwrap());
    state2
        .enqueue(&Url::parse(&format!("{base}/")).unwrap(), 0, None)
        .await
        .unwrap();

    Engine {
        fetcher: Arc::new(HttpFetcher::new(100.0, 50, 1024 * 1024).unwrap()),
        selector: Arc::new(CompositeEngine::new()),
        state: state2,
        sink: Arc::new(SqliteResultSink::open_memory().unwrap()),
        metrics: MetricsHub::new(),
        max_depth: 0,
        allowed_domains: vec!["127.0.0.1".into()],
        concurrency: 2,
        robots: None,
        extraction_config: ExtractionConfig::default(),
        interval: None,
        delta_store: Some(Arc::clone(&delta) as Arc<dyn DeltaTracker>),
    }
    .run()
    .await
    .unwrap();

    server.abort();

    let after_run2 = delta.query_changes().unwrap();
    assert_eq!(after_run2.len(), 2, "run 2 should add a Modified event");
    assert_eq!(
        after_run2[1].change_type,
        ChangeType::Modified,
        "second visit with different content is Modified"
    );
    assert_ne!(
        after_run2[1].old_hash, after_run2[1].new_hash,
        "hashes must differ on a modification"
    );
}
