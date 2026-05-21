pub mod css;

#[cfg(feature = "xpath")]
pub mod xpath;

use scraper_core::{CrawlError, ExtractedData, ExtractionRule, SelectorEngine, SelectorKind, Url};
use std::collections::BTreeMap;

pub use css::CssEngine;

#[cfg(feature = "xpath")]
pub use xpath::XPathEngine;

/// Dispatches to CssEngine or XPathEngine based on rule selector kind.
pub struct CompositeEngine {
    css: CssEngine,
    #[cfg(feature = "xpath")]
    xpath: XPathEngine,
}

impl Default for CompositeEngine {
    fn default() -> Self {
        Self {
            css: CssEngine,
            #[cfg(feature = "xpath")]
            xpath: XPathEngine,
        }
    }
}

impl CompositeEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SelectorEngine for CompositeEngine {
    fn extract(
        &self,
        html: &str,
        base_url: &Url,
        rules: &[ExtractionRule],
    ) -> Result<ExtractedData, CrawlError> {
        let css_rules: Vec<&ExtractionRule> =
            rules.iter().filter(|r| self.css.supports(&r.selector)).collect();

        #[cfg(feature = "xpath")]
        let xpath_rules: Vec<&ExtractionRule> =
            rules.iter().filter(|r| self.xpath.supports(&r.selector)).collect();

        let mut merged = BTreeMap::new();
        let mut links = Vec::new();

        if !css_rules.is_empty() {
            let css_slice: Vec<ExtractionRule> = css_rules.iter().map(|r| (*r).clone()).collect();
            let result = self.css.extract(html, base_url, &css_slice)?;
            merged.extend(result.fields);
            links.extend(result.discovered_links);
        }

        #[cfg(feature = "xpath")]
        if !xpath_rules.is_empty() {
            let xp_slice: Vec<ExtractionRule> = xpath_rules.iter().map(|r| (*r).clone()).collect();
            let result = self.xpath.extract(html, base_url, &xp_slice)?;
            merged.extend(result.fields);
            // links already captured from CSS pass (same HTML)
        }

        // If no rules matched, run CSS engine for link discovery alone
        if rules.is_empty() {
            let result = self.css.extract(html, base_url, &[])?;
            links.extend(result.discovered_links);
        }

        Ok(ExtractedData {
            url: base_url.clone(),
            fields: merged,
            discovered_links: links,
        })
    }

    fn supports(&self, kind: &SelectorKind) -> bool {
        match kind {
            SelectorKind::Css(_) => true,
            #[cfg(feature = "xpath")]
            SelectorKind::XPath(_) => true,
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }
}
