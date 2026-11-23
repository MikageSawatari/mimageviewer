//! Browser output folders open with an unremembered single-page presentation.
//! Classification reads accepted source facts and cached paths only; no I/O.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static DEFAULT_OUTPUT_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Shared spelling for startup policy and worker-side browser registration.
pub(crate) fn output_root_from_pictures(pictures: &Path) -> PathBuf {
    pictures.join("mimageviewer").join("browser")
}

/// Called by run() before either the UI or Remote readers start.
pub(crate) fn initialize_default_root() {
    capture_default_root_once(&DEFAULT_OUTPUT_ROOT, || {
        let pictures = crate::known_folders::pictures_dir_unverified();
        if pictures.is_none() {
            crate::logger::log("browser view: could not resolve the default output root");
        }
        pictures
    });
}

fn capture_default_root_once(
    cache: &OnceLock<Option<PathBuf>>,
    resolve_pictures: impl FnOnce() -> Option<PathBuf>,
) {
    cache.get_or_init(|| resolve_pictures().as_deref().map(output_root_from_pictures));
}

pub(crate) fn is_browser_view_directory(
    path: &Path,
    is_directory: bool,
    explicit_root: Option<&Path>,
) -> bool {
    is_directory_under_root(
        path,
        is_directory,
        explicit_root,
        DEFAULT_OUTPUT_ROOT.get().and_then(|root| root.as_deref()),
    )
}

/// Case-insensitive component comparison, without stat, canonicalization, or
/// book classification. An explicit root replaces the cached default root.
pub(crate) fn is_directory_under_root(
    path: &Path,
    is_directory: bool,
    explicit_root: Option<&Path>,
    default_root: Option<&Path>,
) -> bool {
    let Some(root) = explicit_root.or(default_root) else {
        return false;
    };
    if !is_directory || root.as_os_str().is_empty() {
        return false;
    }
    let mut path_components = path.components();
    root.components().all(|root_component| {
        path_components.next().is_some_and(|component| {
            crate::path_key::normalize_keep_drive(Path::new(component.as_os_str()))
                == crate::path_key::normalize_keep_drive(Path::new(root_component.as_os_str()))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser870_root_comparison_uses_components_and_directory_fact() {
        let root = Path::new("C:/Pictures/mimageviewer/browser");
        for path in [
            "C:/Pictures/mimageviewer/browser",
            "C:/Pictures/mimageviewer/browser/2026-11",
            "C:/Pictures/mimageviewer/browser/2026-11/child",
            "c:/PICTURES/MIMAGEVIEWER/BROWSER/2026-11",
        ] {
            assert!(
                is_directory_under_root(Path::new(path), true, Some(root), None),
                "{path}"
            );
            assert!(!is_directory_under_root(
                Path::new(path),
                false,
                Some(root),
                None
            ));
        }
        for path in [
            "C:/Pictures/mimageviewer/browser2",
            "C:/Pictures/mimageviewer/browser2/2026-11",
            "C:/elsewhere/browser",
            "D:/Pictures/mimageviewer/browser",
        ] {
            assert!(
                !is_directory_under_root(Path::new(path), true, Some(root), None),
                "{path}"
            );
        }
    }

    #[test]
    fn browser870_default_root_is_captured_once_and_only_cached_value_is_read() {
        let cache = OnceLock::new();
        capture_default_root_once(&cache, || Some(PathBuf::from("C:/redirected-pictures")));
        capture_default_root_once(&cache, || panic!("must not query twice"));
        let root = cache.get().unwrap().as_deref();
        let path = Path::new("C:/redirected-pictures/mimageviewer/browser/month");
        assert!(is_directory_under_root(path, true, None, root));
        assert!(!is_directory_under_root(
            path,
            true,
            Some(Path::new("C:/other")),
            root
        ));
    }

    #[test]
    fn browser870_failed_default_root_resolution_only_allows_explicit_root() {
        let cache = OnceLock::new();
        capture_default_root_once(&cache, || None);
        capture_default_root_once(&cache, || panic!("failed resolution is also final"));
        let path = Path::new("C:/Pictures/mimageviewer/browser");
        let root = cache.get().unwrap().as_deref();
        assert!(!is_directory_under_root(path, true, None, root));
        assert!(is_directory_under_root(path, true, Some(path), root));
    }
}
