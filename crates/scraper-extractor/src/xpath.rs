use scraper_core::{CrawlError, ExtractedData, ExtractionRule, SelectorEngine, SelectorKind, Url};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct XPathEngine;

impl SelectorEngine for XPathEngine {
    fn extract(
        &self,
        html: &str,
        base_url: &Url,
        rules: &[ExtractionRule],
    ) -> Result<ExtractedData, CrawlError> {
        use skyscraper::html;
        use skyscraper::xpath;

        let document = html::parse(html)
            .map_err(|e| CrawlError::extraction(format!("HTML parse error: {e}")))?;

        let mut fields = BTreeMap::new();

        for rule in rules {
            let SelectorKind::XPath(ref expr) = rule.selector else {
                continue;
            };

            let xp = xpath::parse(expr)
                .map_err(|e| CrawlError::extraction(format!("invalid XPath '{expr}': {e}")))?;

            let nodes = xp.apply(&document).map_err(|e| {
                CrawlError::extraction(format!("XPath eval error for '{expr}': {e}"))
            })?;

            let values: Vec<Value> = nodes
                .iter()
                .map(|node| {
                    let text = node.get_all_text(&document).unwrap_or_default();
                    Value::String(text)
                })
                .collect();

            let value = if rule.many {
                Value::Array(values)
            } else {
                values.into_iter().next().unwrap_or(Value::Null)
            };

            fields.insert(rule.name.clone(), value);
        }

        // Link discovery is handled by CompositeEngine via CssEngine; XPathEngine skips it.
        Ok(ExtractedData {
            url: base_url.clone(),
            fields,
            discovered_links: Vec::new(),
        })
    }

    fn supports(&self, kind: &SelectorKind) -> bool {
        matches!(kind, SelectorKind::XPath(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper_core::{ExtractionRule, SelectorKind};

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn extracts_text_via_xpath() {
        let engine = XPathEngine;
        let html = r#"<html><body><h1 id="title">Hello XPath</h1></body></html>"#;
        let rules = vec![ExtractionRule {
            name: "title".into(),
            selector: SelectorKind::XPath("//h1[@id='title']".into()),
            attr: None,
            many: false,
        }];
        let result = engine
            .extract(html, &url("https://example.com"), &rules)
            .unwrap();
        let title = result.fields["title"].as_str().unwrap();
        assert!(title.contains("Hello XPath"), "got: {title}");
    }

    #[test]
    fn collects_many_nodes_as_array() {
        let engine = XPathEngine;
        let html = r#"<html><body><li>a</li><li>b</li></body></html>"#;
        let rules = vec![ExtractionRule {
            name: "items".into(),
            selector: SelectorKind::XPath("//li".into()),
            attr: None,
            many: true,
        }];
        let result = engine
            .extract(html, &url("https://example.com"), &rules)
            .unwrap();
        assert!(result.fields["items"].is_array());
        assert_eq!(result.fields["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn returns_null_for_no_match() {
        let engine = XPathEngine;
        let rules = vec![ExtractionRule {
            name: "nothing".into(),
            selector: SelectorKind::XPath("//div[@class='nonexistent']".into()),
            attr: None,
            many: false,
        }];
        let result = engine
            .extract(
                "<html><body></body></html>",
                &url("https://example.com"),
                &rules,
            )
            .unwrap();
        assert_eq!(result.fields["nothing"], Value::Null);
    }
}
