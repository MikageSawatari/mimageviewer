//! External URL validation and opening helpers.
//!
//! Metadata can be user-provided, so only browser-safe HTTP(S) URLs are allowed
//! through this module.

const MAX_EXTERNAL_URL_LEN: usize = 4096;

pub fn normalize_http_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim_matches(|c: char| c.is_whitespace() || c == '<' || c == '>');
    if trimmed.is_empty() || trimmed.len() > MAX_EXTERNAL_URL_LEN {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    let (scheme, rest) = trimmed.split_once(':')?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return None;
    }
    if !rest.starts_with("//") || rest.len() <= 2 {
        return None;
    }
    Some(trimmed.to_string())
}

pub fn is_http_url(raw: &str) -> bool {
    normalize_http_url(raw).is_some()
}

pub fn open_url(raw: &str) -> Result<(), String> {
    let url = normalize_http_url(raw).ok_or_else(|| "unsafe or unsupported URL".to_string())?;
    opener::open(&url).map_err(|e| format!("open URL failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https_urls() {
        assert_eq!(
            normalize_http_url(" https://youtu.be/abc?x=1 ").as_deref(),
            Some("https://youtu.be/abc?x=1")
        );
        assert_eq!(
            normalize_http_url("HTTP://example.com/path").as_deref(),
            Some("HTTP://example.com/path")
        );
    }

    #[test]
    fn rejects_non_browser_or_control_urls() {
        assert!(normalize_http_url("javascript:alert(1)").is_none());
        assert!(normalize_http_url("file:///C:/Windows/win.ini").is_none());
        assert!(normalize_http_url("https://example.com/\ncalc").is_none());
        assert!(normalize_http_url("https://").is_none());
    }
}
