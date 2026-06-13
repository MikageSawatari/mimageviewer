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
const TAG_VIEW_FILTERED_KEY_SCAN_LIMIT: usize = 50_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TagViewKindFilter {
    #[default]
    All,
    Folder,
    Image,
    Video,
    ZipFile,
    PdfFile,
    Archive,
}

pub(crate) const TAG_VIEW_KIND_FILTER_CHOICES: &[TagViewKindFilter] = &[
    TagViewKindFilter::All,
    TagViewKindFilter::Image,
    TagViewKindFilter::Video,
    TagViewKindFilter::Folder,
    TagViewKindFilter::ZipFile,
    TagViewKindFilter::PdfFile,
    TagViewKindFilter::Archive,
];

impl TagViewKindFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            TagViewKindFilter::All => "すべての種類",
            TagViewKindFilter::Folder => "フォルダ",
            TagViewKindFilter::Image => "画像",
            TagViewKindFilter::Video => "動画",
            TagViewKindFilter::ZipFile => "ZIP ファイル",
            TagViewKindFilter::PdfFile => "PDF ファイル",
            TagViewKindFilter::Archive => "アーカイブ",
        }
    }

    fn matches(self, kind: TagViewItemKind) -> bool {
        match self {
            TagViewKindFilter::All => true,
            TagViewKindFilter::Folder => matches!(kind, TagViewItemKind::Folder),
            TagViewKindFilter::Image => matches!(kind, TagViewItemKind::Image),
            TagViewKindFilter::Video => matches!(kind, TagViewItemKind::Video),
            TagViewKindFilter::ZipFile => matches!(kind, TagViewItemKind::ZipFile),
            TagViewKindFilter::PdfFile => matches!(kind, TagViewItemKind::PdfFile),
            TagViewKindFilter::Archive => matches!(kind, TagViewItemKind::Archive(_)),
        }
    }
}

#[derive(Default)]
pub(crate) struct TagViewState {
    pub active: bool,
    pub query: String,
    pub last_executed: String,
    pub kind_filter: TagViewKindFilter,
    pub last_executed_kind_filter: TagViewKindFilter,
    pub focus_request: bool,
    pub has_focus: bool,
    pub saved_folder: Option<PathBuf>,
    pub nav_stack: Vec<PathBuf>,
    // NOTE: favsearch と違い「結果パスの保持」フィールドは持たない。result_count が
    // 件数表示の正本で、ナビは通常グリッドの items を直接使う (write-only の
    // Vec<PathBuf> clone を 10k 件分払っていた dead state を v1.4.0 レビューで削除)。
    pub summaries: Vec<TagSummary>,
    pub result_count: usize,
    pub truncated: bool,
    pub reject_message: Option<String>,
}

impl TagViewState {
    /// 「今まさに結果グリッド (またはタグブラウザ) を表示中」か。
    ///
    /// `FavSearchState::on_results_grid` と**同じ意味論** (`nav_stack.is_empty()`) に
    /// 揃えること。結果からフォルダ/コンテナへドリルインしたら false になり、
    /// Ctrl+V ペースト・外部 D&D・エクスポート再読込のゲートが実フォルダで再び
    /// 有効になる (共有ゲートのコメント「検索から実フォルダを開いた後は有効に戻る」
    /// は Ctrl+G/S と共通の不変条件)。旧実装は「クエリを一度でも実行したか」で、
    /// ドリルイン後も true のまま貼り付けが拒否され続ける非対称があった。
    pub fn on_results_grid(&self) -> bool {
        self.active && self.nav_stack.is_empty()
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
    pub kind_filter: TagViewKindFilter,
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

pub(crate) fn spawn_tag_view_search(
    data_dir: PathBuf,
    query: String,
    kind_filter: TagViewKindFilter,
) -> TagViewPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tag-view-search".to_string())
        .spawn(move || {
            let result = run_tag_view_search(&data_dir, &query, kind_filter, &cancel_worker);
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
    kind_filter: TagViewKindFilter,
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
            kind_filter,
            summaries,
            entries: Vec::new(),
            truncated: false,
        });
    }

    let scan_limit = if kind_filter == TagViewKindFilter::All {
        TAG_VIEW_RESULT_LIMIT
    } else {
        TAG_VIEW_FILTERED_KEY_SCAN_LIMIT
    };
    let terms = parse_tag_view_query_terms(trimmed);
    let exact_keys = db.item_keys_by_tag_exact(trimmed, scan_limit + 1);
    let keys = if terms.len() > 1 && exact_keys.is_empty() {
        db.item_keys_by_tag_terms_and(&terms, scan_limit + 1)
    } else if exact_keys.is_empty() {
        db.item_keys_by_tag_prefix(trimmed, scan_limit + 1)
    } else {
        exact_keys
    };
    let scan_truncated = keys.len() > scan_limit;
    let mut truncated = false;
    let mut entries = Vec::with_capacity(keys.len().min(TAG_VIEW_RESULT_LIMIT));
    let mut prune_keys = Vec::new();
    for key in keys.into_iter().take(scan_limit) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(TagViewResult {
                query: query.to_string(),
                kind_filter,
                summaries,
                entries,
                truncated,
            });
        }
        let path = PathBuf::from(&key);
        match classify_tag_view_path(path) {
            ClassifiedTagViewPath::Existing(entry) => {
                if kind_filter.matches(entry.kind) {
                    if entries.len() >= TAG_VIEW_RESULT_LIMIT {
                        truncated = true;
                        break;
                    }
                    entries.push(entry);
                }
            }
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
    if scan_truncated {
        truncated = true;
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(TagViewResult {
        query: query.to_string(),
        kind_filter,
        summaries,
        entries,
        truncated,
    })
}

fn parse_tag_view_query_terms(query: &str) -> Vec<String> {
    dedup_tag_terms(
        query
            .split_whitespace()
            .map(crate::tags_db::normalize_tag_display_name)
            .collect(),
    )
}

fn dedup_tag_terms(terms: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for term in terms {
        let key = crate::tags_db::normalize_tag_key(&term);
        if key.is_empty() || seen.iter().any(|existing| existing == &key) {
            continue;
        }
        seen.push(key);
        out.push(term);
    }
    out
}

enum ClassifiedTagViewPath {
    Existing(TagViewEntry),
    Missing(PathBuf),
}

fn classify_tag_view_path(path: PathBuf) -> ClassifiedTagViewPath {
    match std::fs::metadata(&path) {
        Ok(meta) => existing_tag_view_entry(restore_real_casing(path), meta),
        Err(_) => {
            // 大文字小文字を区別するディレクトリ (DevDrive / WSL の case-sensitive
            // フラグ) では、小文字正規化された item_key の metadata が**実在ファイル
            // でも**失敗する。そのまま Missing 扱いにすると prune が実在ファイルの
            // タグを恒久削除してしまうため、親ディレクトリを 1 回だけ走査して
            // 大小無視一致を探す (見つかれば実 casing の Existing として扱う)。
            if let Some(real) = find_case_insensitive_sibling(&path)
                && let Ok(meta) = std::fs::metadata(&real)
            {
                return existing_tag_view_entry(real, meta);
            }
            ClassifiedTagViewPath::Missing(path)
        }
    }
}

fn existing_tag_view_entry(path: PathBuf, meta: std::fs::Metadata) -> ClassifiedTagViewPath {
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

/// 表示用に実ディスク上の casing を復元する。
///
/// tags.db の item_key は小文字正規化されているため、キーから直接 GridItem を作ると
/// セル名・ツールチップ・クリップボードまで全小文字になる (Ctrl+G/S は実 casing)。
/// `canonicalize` は Windows で実 casing を返すので、`\\?\` プレフィックスを剥がした
/// 上で**大小・区切り以外が変わらない場合のみ**採用する (シンボリックリンク解決で
/// 別パスになった場合は item_key とずれてタグ操作が空振りするため、元のキーを保つ)。
fn restore_real_casing(path: PathBuf) -> PathBuf {
    let Ok(canon) = std::fs::canonicalize(&path) else {
        return path;
    };
    let s = canon.to_string_lossy();
    let stripped: PathBuf = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        canon
    };
    if crate::adjustment_db::normalize_path(&stripped)
        == crate::adjustment_db::normalize_path(&path)
    {
        stripped
    } else {
        path
    }
}

/// 親ディレクトリを走査して、ファイル名が大小無視で一致するエントリを探す。
/// 祖先側も casing がずれている full case-sensitive 環境は対象外 (1 階層のみ)。
fn find_case_insensitive_sibling(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let wanted = path.file_name()?.to_string_lossy().to_lowercase();
    for entry in std::fs::read_dir(parent).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.file_name().to_string_lossy().to_lowercase() == wanted {
            return Some(entry.path());
        }
    }
    None
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

        let result = run_tag_view_search(
            &data_dir,
            "#cat",
            TagViewKindFilter::All,
            &AtomicBool::new(false),
        )
        .unwrap();

        // entry.path は実 casing 復元済みなので、小文字キーとの比較は item_key 正規化で行う
        let entry_key = |e: &TagViewEntry| crate::tags_db::item_key_for_path(&e.path);
        assert_eq!(result.query, "#cat");
        assert_eq!(result.entries.len(), 1);
        assert!(!result.truncated);
        assert!(result.summaries.iter().any(|s| s.tag_key == "cat"));
        assert!(
            result
                .entries
                .iter()
                .any(|e| entry_key(e) == image_key && e.kind == TagViewItemKind::Image)
        );
        assert!(result.entries.iter().all(|e| entry_key(e) != folder_key));
        assert!(!result.entries.iter().any(|e| entry_key(e) == other_key));

        let prefix_result = run_tag_view_search(
            &data_dir,
            "#ca",
            TagViewKindFilter::All,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(prefix_result.entries.len(), 2);
        assert!(
            prefix_result
                .entries
                .iter()
                .any(|e| entry_key(e) == folder_key && e.kind == TagViewItemKind::Folder)
        );
    }

    #[test]
    fn tag_view_search_intersects_multiple_tags() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let cat_dog = temp.path().join("cat-dog.jpg");
        let cat_only = temp.path().join("cat-only.jpg");
        let dog_only = temp.path().join("dog-only.jpg");
        let cat_dognap = temp.path().join("cat-dognap.jpg");
        std::fs::write(&cat_dog, b"jpg").unwrap();
        std::fs::write(&cat_only, b"jpg").unwrap();
        std::fs::write(&dog_only, b"jpg").unwrap();
        std::fs::write(&cat_dognap, b"jpg").unwrap();

        let cat_dog_key = crate::tags_db::item_key_for_path(&cat_dog);
        let cat_dognap_key = crate::tags_db::item_key_for_path(&cat_dognap);
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(&cat_dog_key, ["cat", "dog"], "test")
            .unwrap();
        db.set_item_tags(
            &crate::tags_db::item_key_for_path(&cat_only),
            ["cat"],
            "test",
        )
        .unwrap();
        db.set_item_tags(
            &crate::tags_db::item_key_for_path(&dog_only),
            ["dog"],
            "test",
        )
        .unwrap();
        db.set_item_tags(&cat_dognap_key, ["cat", "dognap"], "test")
            .unwrap();
        drop(db);

        let entry_key = |e: &TagViewEntry| crate::tags_db::item_key_for_path(&e.path);
        let exact_result = run_tag_view_search(
            &data_dir,
            "#cat #dog",
            TagViewKindFilter::All,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            exact_result
                .entries
                .iter()
                .map(entry_key)
                .collect::<Vec<_>>(),
            vec![cat_dog_key.clone()]
        );

        let prefix_result = run_tag_view_search(
            &data_dir,
            "#cat #do",
            TagViewKindFilter::All,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            prefix_result
                .entries
                .iter()
                .map(entry_key)
                .collect::<Vec<_>>(),
            vec![cat_dog_key, cat_dognap_key]
        );
    }

    #[test]
    fn tag_view_query_terms_are_whitespace_separated() {
        assert_eq!(
            parse_tag_view_query_terms("#cat #dog"),
            vec!["cat".to_string(), "dog".to_string()]
        );
        assert_eq!(
            parse_tag_view_query_terms("#cat dog"),
            vec!["cat".to_string(), "dog".to_string()]
        );
        assert_eq!(
            parse_tag_view_query_terms("cat cat #dog"),
            vec!["cat".to_string(), "dog".to_string()]
        );
    }

    /// 結果の path は tags.db の小文字キーではなく**実ディスク上の casing** を持つ。
    /// (小文字のままだとセル名・コピー・外部連携が全小文字になる UX 退行 +
    ///  case-sensitive ディレクトリで prune がタグを誤削除する)
    #[test]
    fn tag_view_results_restore_real_path_casing() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let image = temp.path().join("MixedCase.JPG");
        std::fs::write(&image, b"jpg").unwrap();
        let image_key = crate::tags_db::item_key_for_path(&image);
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(&image_key, ["cat"], "test").unwrap();
        drop(db);

        let result = run_tag_view_search(
            &data_dir,
            "#cat",
            TagViewKindFilter::All,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(
            result.entries[0].path.file_name().unwrap().to_str(),
            Some("MixedCase.JPG"),
            "実 casing が復元される"
        );
        // タグ操作の同一性: 復元後パスからも同じ item_key が導出される
        assert_eq!(
            crate::tags_db::item_key_for_path(&result.entries[0].path),
            image_key
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

        let result = run_tag_view_search(
            &data_dir,
            "#cat",
            TagViewKindFilter::All,
            &AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(
            crate::tags_db::item_key_for_path(&result.entries[0].path),
            image_key
        );

        let db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        assert!(db.display_tags_for_item(&missing_key).is_empty());
        assert!(!db.has_item_state(&missing_key));
    }

    #[test]
    fn tag_view_kind_filter_keeps_only_matching_item_kinds() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let image = temp.path().join("image.jpg");
        let video = temp.path().join("video.mp4");
        let zip = temp.path().join("book.zip");
        std::fs::write(&image, b"jpg").unwrap();
        std::fs::write(&video, b"mp4").unwrap();
        std::fs::write(&zip, b"zip").unwrap();

        let image_key = crate::tags_db::item_key_for_path(&image);
        let video_key = crate::tags_db::item_key_for_path(&video);
        let zip_key = crate::tags_db::item_key_for_path(&zip);
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(&image_key, ["cat"], "test").unwrap();
        db.set_item_tags(&video_key, ["cat"], "test").unwrap();
        db.set_item_tags(&zip_key, ["cat"], "test").unwrap();
        drop(db);

        let result = run_tag_view_search(
            &data_dir,
            "#cat",
            TagViewKindFilter::Video,
            &AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(result.kind_filter, TagViewKindFilter::Video);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(
            crate::tags_db::item_key_for_path(&result.entries[0].path),
            video_key
        );
        assert_eq!(result.entries[0].kind, TagViewItemKind::Video);
    }
}
