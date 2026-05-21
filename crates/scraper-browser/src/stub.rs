//! Stub BrowserBackend for testing and non-browser builds.
use async_trait::async_trait;
use scraper_core::{BrowserBackend, CrawlError, FetchResponse, RenderOptions, Url};

/// Always returns an error; used when the `chromium` feature is disabled.
pub struct StubBrowser;

#[async_trait]
impl BrowserBackend for StubBrowser {
    async fn render(&self, _url: &Url, _opts: &RenderOptions) -> Result<FetchResponse, CrawlError> {
        Err(CrawlError::browser(
            "browser backend not enabled — recompile with feature 'chromium'",
        ))
    }

    async fn shutdown(&self) -> Result<(), CrawlError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper_core::{RenderOptions, Url};

    #[tokio::test]
    async fn stub_render_returns_error() {
        let b = StubBrowser;
        let url = Url::parse("https://example.com").unwrap();
        let result = b.render(&url, &RenderOptions::default()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CrawlError::Browser { .. }));
    }

    #[tokio::test]
    async fn stub_shutdown_succeeds() {
        let b = StubBrowser;
        b.shutdown().await.unwrap();
    }
}
