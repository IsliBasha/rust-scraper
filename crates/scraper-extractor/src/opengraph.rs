use scraper::{Html, Selector};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

fn og_selector() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| {
        Selector::parse(r#"meta[property^="og:"]"#).expect("hardcoded selector is valid")
    })
}

/// Extracts Open Graph `<meta property="og:*" content="...">` tags.
///
/// Property names are normalised: `og:title` → field key `og_title`.
/// Only tags with a non-empty `content` attribute are included.
pub fn extract_og(html: &str) -> BTreeMap<String, Value> {
    let document = Html::parse_document(html);
    let mut out = BTreeMap::new();

    for el in document.select(og_selector()) {
        let attrs = el.value();
        let Some(property) = attrs.attr("property") else {
            continue;
        };
        let Some(content) = attrs.attr("content") else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        // "og:title" → "og_title"
        let key = property.replace(':', "_");
        out.insert(key, Value::String(content.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_title_and_description() {
        let html = r#"<html><head>
            <meta property="og:title" content="My Product">
            <meta property="og:description" content="A great widget">
        </head><body></body></html>"#;
        let fields = extract_og(html);
        assert_eq!(fields["og_title"], Value::String("My Product".into()));
        assert_eq!(
            fields["og_description"],
            Value::String("A great widget".into())
        );
    }

    #[test]
    fn extracts_image_and_type() {
        let html = r#"<html><head>
            <meta property="og:image" content="https://example.com/img.jpg">
            <meta property="og:type" content="product">
        </head></html>"#;
        let fields = extract_og(html);
        assert_eq!(
            fields["og_image"],
            Value::String("https://example.com/img.jpg".into())
        );
        assert_eq!(fields["og_type"], Value::String("product".into()));
    }

    #[test]
    fn empty_content_attribute_is_skipped() {
        let html = r#"<html><head>
            <meta property="og:title" content="">
            <meta property="og:description" content="Present">
        </head></html>"#;
        let fields = extract_og(html);
        assert!(!fields.contains_key("og_title"));
        assert!(fields.contains_key("og_description"));
    }

    #[test]
    fn no_og_tags_returns_empty_map() {
        let fields = extract_og("<html><head><title>Nothing</title></head></html>");
        assert!(fields.is_empty());
    }

    #[test]
    fn colon_in_property_name_replaced_with_underscore() {
        let html = r#"<html><head>
            <meta property="og:site_name" content="Acme Store">
        </head></html>"#;
        let fields = extract_og(html);
        assert!(fields.contains_key("og_site_name"));
    }
}
