//! External URL validation and opening helpers.
//!
//! Metadata can be user-provided, so only browser-safe HTTP(S) URLs are allowed
//! through this module.

const MAX_EXTERNAL_URL_LEN: usize = 4096;

pub(crate) fn google_maps_url(gps: crate::exif_reader::GpsCoordinates) -> String {
    // Fixed precision is locale-independent. Normalize rounded negative zero too.
    let decimal = |value: f64| {
        let text = format!("{value:.7}");
        if text == "-0.0000000" {
            "0.0000000".to_string()
        } else {
            text
        }
    };
    format!(
        "https://www.google.com/maps/search/?api=1&query={}%2C{}",
        decimal(gps.latitude),
        decimal(gps.longitude),
    )
}

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
    fn google_maps_url_orders_and_formats_coordinates() {
        use crate::exif_reader::GpsCoordinates;
        assert_eq!(
            google_maps_url(GpsCoordinates {
                latitude: -35.123456789,
                longitude: -139.75
            }),
            "https://www.google.com/maps/search/?api=1&query=-35.1234568%2C-139.7500000"
        );
        assert_eq!(
            google_maps_url(GpsCoordinates {
                latitude: -0.000000001,
                longitude: -0.0
            }),
            "https://www.google.com/maps/search/?api=1&query=0.0000000%2C0.0000000"
        );
        assert_eq!(
            google_maps_url(GpsCoordinates {
                latitude: 90.0,
                longitude: 180.0
            }),
            "https://www.google.com/maps/search/?api=1&query=90.0000000%2C180.0000000"
        );
    }

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
