use async_trait::async_trait;
use rusqlite::{params, Connection};
use scraper_core::{CrawlError, PersistedUrl, StateStore, Url, UrlId, UrlStatus};
use std::sync::Mutex;

/// SQLite-backed crawl frontier. Uses a dedicated connection with WAL mode.
///
/// `Connection` is `!Sync`, so it lives behind a `Mutex` owned by a single DB task.
/// In the engine, all StateStore calls are routed through a `tokio::sync::mpsc` channel
/// to the DB owner task — this struct is the implementation used inside that task.
pub struct SqliteStateStore {
    conn: Mutex<Connection>,
}

impl SqliteStateStore {
    pub fn open(path: &str) -> Result<Self, CrawlError> {
        let conn = Connection::open(path).map_err(|e| CrawlError::storage(e.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_memory() -> Result<Self, CrawlError> {
        Self::open(":memory:")
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS urls (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    url      TEXT    NOT NULL UNIQUE,
    depth    INTEGER NOT NULL DEFAULT 0,
    parent   INTEGER,
    status   TEXT    NOT NULL DEFAULT 'pending',
    attempt  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_urls_status ON urls(status);
";

#[async_trait]
impl StateStore for SqliteStateStore {
    async fn load_pending(&self) -> Result<Vec<PersistedUrl>, CrawlError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, url, depth, parent, status, attempt FROM urls WHERE status IN ('pending', 'in_progress')"
            )
            .map_err(|e| CrawlError::storage(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                    row.get::<_, String>(4)?,
                    row.get::<_, u32>(5)?,
                ))
            })
            .map_err(|e| CrawlError::storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (id, url_str, depth, parent, status_str, attempt) =
                row.map_err(|e| CrawlError::storage(e.to_string()))?;
            let url = Url::parse(&url_str)?;
            let status = parse_status(&status_str)?;
            out.push(PersistedUrl {
                id,
                url,
                depth,
                parent,
                status,
                attempt,
            });
        }
        Ok(out)
    }

    async fn enqueue(
        &self,
        url: &Url,
        depth: u32,
        parent: Option<UrlId>,
    ) -> Result<UrlId, CrawlError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO urls (url, depth, parent) VALUES (?1, ?2, ?3)",
            params![url.as_str(), depth, parent.map(|v| v as i64)],
        )
        .map_err(|e| CrawlError::storage(e.to_string()))?;

        // On a new insert, last_insert_rowid() is ready with no extra query.
        // On a duplicate (INSERT OR IGNORE skipped the row), changes() == 0 so we SELECT.
        let id: i64 = if conn.changes() > 0 {
            conn.last_insert_rowid()
        } else {
            conn.query_row(
                "SELECT id FROM urls WHERE url = ?1",
                params![url.as_str()],
                |row| row.get(0),
            )
            .map_err(|e| CrawlError::storage(e.to_string()))?
        };

        Ok(id as u64)
    }

    async fn set_status(&self, id: UrlId, status: UrlStatus) -> Result<(), CrawlError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE urls SET status = ?1 WHERE id = ?2",
            params![status.to_string(), id as i64],
        )
        .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(())
    }

    async fn mark_done(&self, id: UrlId) -> Result<(), CrawlError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE urls SET status = 'done' WHERE id = ?1",
            params![id as i64],
        )
        .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(())
    }

    async fn mark_failed(
        &self,
        id: UrlId,
        _error: &str,
        retryable: bool,
    ) -> Result<(), CrawlError> {
        let status = if retryable { "pending" } else { "failed" };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE urls SET status = ?1, attempt = attempt + 1 WHERE id = ?2",
            params![status, id as i64],
        )
        .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(())
    }

    async fn reclaim_in_flight(&self) -> Result<u64, CrawlError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE urls SET status = 'pending' WHERE status = 'in_progress'",
                [],
            )
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(changed as u64)
    }

    async fn reset_for_recrawl(&self) -> Result<u64, CrawlError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE urls SET status = 'pending', attempt = 0 WHERE status = 'done'",
                [],
            )
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(changed as u64)
    }

    /// Single-transaction override: marks `done_id` done and inserts all `links` in one
    /// `BEGIN … COMMIT` block, replacing N+1 individual write transactions with one.
    async fn mark_done_and_enqueue_batch(
        &self,
        done_id: UrlId,
        links: &[(scraper_core::Url, u32, Option<UrlId>)],
    ) -> Result<Vec<UrlId>, CrawlError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| CrawlError::storage(e.to_string()))?;

        tx.execute(
            "UPDATE urls SET status = 'done' WHERE id = ?1",
            params![done_id as i64],
        )
        .map_err(|e| CrawlError::storage(e.to_string()))?;

        let mut ids = Vec::with_capacity(links.len());
        for (url, depth, parent) in links {
            tx.execute(
                "INSERT OR IGNORE INTO urls (url, depth, parent) VALUES (?1, ?2, ?3)",
                params![url.as_str(), depth, parent.map(|v| v as i64)],
            )
            .map_err(|e| CrawlError::storage(e.to_string()))?;

            let id: i64 = if tx.changes() > 0 {
                tx.last_insert_rowid()
            } else {
                tx.query_row(
                    "SELECT id FROM urls WHERE url = ?1",
                    params![url.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| CrawlError::storage(e.to_string()))?
            };
            ids.push(id as u64);
        }

        tx.commit()
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(ids)
    }
}

fn parse_status(s: &str) -> Result<UrlStatus, CrawlError> {
    match s {
        "pending" => Ok(UrlStatus::Pending),
        "in_progress" => Ok(UrlStatus::InProgress),
        "done" => Ok(UrlStatus::Done),
        "failed" => Ok(UrlStatus::Failed),
        "skipped" => Ok(UrlStatus::Skipped),
        other => Err(CrawlError::storage(format!("unknown status: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper_core::Url;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[tokio::test]
    async fn enqueue_idempotent_and_load_pending() {
        let store = SqliteStateStore::open_memory().unwrap();
        let u = url("https://example.com/");

        let id1 = store.enqueue(&u, 0, None).await.unwrap();
        let id2 = store.enqueue(&u, 0, None).await.unwrap();
        assert_eq!(id1, id2, "INSERT OR IGNORE must be idempotent");

        let pending = store.load_pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].url, u);
    }

    #[tokio::test]
    async fn mark_done_removes_from_pending() {
        let store = SqliteStateStore::open_memory().unwrap();
        let u = url("https://example.com/page");

        let id = store.enqueue(&u, 1, None).await.unwrap();
        store.mark_done(id).await.unwrap();

        let pending = store.load_pending().await.unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn mark_failed_retryable_keeps_pending() {
        let store = SqliteStateStore::open_memory().unwrap();
        let u = url("https://example.com/retry");

        let id = store.enqueue(&u, 0, None).await.unwrap();
        store.mark_failed(id, "timeout", true).await.unwrap();

        let pending = store.load_pending().await.unwrap();
        assert_eq!(pending[0].attempt, 1);
        assert_eq!(pending[0].status, UrlStatus::Pending);
    }

    #[tokio::test]
    async fn reclaim_in_flight_resets_to_pending() {
        let store = SqliteStateStore::open_memory().unwrap();
        let u = url("https://example.com/inflight");

        let id = store.enqueue(&u, 0, None).await.unwrap();
        store.set_status(id, UrlStatus::InProgress).await.unwrap();

        let reclaimed = store.reclaim_in_flight().await.unwrap();
        assert_eq!(reclaimed, 1);

        let pending = store.load_pending().await.unwrap();
        assert_eq!(pending[0].status, UrlStatus::Pending);
    }

    #[tokio::test]
    async fn reset_for_recrawl_moves_done_urls_back_to_pending() {
        let store = SqliteStateStore::open_memory().unwrap();
        let u = url("https://example.com/page");

        let id = store.enqueue(&u, 0, None).await.unwrap();
        store.mark_done(id).await.unwrap();

        // Verify it's gone from pending after mark_done.
        assert!(store.load_pending().await.unwrap().is_empty());

        let reset = store.reset_for_recrawl().await.unwrap();
        assert_eq!(reset, 1, "one URL should be reset");

        let pending = store.load_pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, UrlStatus::Pending);
        assert_eq!(pending[0].attempt, 0);
    }
}
