//! Retained archive handles during source-folder mutations.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

type FenceKey = Vec<String>;
static FENCES: Mutex<Vec<(u64, Vec<FenceKey>)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn key(path: &Path) -> Option<FenceKey> {
    let spelling = crate::path_key::physical_path_spelling(path)?;
    Some(
        spelling
            .trim_end_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect(),
    )
}

pub(crate) struct FenceGuard {
    id: Option<u64>,
}

impl Drop for FenceGuard {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            FENCES.lock().unwrap().retain(|(active, _)| *active != id);
        }
    }
}

/// Call on the Shell operation's worker, after its final kind is known.
pub(crate) fn begin(folders: &[PathBuf]) -> FenceGuard {
    begin_with_releases(folders, || {
        crate::zip_loader::retire_fenced_subtrees();
        crate::pdf_loader::release_fenced_documents();
    })
}

pub(crate) fn begin_with_releases(folders: &[PathBuf], release: impl FnOnce()) -> FenceGuard {
    #[cfg(test)]
    if BYPASS.with(std::cell::Cell::get) {
        return FenceGuard { id: None };
    }
    let keys: Vec<_> = folders
        .iter()
        .filter_map(|folder| {
            let key = key(folder);
            if key.is_none() {
                crate::logger::log(format!("mutation_fence: cannot spell folder {folder:?}"));
            }
            key
        })
        .collect();
    if keys.is_empty() {
        return FenceGuard { id: None };
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    FENCES.lock().unwrap().push((id, keys));
    let guard = FenceGuard { id: Some(id) };
    // Register before taking cache locks; never hold the registry during releases.
    release();
    guard
}

pub(crate) fn covers(path: &Path) -> bool {
    let Some(path) = key(path) else { return false };
    FENCES.lock().unwrap().iter().any(|(_, keys)| {
        keys.iter()
            .any(|folder| path.len() > folder.len() && path.starts_with(folder))
    })
}

#[cfg(test)]
thread_local! {
    static BYPASS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn without_fence_for_test<T>(run: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            BYPASS.with(|value| value.set(self.0));
        }
    }
    let _restore = Restore(BYPASS.with(|value| value.replace(true)));
    run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_858b_covers_physical_components_and_spellings() {
        let roots = [
            PathBuf::from(r"C:\fence_858b_a\"),
            PathBuf::from(r"\\server\share\fence_858b_unc\"),
        ];
        let guard = begin_with_releases(&roots, || {
            assert!(covers(Path::new(r"C:\fence_858b_a\book.zip")));
        });
        for path in [
            r"C:\fence_858b_a\book.zip",
            "c:/FENCE_858B_A/sub/book.pdf",
            r"\\?\C:\fence_858b_a\book.zip",
            r"\\?\UNC\SERVER\SHARE\fence_858b_unc\book.pdf",
            r"\\server\share\fence_858b_unc\sub\",
        ] {
            assert!(covers(Path::new(path)), "{path}");
        }
        for path in [
            r"C:\fence_858b_ab\book.zip",
            r"C:\fence_858b_a",
            r"C:\fence_858b_a\",
            r"D:\fence_858b_a\book.zip",
            r"\\server\share\fence_858b_unc",
            r"\\?\Volume{bad}\book.zip",
        ] {
            assert!(!covers(Path::new(path)), "{path}");
        }
        drop(guard);
        assert!(!covers(Path::new(r"C:\fence_858b_a\book.zip")));
    }

    #[test]
    fn fence_858b_multiple_guards_drop_independently() {
        let folder = PathBuf::from(r"C:\fence_858b_multiple");
        let child = folder.join("book.pdf");
        let first = begin_with_releases(std::slice::from_ref(&folder), || {});
        let second = begin_with_releases(&[folder], || {});
        drop(first);
        assert!(covers(&child));
        drop(second);
        assert!(!covers(&child));
    }

    #[test]
    fn fence_858b_empty_and_unspellable_do_not_release() {
        let release = || panic!("no registered folders");
        let _empty = begin_with_releases(&[], release);
        let _unspellable = begin_with_releases(&[PathBuf::from(r"\\?\Volume{bad}\")], release);
    }
}
