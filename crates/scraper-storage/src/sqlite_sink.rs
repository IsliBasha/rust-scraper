use async_trait::async_trait;
use rusqlite::{params, Connection};
use scraper_core::{CrawlError, ExtractedData, ResultSink};
use std::sync::Mutex;

pub struct SqliteResultSink {
    conn: Mutex<Connection>,
}

impl SqliteResultSink {
    pub fn open(path: &str) -> Result<Self, CrawlError> {
        let conn = Connection::open(path).map_err(|e| CrawlError::storage(e.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        conn.execute_batch(SCHEMA).map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_memory() -> Result<Self, CrawlError> {
        Self::open(":memory:")
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS results (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    url        TEXT    NOT NULL,
    fields     TEXT    NOT NULL,
    scraped_at TEXT    NOT NULL DEFAULT (datetime('now'))
);
";

#[async_trait]
impl ResultSink for SqliteResultSink {
    async fn write(&self, data: &ExtractedData) -> Result<(), CrawlError> {
        let fields_json = serde_json::to_string(&data.fields)
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO results (url, fields) VALUES (?1, ?2)",
            params![data.url.as_str(), fields_json],
        )
        .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(())
    }

    async fn flush(&self) -> Result<(), CrawlError> {
        // PASSIVE checkpoint — non-blocking WAL flush.
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper_core::{ExtractedData, Url};
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn write_and_flush_succeed() {
        let sink = SqliteResultSink::open_memory().unwrap();
        let data = ExtractedData {
            url: Url::parse("https://example.com").unwrap(),
            fields: {
                let mut m = BTreeMap::new();
                m.insert("title".into(), serde_json::Value::String("Hello".into()));
                m
            },
            discovered_links: Vec::new(),
        };
        sink.write(&data).await.unwrap();
        sink.flush().await.unwrap();

        let conn = sink.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM results", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
