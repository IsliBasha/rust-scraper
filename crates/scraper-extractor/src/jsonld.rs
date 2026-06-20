use scraper::{Html, Selector};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

fn ld_selector() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| {
        Selector::parse(r#"script[type="application/ld+json"]"#)
            .expect("hardcoded selector is valid")
    })
}

/// Extracts structured data from all `<script type="application/ld+json">` blocks.
///
/// Top-level scalar fields (string, number, bool) are returned with a `ld_` prefix.
/// `@context` is dropped; `@type` becomes `ld_type`.
/// Nested objects and arrays are serialised as JSON strings.
/// If a key appears in multiple blocks the last value wins.
pub fn extract_jsonld(html: &str) -> BTreeMap<String, Value> {
    let document = Html::parse_document(html);
    let mut out = BTreeMap::new();

    for el in document.select(ld_selector()) {
        let text: String = el.text().collect();
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for (key, val) in map {
            if key == "@context" {
                continue;
            }
            let field = if let Some(rest) = key.strip_prefix('@') {
                format!("ld_{rest}")
            } else {
                format!("ld_{key}")
            };
            let stored = match &val {
                Value::String(_) | Value::Number(_) | Value::Bool(_) => val,
                Value::Null => continue,
                _ => Value::String(val.to_string()),
            };
            out.insert(field, stored);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_product_name_and_type() {
        let html = r#"<html><head>
            <script type="application/ld+json">
            {"@context":"https://schema.org","@type":"Product","name":"Blue Widget","sku":"W-42"}
            </script>
        </head><body></body></html>"#;
        let fields = extract_jsonld(html);
        assert_eq!(fields["ld_type"], Value::String("Product".into()));
        assert_eq!(fields["ld_name"], Value::String("Blue Widget".into()));
        assert_eq!(fields["ld_sku"], Value::String("W-42".into()));
        assert!(
            !fields.contains_key("ld_context"),
            "@context must be dropped"
        );
    }

    #[test]
    fn numeric_price_preserved_as_number() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"@type":"Product","price":9.99}</script>
        </head></html>"#;
        let fields = extract_jsonld(html);
        assert!(fields["ld_price"].is_number());
    }

    #[test]
    fn nested_object_serialised_as_json_string() {
        let html = r#"<html><head>
            <script type="application/ld+json">
            {"@type":"Product","offers":{"price":"5.00","priceCurrency":"EUR"}}
            </script>
        </head></html>"#;
        let fields = extract_jsonld(html);
        let offers = fields["ld_offers"].as_str().unwrap();
        assert!(offers.contains("price"));
    }

    #[test]
    fn multiple_scripts_merged_last_wins() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"@type":"Product","name":"First"}</script>
            <script type="application/ld+json">{"@type":"Product","name":"Second"}</script>
        </head></html>"#;
        let fields = extract_jsonld(html);
        assert_eq!(fields["ld_name"], Value::String("Second".into()));
    }

    #[test]
    fn invalid_json_block_is_silently_skipped() {
        let html = r#"<html><head>
            <script type="application/ld+json">NOT VALID JSON</script>
            <script type="application/ld+json">{"@type":"Article","headline":"OK"}</script>
        </head></html>"#;
        let fields = extract_jsonld(html);
        assert_eq!(fields["ld_type"], Value::String("Article".into()));
        assert_eq!(fields["ld_headline"], Value::String("OK".into()));
    }

    #[test]
    fn no_ld_script_returns_empty_map() {
        let fields = extract_jsonld("<html><body>nothing</body></html>");
        assert!(fields.is_empty());
    }

    #[test]
    fn null_values_omitted() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"@type":"Product","discontinued":null,"name":"X"}</script>
        </head></html>"#;
        let fields = extract_jsonld(html);
        assert!(!fields.contains_key("ld_discontinued"));
        assert!(fields.contains_key("ld_name"));
    }
}
