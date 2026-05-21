//! Full-pipeline integration tests: real axum HTTP server → Engine → assertions.
//!
//! Each test binds to port 0 (OS-assigned) so tests can run in parallel without
//! conflicts. `flavor = "multi_thread"` is required because the Engine spawns
//! worker tasks and the MetricsHub snapshot task concurrently.

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{extract::State, Router};
use scraper_core::{StateStore, Url};
use scraper_engine::Engine;
use scraper_extractor::CompositeEngine;
use scraper_fetch_http::HttpFetcher;
use scraper_metrics::MetricsHub;
use scraper_storage::{SqliteResultSink, SqliteStateStore};
use tokio::net::TcpListener;

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
async fn run_engine(seed: String, max_depth: u32, allowed_domains: Vec<String>) -> MetricsHub {
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

    let metrics = run_engine(format!("{base}/"), 2, vec!["127.0.0.1".into()]).await;

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

    let metrics = run_engine(format!("{base}/"), 0, vec!["127.0.0.1".into()]).await;

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
    )
    .await;

    server.abort();

    let snap = metrics.snapshot();
    assert_eq!(snap.urls_done, 2, "only / and /local should be visited");
    assert_eq!(snap.urls_failed, 0, "external link must not be attempted");
}
