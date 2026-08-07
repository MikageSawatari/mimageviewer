/// Whether a file below `web/` belongs to the browser distribution.
///
/// The build script walks regular files recursively. Tests and Node package metadata are useful
/// from the source tree but are not browser runtime assets; source maps are development-only and
/// can be large. Licenses and version markers intentionally remain distribution assets.
pub(crate) fn is_distribution_asset(relative_path: &str) -> bool {
    let Some(file_name) = relative_path.rsplit('/').next() else {
        return false;
    };
    !file_name.ends_with(".test.mjs")
        && file_name != "package.json"
        && file_name != "package-lock.json"
        && !file_name.ends_with(".map")
}

/// The single Content-Type table for every generated or disk-served web asset.
///
/// Adding a distribution asset with an unsupported extension fails in `build.rs`, so a new type
/// cannot silently reach browsers as an arbitrary binary response.
pub(crate) fn content_type(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.')?.1;
    match extension {
        "html" => Some("text/html; charset=utf-8"),
        "js" | "mjs" => Some("text/javascript; charset=utf-8"),
        "css" => Some("text/css; charset=utf-8"),
        "webmanifest" => Some("application/manifest+json; charset=utf-8"),
        "png" => Some("image/png"),
        "txt" => Some("text/plain; charset=utf-8"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_filter_excludes_only_development_artifacts() {
        for path in [
            "app-runtime.test.mjs",
            "nested/player.test.mjs",
            "package.json",
            "package-lock.json",
            "vendor/hls.min.js.map",
        ] {
            assert!(!is_distribution_asset(path), "{path}");
        }
        for path in [
            "app.js",
            "icons/icon-192.png",
            "vendor/hls.LICENSE.txt",
            "vendor/hls.VERSION.txt",
        ] {
            assert!(is_distribution_asset(path), "{path}");
        }
    }

    #[test]
    fn content_types_have_one_explicit_mapping() {
        assert_eq!(content_type("index.html"), Some("text/html; charset=utf-8"));
        assert_eq!(
            content_type("video-stream.mjs"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(content_type("icons/icon-192.png"), Some("image/png"));
        assert_eq!(
            content_type("manifest.webmanifest"),
            Some("application/manifest+json; charset=utf-8")
        );
        assert_eq!(content_type("unsupported.bin"), None);
    }
}
