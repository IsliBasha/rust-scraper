use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use scraper_core::CrawlError;
use std::path::Path;

use crate::config::CrawlConfig;

/// Load config from a TOML file, overridden by `SCRAPER_` environment variables.
///
/// If `path` is `None`, only environment variables are applied on top of defaults.
pub fn load(path: Option<&Path>) -> Result<CrawlConfig, CrawlError> {
    let mut figment = Figment::new();

    if let Some(p) = path {
        figment = figment.merge(Toml::file(p));
    }

    figment = figment.merge(Env::prefixed("SCRAPER_").split("__"));

    figment
        .extract::<CrawlConfig>()
        .map_err(|e| CrawlError::config(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_defaults_when_no_file() {
        let cfg = load(None).unwrap();
        assert_eq!(cfg.max_depth, 3);
    }
}
