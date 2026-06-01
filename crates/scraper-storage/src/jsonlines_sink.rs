use async_trait::async_trait;
use scraper_core::{CrawlError, ExtractedData, ResultSink};
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    sync::Mutex,
};

/// Writes extracted data to a newline-delimited JSON (`.jsonl`) file.
///
/// Each record is serialised as a single JSON object on its own line.
/// The file is opened in append mode so restarts do not overwrite prior output.
pub struct JsonLinesSink {
    writer: Mutex<BufWriter<File>>,
}

impl JsonLinesSink {
    pub fn create(path: &str) -> Result<Self, CrawlError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
        })
    }
}

#[async_trait]
impl ResultSink for JsonLinesSink {
    async fn write(&self, data: &ExtractedData) -> Result<(), CrawlError> {
        let line =
            serde_json::to_string(data).map_err(|e| CrawlError::storage(e.to_string()))?;
        let mut guard = self.writer.lock().unwrap();
        writeln!(guard, "{line}").map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(())
    }

    async fn flush(&self) -> Result<(), CrawlError> {
        let mut guard = self.writer.lock().unwrap();
        guard
            .flush()
            .map_err(|e| CrawlError::storage(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper_core::{ExtractedData, Url};
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn make_record(url: &str, title: &str) -> ExtractedData {
        let mut fields = BTreeMap::new();
        fields.insert("title".into(), Value::String(title.into()));
        ExtractedData {
            url: Url::parse(url).unwrap(),
            fields,
            discovered_links: vec![],
            content_hash: None,
        }
    }

    fn temp_path(suffix: &str) -> String {
        format!(
            "/tmp/jsonlines_test_{suffix}_{}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        )
    }

    #[tokio::test]
    async fn creates_file_and_writes_records() {
        let path = temp_path("write");
        let sink = JsonLinesSink::create(&path).unwrap();

        for i in 0..3 {
            let rec = make_record(
                &format!("https://example.com/{i}"),
                &format!("Title {i}"),
            );
            sink.write(&rec).await.unwrap();
        }
        sink.flush().await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn each_line_is_valid_json_with_url_field() {
        let path = temp_path("valid_json");
        let sink = JsonLinesSink::create(&path).unwrap();

        sink.write(&make_record("https://example.com/a", "A"))
            .await
            .unwrap();
        sink.flush().await.unwrap();

        let line = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(line.trim()).expect("must be valid JSON");
        assert_eq!(v["url"], Value::String("https://example.com/a".into()));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn append_mode_preserves_previous_records() {
        let path = temp_path("append");
        {
            let sink = JsonLinesSink::create(&path).unwrap();
            sink.write(&make_record("https://example.com/1", "First"))
                .await
                .unwrap();
            sink.flush().await.unwrap();
        }
        {
            let sink = JsonLinesSink::create(&path).unwrap();
            sink.write(&make_record("https://example.com/2", "Second"))
                .await
                .unwrap();
            sink.flush().await.unwrap();
        }

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2, "both sessions must be present");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn fields_serialised_into_json_object() {
        let path = temp_path("fields");
        let sink = JsonLinesSink::create(&path).unwrap();

        let mut fields = BTreeMap::new();
        fields.insert("price".into(), Value::Number(9.into()));
        fields.insert("name".into(), Value::String("Widget".into()));

        let rec = ExtractedData {
            url: Url::parse("https://example.com/widget").unwrap(),
            fields,
            discovered_links: vec![],
            content_hash: None,
        };
        sink.write(&rec).await.unwrap();
        sink.flush().await.unwrap();

        let line = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["fields"]["name"], Value::String("Widget".into()));
        let _ = std::fs::remove_file(&path);
    }
}
