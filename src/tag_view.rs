//! タグビュー (Ctrl+T) の状態と検索 worker。
//!
//! `tags.db` の検索と `fs::metadata` によるパス種別判定は worker 側で行い、
//! UI スレッドでは結果を通常グリッドへ反映するだけにする。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::archive_converter::ArchiveFormat;
use crate::tags_db::TagSummary;

pub const TAG_VIEW_RESULT_LIMIT: usize = 10_000;

#[derive(Default)]
pub(crate) struct TagViewState {
    pub active: bool,
    pub query: String,
    pub last_executed: String,
    pub focus_request: bool,
    pub has_focus: bool,
    pub saved_folder: Option<PathBuf>,
    pub results_paths: Vec<PathBuf>,
    pub summaries: Vec<TagSummary>,
    pub result_count: usize,
    pub truncated: bool,
    pub reject_message: Option<String>,
}

impl TagViewState {
    pub fn on_results_grid(&self) -> bool {
        self.active && !self.last_executed.trim().is_empty()
    }
}

pub(crate) struct TagViewPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<TagViewResult, String>>,
}

impl TagViewPending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn try_recv(&self) -> Result<Result<TagViewResult, String>, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TagViewResult {
    pub query: String,
    pub summaries: Vec<TagSummary>,
    pub entries: Vec<TagViewEntry>,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct TagViewEntry {
    pub path: PathBuf,
    pub kind: TagViewItemKind,
    pub mtime: i64,
    pub file_size: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagViewItemKind {
    Folder,
    Image,
    Video,
    ZipFile,
    PdfFile,
    Archive(ArchiveFormat),
}

pub(crate) fn spawn_tag_view_search(data_dir: PathBuf, query: String) -> TagViewPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tag-view-search".to_string())
        .spawn(move || {
            let result = run_tag_view_search(&data_dir, &query, &cancel_worker);
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(result);
            }
        })
        .ok();
    TagViewPending { cancel, rx }
}

fn run_tag_view_search(
    data_dir: &Path,
    query: &str,
    cancel: &AtomicBool,
) -> Result<TagViewResult, String> {
    let db_path = data_dir.join("tags.db");
    let mut db = crate::tags_db::TagsDb::open_at(&db_path)
        .map_err(|e| format!("タグDBを開けません: {e}"))?;
    let summaries = db.tag_summaries();
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(TagViewResult {
            query: query.to_string(),
            summaries,
            entries: Vec::new(),
            truncated: false,
        });
    }

    let exact_keys = db.item_keys_by_tag_exact(trimmed, TAG_VIEW_RESULT_LIMIT + 1);
    let keys = if exact_keys.is_empty() {
        db.item_keys_by_tag_prefix(trimmed, TAG_VIEW_RESULT_LIMIT + 1)
    } else {
        exact_keys
    };
    let truncated = keys.len() > TAG_VIEW_RESULT_LIMIT;
    let mut entries = Vec::with_capacity(keys.len().min(TAG_VIEW_RESULT_LIMIT));
    let mut prune_keys = Vec::new();
    for key in keys.into_iter().take(TAG_VIEW_RESULT_LIMIT) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(TagViewResult {
                query: query.to_string(),
                summaries,
                entries,
                truncated,
            });
        }
        let path = PathBuf::from(&key);
        match classify_tag_view_path(path) {
            ClassifiedTagViewPath::Existing(entry) => entries.push(entry),
            ClassifiedTagViewPath::Missing(path) => {
                if should_prune_missing_path(&path) {
                    prune_keys.push(key);
                }
            }
        }
    }
    if !prune_keys.is_empty() && !cancel.load(Ordering::Relaxed) {
        match db.prune_items(&prune_keys) {
            Ok(removed) => crate::logger::log(format!(
                "tag_view: pruned {} stale item(s), removed {} tag row(s)",
                prune_keys.len(),
                removed
            )),
            Err(e) => crate::logger::log(format!("tag_view: stale item prune failed: {e}")),
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(TagViewResult {
        query: query.to_string(),
        summaries,
        entries,
        truncated,
    })
}

enum ClassifiedTagViewPath {
    Existing(TagViewEntry),
    Missing(PathBuf),
}

fn classify_tag_view_path(path: PathBuf) -> ClassifiedTagViewPath {
    let Ok(meta) = std::fs::metadata(&path) else {
        return ClassifiedTagViewPath::Missing(path);
    };
    let mtime = crate::ui_helpers::mtime_secs(&meta);
    let file_size = if meta.is_file() { meta.len() as i64 } else { 0 };
    let kind = if meta.is_dir() {
        TagViewItemKind::Folder
    } else {
        classify_file_kind(&path)
    };
    ClassifiedTagViewPath::Existing(TagViewEntry {
        path,
        kind,
        mtime,
        file_size,
    })
}

fn should_prune_missing_path(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    if !matches!(path.try_exists(), Ok(false)) {
        return false;
    }
    path.ancestors()
        .skip(1)
        .any(|ancestor| !ancestor.as_os_str().is_empty() && ancestor.is_dir())
}

fn classify_file_kind(path: &Path) -> TagViewItemKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if crate::folder_tree::is_recognized_image_ext(&ext) {
        TagViewItemKind::Image
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        TagViewItemKind::Video
    } else if crate::folder_tree::is_zip_extension(&ext) {
        TagViewItemKind::ZipFile
    } else if ext == "pdf" {
        TagViewItemKind::PdfFile
    } else if let Some(format) = ArchiveFormat::from_extension(&ext) {
        TagViewItemKind::Archive(format)
    } else {
        TagViewItemKind::Folder
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn tag_view_search_returns_tagged_existing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let image = temp.path().join("image.jpg");
        let folder = temp.path().join("folder");
        let other = temp.path().join("other.jpg");
        std::fs::write(&image, b"jpg").unwrap();
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&other, b"jpg").unwrap();

        let image_key = crate::tags_db::item_key_for_path(&image);
        let folder_key = crate::tags_db::item_key_for_path(&folder);
        let other_key = crate::tags_db::item_key_for_path(&other);
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(&image_key, ["Cat"], "test").unwrap();
        db.set_item_tags(&folder_key, ["catnap"], "test").unwrap();
        db.set_item_tags(&other_key, ["dog"], "test").unwrap();
        drop(db);

        let result = run_tag_view_search(&data_dir, "#cat", &AtomicBool::new(false)).unwrap();

        assert_eq!(result.query, "#cat");
        assert_eq!(result.entries.len(), 1);
        assert!(!result.truncated);
        assert!(result.summaries.iter().any(|s| s.tag_key == "cat"));
        assert!(
            result
                .entries
                .iter()
                .any(|e| e.path == PathBuf::from(&image_key) && e.kind == TagViewItemKind::Image)
        );
        assert!(
            result
                .entries
                .iter()
                .all(|e| e.path != PathBuf::from(&folder_key))
        );
        assert!(
            !result
                .entries
                .iter()
                .any(|e| e.path == PathBuf::from(&other_key))
        );

        let prefix_result = run_tag_view_search(&data_dir, "#ca", &AtomicBool::new(false)).unwrap();
        assert_eq!(prefix_result.entries.len(), 2);
        assert!(
            prefix_result
                .entries
                .iter()
                .any(|e| e.path == PathBuf::from(&folder_key) && e.kind == TagViewItemKind::Folder)
        );
    }

    #[test]
    fn tag_view_search_hides_and_prunes_reachable_missing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let image = temp.path().join("image.jpg");
        let missing = temp.path().join("missing.jpg");
        std::fs::write(&image, b"jpg").unwrap();

        let image_key = crate::tags_db::item_key_for_path(&image);
        let missing_key = crate::tags_db::item_key_for_path(&missing);
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(&image_key, ["cat"], "test").unwrap();
        db.set_item_tags(&missing_key, ["cat"], "test").unwrap();
        drop(db);

        let result = run_tag_view_search(&data_dir, "#cat", &AtomicBool::new(false)).unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].path, PathBuf::from(&image_key));

        let db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        assert!(db.display_tags_for_item(&missing_key).is_empty());
        assert!(!db.has_item_state(&missing_key));
    }
}
