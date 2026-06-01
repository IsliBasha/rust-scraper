/// Returns `true` if `url` matches `pattern`.
///
/// `*` is a wildcard that matches any run of characters (including `/`).
/// Patterns with no wildcard are treated as a substring check.
///
/// Examples:
/// - `"*/products/*"` matches `"https://shop.com/products/widget"`
/// - `"https://example.com/blog/*"` matches `"https://example.com/blog/post"`
/// - `"*"` matches everything
pub fn url_matches_pattern(url: &str, pattern: &str) -> bool {
    if !pattern.contains('*') {
        return url.contains(pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let n = parts.len();
    let trailing_anchored = !pattern.ends_with('*');
    let mut remaining = url;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let is_first = i == 0 && !pattern.starts_with('*');
        let is_last = trailing_anchored && i == n - 1;

        if is_first {
            if !remaining.starts_with(*part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if is_last {
            return remaining.ends_with(*part);
        } else {
            match remaining.find(*part) {
                Some(pos) => remaining = &remaining[pos + part.len()..],
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_only_matches_everything() {
        assert!(url_matches_pattern("https://example.com/any/path", "*"));
    }

    #[test]
    fn leading_and_trailing_wildcard_matches_middle_segment() {
        let pattern = "*/products/*";
        assert!(url_matches_pattern(
            "https://shop.com/products/widget",
            pattern
        ));
        assert!(url_matches_pattern(
            "https://shop.com/en/products/widget-42",
            pattern
        ));
        assert!(!url_matches_pattern("https://shop.com/blog/post", pattern));
    }

    #[test]
    fn prefix_wildcard_matches_any_host() {
        let pattern = "*/blog/*";
        assert!(url_matches_pattern("https://a.com/blog/post", pattern));
        assert!(url_matches_pattern("https://b.org/blog/entry", pattern));
        assert!(!url_matches_pattern("https://a.com/news/article", pattern));
    }

    #[test]
    fn no_wildcard_is_substring_match() {
        assert!(url_matches_pattern("https://example.com/page", "/page"));
        assert!(url_matches_pattern("https://example.com/page", "example.com"));
        assert!(!url_matches_pattern("https://example.com/page", "other.com"));
    }

    #[test]
    fn exact_prefix_with_trailing_wildcard() {
        let pattern = "https://example.com/*";
        assert!(url_matches_pattern("https://example.com/foo", pattern));
        assert!(url_matches_pattern("https://example.com/foo/bar", pattern));
        assert!(!url_matches_pattern("https://other.com/foo", pattern));
    }

}
