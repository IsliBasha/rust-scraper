use scraper::{Html, Selector};
use scraper_core::{CrawlError, ExtractedData, ExtractionRule, SelectorEngine, SelectorKind, Url};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct CssEngine;

impl SelectorEngine for CssEngine {
    fn extract(
        &self,
        html: &str,
        base_url: &Url,
        rules: &[ExtractionRule],
    ) -> Result<ExtractedData, CrawlError> {
        let document = Html::parse_document(html);
        let mut fields = BTreeMap::new();

        // Always collect discovered links from <a href="...">
        let mut discovered_links = Vec::new();
        if let Ok(a_sel) = Selector::parse("a[href]") {
            for el in document.select(&a_sel) {
                if let Some(href) = el.value().attr("href") {
                    if let Ok(url) = base_url.join(href) {
                        discovered_links.push(url);
                    }
                }
            }
        }

        for rule in rules {
            let SelectorKind::Css(ref css) = rule.selector else {
                continue;
            };

            let selector = Selector::parse(css).map_err(|e| {
                CrawlError::extraction(format!("invalid CSS selector '{css}': {e:?}"))
            })?;

            let values: Vec<Value> = document
                .select(&selector)
                .map(|el| {
                    if let Some(ref attr) = rule.attr {
                        el.value()
                            .attr(attr)
                            .map(|v| Value::String(v.to_string()))
                            .unwrap_or(Value::Null)
                    } else {
                        let text: String = el.text().collect::<Vec<_>>().join(" ");
                        Value::String(text.split_whitespace().collect::<Vec<_>>().join(" "))
                    }
                })
                .collect();

            let value = if rule.many {
                Value::Array(values)
            } else {
                values.into_iter().next().unwrap_or(Value::Null)
            };

            fields.insert(rule.name.clone(), value);
        }

        Ok(ExtractedData {
            url: base_url.clone(),
            fields,
            discovered_links,
        })
    }

    fn supports(&self, kind: &SelectorKind) -> bool {
        matches!(kind, SelectorKind::Css(_))
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
    fn extracts_single_text_field() {
        let engine = CssEngine;
        let html = r#"<html><body><h1 class="title">Hello</h1></body></html>"#;
        let rules = vec![ExtractionRule {
            name: "title".into(),
            selector: SelectorKind::Css(".title".into()),
            attr: None,
            many: false,
        }];
        let result = engine.extract(html, &url("https://example.com"), &rules).unwrap();
        assert_eq!(result.fields["title"], Value::String("Hello".into()));
    }

    #[test]
    fn extracts_attribute_value() {
        let engine = CssEngine;
        let html = r#"<html><body><a href="https://example.com/page">link</a></body></html>"#;
        let rules = vec![ExtractionRule {
            name: "link".into(),
            selector: SelectorKind::Css("a".into()),
            attr: Some("href".into()),
            many: false,
        }];
        let result = engine.extract(html, &url("https://example.com"), &rules).unwrap();
        assert_eq!(
            result.fields["link"],
            Value::String("https://example.com/page".into())
        );
    }

    #[test]
    fn extracts_many_values_as_array() {
        let engine = CssEngine;
        let html = r#"<html><body><li>one</li><li>two</li><li>three</li></body></html>"#;
        let rules = vec![ExtractionRule {
            name: "items".into(),
            selector: SelectorKind::Css("li".into()),
            attr: None,
            many: true,
        }];
        let result = engine.extract(html, &url("https://example.com"), &rules).unwrap();
        assert_eq!(
            result.fields["items"],
            Value::Array(vec![
                Value::String("one".into()),
                Value::String("two".into()),
                Value::String("three".into()),
            ])
        );
    }

    #[test]
    fn discovers_links_from_anchor_tags() {
        let engine = CssEngine;
        let html = r#"<html><body>
            <a href="/about">About</a>
            <a href="https://other.com">External</a>
        </body></html>"#;
        let result = engine.extract(html, &url("https://example.com"), &[]).unwrap();
        assert_eq!(result.discovered_links.len(), 2);
        assert!(result
            .discovered_links
            .iter()
            .any(|u| u.as_str() == "https://example.com/about"));
    }

    #[test]
    fn returns_null_when_selector_has_no_match() {
        let engine = CssEngine;
        let html = r#"<html><body></body></html>"#;
        let rules = vec![ExtractionRule {
            name: "missing".into(),
            selector: SelectorKind::Css(".nonexistent".into()),
            attr: None,
            many: false,
        }];
        let result = engine.extract(html, &url("https://example.com"), &rules).unwrap();
        assert_eq!(result.fields["missing"], Value::Null);
    }

    #[test]
    fn returns_extraction_error_for_invalid_selector() {
        let engine = CssEngine;
        let rules = vec![ExtractionRule {
            name: "bad".into(),
            selector: SelectorKind::Css("!!!invalid!!!".into()),
            attr: None,
            many: false,
        }];
        let result = engine.extract("<html></html>", &url("https://example.com"), &rules);
        assert!(result.is_err());
    }
}
