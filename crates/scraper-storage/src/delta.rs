use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use scraper_core::{ChangeEvent, ChangeType, CrawlError, DeltaTracker};
use sha2::{Digest, Sha256};

/// Returns the current Unix timestamp in seconds.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Hex-encoded SHA-256 digest of `html`.
pub fn hash_html(html: &str) -> String {
    let mut h = Sha256::new();
    h.update(html.as_bytes());
    format!("{:x}", h.finalize())
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS page_hashes (
    url        TEXT    PRIMARY KEY,
    last_hash  TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS changes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    url         TEXT    NOT NULL,
    change_type TEXT    NOT NULL,
    detected_at INTEGER NOT NULL,
    old_hash    TEXT,
    new_hash    TEXT
);
";

/// SQLite-backed delta tracker.
///
/// Maintains a `page_hashes` table (latest hash per URL) and a `changes`
/// table (append-only log of all detected changes across crawl runs).
pub struct DeltaStore {
    conn: Mutex<Connection>,
}

impl DeltaStore {
    pub fn open(path: &str) -> Result<Self, CrawlError> {
        let conn = Connection::open(path).map_err(|e| CrawlError::storage(e.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
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

    /// Return all recorded change events ordered by insertion id.
    pub fn query_changes(&self) -> Result<Vec<ChangeEvent>, CrawlError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT url, change_type, detected_at, old_hash, new_hash
                 FROM changes ORDER BY id",
            )
            .map_err(|e| CrawlError::storage(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|e| CrawlError::storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (url, ct_str, detected_at, old_hash, new_hash) =
                row.map_err(|e| CrawlError::storage(e.to_string()))?;
            let change_type = match ct_str.as_str() {
                "new" => ChangeType::New,
                "modified" => ChangeType::Modified,
                "removed" => ChangeType::Removed,
                other => return Err(CrawlError::storage(format!("unknown change_type: {other}"))),
            };
            out.push(ChangeEvent {
                url,
                change_type,
                detected_at,
                old_hash,
                new_hash,
            });
        }
        Ok(out)
    }
}

#[async_trait]
impl DeltaTracker for DeltaStore {
    async fn detect_and_record(
        &self,
        url: &str,
        new_hash: &str,
    ) -> Result<Option<ChangeEvent>, CrawlError> {
        let conn = self.conn.lock().unwrap();
        let now = unix_now();

        let old_hash: Option<String> = conn
            .query_row(
                "SELECT last_hash FROM page_hashes WHERE url = ?1",
                params![url],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| CrawlError::storage(e.to_string()))?;

        // Upsert: always refresh the timestamp so detect_removed can identify
        // URLs not visited in the current crawl run.
        conn.execute(
            "INSERT INTO page_hashes (url, last_hash, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(url) DO UPDATE SET last_hash = ?2, updated_at = ?3",
            params![url, new_hash, now],
        )
        .map_err(|e| CrawlError::storage(e.to_string()))?;

        let event = match &old_hash {
            None => {
                conn.execute(
                    "INSERT INTO changes (url, change_type, detected_at, old_hash, new_hash)
                     VALUES (?1, 'new', ?2, NULL, ?3)",
                    params![url, now, new_hash],
                )
                .map_err(|e| CrawlError::storage(e.to_string()))?;
                Some(ChangeEvent {
                    url: url.to_string(),
                    change_type: ChangeType::New,
                    detected_at: now,
                    old_hash: None,
                    new_hash: Some(new_hash.to_string()),
                })
            }
            Some(prev) if prev != new_hash => {
                conn.execute(
                    "INSERT INTO changes (url, change_type, detected_at, old_hash, new_hash)
                     VALUES (?1, 'modified', ?2, ?3, ?4)",
                    params![url, now, prev, new_hash],
                )
                .map_err(|e| CrawlError::storage(e.to_string()))?;
                Some(ChangeEvent {
                    url: url.to_string(),
                    change_type: ChangeType::Modified,
                    detected_at: now,
                    old_hash: Some(prev.clone()),
                    new_hash: Some(new_hash.to_string()),
                })
            }
            _ => None,
        };

        Ok(event)
    }

    async fn detect_removed(&self, crawl_start_secs: i64) -> Result<Vec<ChangeEvent>, CrawlError> {
        let conn = self.conn.lock().unwrap();
        let now = unix_now();

        let mut stmt = conn
            .prepare("SELECT url, last_hash FROM page_hashes WHERE updated_at < ?1")
            .map_err(|e| CrawlError::storage(e.to_string()))?;

        let stale: Vec<(String, String)> = stmt
            .query_map(params![crawl_start_secs], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| CrawlError::storage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        let mut events = Vec::new();
        for (url, old_hash) in stale {
            conn.execute(
                "INSERT INTO changes (url, change_type, detected_at, old_hash, new_hash)
                 VALUES (?1, 'removed', ?2, ?3, NULL)",
                params![url, now, old_hash],
            )
            .map_err(|e| CrawlError::storage(e.to_string()))?;
            events.push(ChangeEvent {
                url,
                change_type: ChangeType::Removed,
                detected_at: now,
                old_hash: Some(old_hash),
                new_hash: None,
            });
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_url_is_detected_as_new_event() {
        let store = DeltaStore::open_memory().unwrap();
        let event = store
            .detect_and_record("https://example.com/", "abc123")
            .await
            .unwrap()
            .expect("first visit must produce a New event");

        assert_eq!(event.change_type, ChangeType::New);
        assert_eq!(event.url, "https://example.com/");
        assert_eq!(event.new_hash.as_deref(), Some("abc123"));
        assert!(event.old_hash.is_none());
    }

    #[tokio::test]
    async fn same_hash_produces_no_event() {
        let store = DeltaStore::open_memory().unwrap();
        store
            .detect_and_record("https://example.com/", "abc123")
            .await
            .unwrap();
        let second = store
            .detect_and_record("https://example.com/", "abc123")
            .await
            .unwrap();
        assert!(second.is_none(), "unchanged content must produce no event");
    }

    #[tokio::test]
    async fn changed_hash_produces_modified_event() {
        let store = DeltaStore::open_memory().unwrap();
        store
            .detect_and_record("https://example.com/", "hash1")
            .await
            .unwrap();
        let event = store
            .detect_and_record("https://example.com/", "hash2")
            .await
            .unwrap()
            .expect("different hash must produce a Modified event");

        assert_eq!(event.change_type, ChangeType::Modified);
        assert_eq!(event.old_hash.as_deref(), Some("hash1"));
        assert_eq!(event.new_hash.as_deref(), Some("hash2"));
    }

    #[tokio::test]
    async fn query_changes_returns_all_recorded_events() {
        let store = DeltaStore::open_memory().unwrap();
        store
            .detect_and_record("https://a.com/", "h1")
            .await
            .unwrap();
        store
            .detect_and_record("https://b.com/", "h2")
            .await
            .unwrap();
        store
            .detect_and_record("https://a.com/", "h1_changed")
            .await
            .unwrap();

        let changes = store.query_changes().unwrap();
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].change_type, ChangeType::New);
        assert_eq!(changes[1].change_type, ChangeType::New);
        assert_eq!(changes[2].change_type, ChangeType::Modified);
    }

    #[tokio::test]
    async fn detect_removed_finds_stale_urls() {
        let store = DeltaStore::open_memory().unwrap();
        // Seed two URLs with timestamps 1000 seconds in the past.
        {
            let conn = store.conn.lock().unwrap();
            let past = unix_now() - 1000;
            conn.execute(
                "INSERT INTO page_hashes (url, last_hash, updated_at) VALUES (?1, ?2, ?3)",
                params!["https://old1.com/", "oldhash1", past],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO page_hashes (url, last_hash, updated_at) VALUES (?1, ?2, ?3)",
                params!["https://old2.com/", "oldhash2", past],
            )
            .unwrap();
        }
        let crawl_start = unix_now() - 500;
        let removed = store.detect_removed(crawl_start).await.unwrap();

        assert_eq!(removed.len(), 2);
        assert!(removed.iter().all(|e| e.change_type == ChangeType::Removed));
        assert!(removed[0].old_hash.is_some());
    }

    #[test]
    fn hash_html_is_deterministic_and_correct_length() {
        let h1 = hash_html("<html>hello</html>");
        let h2 = hash_html("<html>hello</html>");
        let h3 = hash_html("<html>world</html>");
        assert_eq!(h1, h2, "same content must produce same hash");
        assert_ne!(h1, h3, "different content must produce different hash");
        assert_eq!(h1.len(), 64, "SHA-256 hex is always 64 characters");
    }
}
