use async_trait::async_trait;
use scraper_core::{BrowserBackend, CrawlError, FetchResponse, RenderOptions, Url};

pub struct ChromiumBackend;

#[async_trait]
impl BrowserBackend for ChromiumBackend {
    async fn render(&self, _url: &Url, _opts: &RenderOptions) -> Result<FetchResponse, CrawlError> {
        Err(CrawlError::browser("chromium backend not yet implemented"))
    }

    async fn shutdown(&self) -> Result<(), CrawlError> {
        Ok(())
    }
}
