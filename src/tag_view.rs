//! タグビュー (Ctrl+T) の状態と検索 worker。
//!
//! `tags.db` の検索と `fs::metadata` によるパス種別判定は worker 側で行い、
//! UI スレッドでは結果を通常グリッドへ反映するだけにする。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::archive_converter::ArchiveFormat;
use crate::settings::TagDef;
use crate::tags_db::TagSummary;

pub const TAG_VIEW_RESULT_LIMIT: usize = 10_000;
pub(crate) const TAG_VIEW_FILTERED_KEY_SCAN_LIMIT: usize = 50_000;
const TAG_VIEW_RECENT_LIMIT: usize = 20;
const TAG_VIEW_POPULAR_LIMIT: usize = 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TagViewSort {
    #[default]
    PathOrder,
    AppliedAtDesc,
    AppliedAtAsc,
}

impl TagViewSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::PathOrder => "パス順",
            Self::AppliedAtDesc => "タグ付与時刻（新しい順）",
            Self::AppliedAtAsc => "タグ付与時刻（古い順）",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::PathOrder => "パス順",
            Self::AppliedAtDesc => "タグ時刻↓",
            Self::AppliedAtAsc => "タグ時刻↑",
        }
    }

    pub fn arranges_by_category(self) -> bool {
        // 従来のパス順も、カテゴリで再配置しない固定順。
        false
    }

    fn query_order(self) -> crate::tags_db::TagViewQueryOrder {
        match self {
            Self::PathOrder => crate::tags_db::TagViewQueryOrder::Path,
            Self::AppliedAtDesc => crate::tags_db::TagViewQueryOrder::AppliedAtDesc,
            Self::AppliedAtAsc => crate::tags_db::TagViewQueryOrder::AppliedAtAsc,
        }
    }
}

/// PC and Remote inspect the same bounded candidate set when policy projection can remove
/// rows. The visible response cap is applied after classification and filtering.
pub(crate) fn key_scan_limit_for_projection(
    all_kinds: bool,
    visibility: &crate::file_type_visibility::FileTypeVisibility,
) -> usize {
    if all_kinds && visibility.is_all_visible() {
        TAG_VIEW_RESULT_LIMIT
    } else {
        TAG_VIEW_FILTERED_KEY_SCAN_LIMIT
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagViewMenuChoice {
    pub name: String,
    pub tag_key: String,
    pub count: usize,
}

/// 本体タグビューとリモート閲覧で共有するタグブラウザ 3 区画の正本。
///
/// 入力はタグ一覧と設定上のタグ定義だけで、App の他状態には依存しない。
pub(crate) fn tag_view_menu_sections(
    summaries: &[TagSummary],
    tag_defs: &[TagDef],
) -> (
    Vec<TagViewMenuChoice>,
    Vec<TagViewMenuChoice>,
    Vec<TagViewMenuChoice>,
) {
    let summary_by_key: HashMap<&str, &TagSummary> = summaries
        .iter()
        .map(|summary| (summary.tag_key.as_str(), summary))
        .collect();
    let display_names: HashMap<&str, &str> = tag_defs
        .iter()
        .map(|tag| (tag.tag_key.as_str(), tag.name.as_str()))
        .collect();
    let mut excluded: HashSet<String> = HashSet::new();

    let mut pinned = Vec::new();
    for tag in tag_defs.iter().filter(|tag| tag.show_shortcut) {
        if tag.tag_key.is_empty() || tag.name.trim().is_empty() {
            continue;
        }
        let count = summary_by_key
            .get(tag.tag_key.as_str())
            .map_or(0, |summary| summary.count);
        pinned.push(TagViewMenuChoice {
            name: tag.name.clone(),
            tag_key: tag.tag_key.clone(),
            count,
        });
        excluded.insert(tag.tag_key.clone());
    }
    pinned.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let choice_for_summary = |summary: &TagSummary| -> TagViewMenuChoice {
        let name = display_names
            .get(summary.tag_key.as_str())
            .copied()
            .unwrap_or(summary.tag.as_str())
            .to_string();
        TagViewMenuChoice {
            name,
            tag_key: summary.tag_key.clone(),
            count: summary.count,
        }
    };

    let mut recent_source: Vec<_> = summaries.iter().collect();
    recent_source.sort_by(|a, b| {
        b.last_applied_at
            .cmp(&a.last_applied_at)
            .then_with(|| a.tag.to_lowercase().cmp(&b.tag.to_lowercase()))
    });
    let mut recent = Vec::new();
    for summary in recent_source {
        if !excluded.insert(summary.tag_key.clone()) {
            continue;
        }
        recent.push(choice_for_summary(summary));
        if recent.len() >= TAG_VIEW_RECENT_LIMIT {
            break;
        }
    }

    let mut popular_source: Vec<_> = summaries.iter().collect();
    popular_source.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.tag.to_lowercase().cmp(&b.tag.to_lowercase()))
    });
    let mut popular = Vec::new();
    for summary in popular_source {
        if !excluded.insert(summary.tag_key.clone()) {
            continue;
        }
        popular.push(choice_for_summary(summary));
        if popular.len() >= TAG_VIEW_POPULAR_LIMIT {
            break;
        }
    }

    (pinned, recent, popular)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TagViewKindFilter {
    #[default]
    All,
    Folder,
    Image,
    Video,
    Audio,
    ZipFile,
    PdfFile,
    Archive,
}

pub(crate) const TAG_VIEW_KIND_FILTER_CHOICES: &[TagViewKindFilter] = &[
    TagViewKindFilter::All,
    TagViewKindFilter::Image,
    TagViewKindFilter::Video,
    TagViewKindFilter::Audio,
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
            TagViewKindFilter::Audio => "音声",
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
            TagViewKindFilter::Audio => matches!(kind, TagViewItemKind::Audio),
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
    /// プロセス内だけで保持する session-local 選択。Settings / DB には永続化しない。
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
    pub(crate) video_thumb_overrides: std::collections::HashMap<String, PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct TagViewEntry {
    pub path: PathBuf,
    pub kind: TagViewItemKind,
    pub mtime: i64,
    pub file_size: i64,
    /// Query-matching tag rows' MAX timestamp, converted from DB seconds to ms.
    /// Shared classification by itself leaves this unset.
    pub applied_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagViewItemKind {
    Folder,
    Image,
    Video,
    Audio,
    ZipFile,
    PdfFile,
    Archive(ArchiveFormat),
}

pub(crate) fn spawn_tag_view_search(
    data_dir: PathBuf,
    query: String,
    kind_filter: TagViewKindFilter,
    settings: crate::settings::Settings,
) -> TagViewPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tag-view-search".to_string())
        .spawn(move || {
            let result = run_tag_view_search_with_visibility(
                &data_dir,
                &query,
                kind_filter,
                settings.tag_view_sort,
                &settings.file_type_visibility,
                &cancel_worker,
            )
            .and_then(|mut result| {
                let media_paths = result
                    .entries
                    .iter()
                    .filter(|entry| {
                        matches!(entry.kind, TagViewItemKind::Video | TagViewItemKind::Audio)
                    })
                    .map(|entry| entry.path.clone())
                    .collect::<Vec<_>>();
                let sidecars = crate::app::folder_scan::discover_aggregate_video_sidecars_while(
                    &settings,
                    &media_paths,
                    64,
                    || !cancel_worker.load(Ordering::Relaxed),
                )
                .ok_or_else(|| "cancelled".to_string())?;
                if sidecars.skipped_parents > 0 {
                    crate::logger::log(format!(
                        "tag sidecar discovery capped: scanned={} skipped={}",
                        sidecars.scanned_parents, sidecars.skipped_parents
                    ));
                }
                for (parent, error) in &sidecars.scan_errors {
                    crate::logger::log(format!(
                        "tag sidecar discovery failed {}: {error}",
                        parent.display()
                    ));
                }
                result.video_thumb_overrides = sidecars.by_video_path;
                Ok(result)
            });
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(result);
            }
        })
        .ok();
    TagViewPending { cancel, rx }
}

#[cfg(test)]
fn run_tag_view_search(
    data_dir: &Path,
    query: &str,
    kind_filter: TagViewKindFilter,
    cancel: &AtomicBool,
) -> Result<TagViewResult, String> {
    run_tag_view_search_with_visibility(
        data_dir,
        query,
        kind_filter,
        TagViewSort::PathOrder,
        &crate::file_type_visibility::FileTypeVisibility::default(),
        cancel,
    )
}

fn run_tag_view_search_with_visibility(
    data_dir: &Path,
    query: &str,
    kind_filter: TagViewKindFilter,
    sort: TagViewSort,
    visibility: &crate::file_type_visibility::FileTypeVisibility,
    cancel: &AtomicBool,
) -> Result<TagViewResult, String> {
    let db_path = data_dir.join("tags.db");
    let db = crate::tags_db::TagsDb::open_at(&db_path)
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
            video_thumb_overrides: Default::default(),
        });
    }

    let scan_limit =
        key_scan_limit_for_projection(kind_filter == TagViewKindFilter::All, visibility);
    let matches = select_tag_view_matches(&db, trimmed, sort, scan_limit + 1);
    let scan_truncated = matches.len() > scan_limit;
    let mut truncated = false;
    let mut entries = Vec::with_capacity(matches.len().min(TAG_VIEW_RESULT_LIMIT));
    for matched in matches.into_iter().take(scan_limit) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(TagViewResult {
                query: query.to_string(),
                kind_filter,
                summaries,
                entries,
                truncated,
                video_thumb_overrides: Default::default(),
            });
        }
        let path = PathBuf::from(&matched.item_key);
        match classify_tag_view_path(path) {
            ClassifiedTagViewPath::Existing(classified) => {
                let mut entry = classified.entry;
                entry.applied_at_ms = matched.applied_at.checked_mul(1000);
                // Classification already owns the stat result. A directory named *.zip is
                // a structural row, even if the ZIP file category is disabled.
                if (classified.is_directory || visibility.allows_physical_file(&entry.path))
                    && kind_filter.matches(entry.kind)
                {
                    if entries.len() >= TAG_VIEW_RESULT_LIMIT {
                        truncated = true;
                        break;
                    }
                    entries.push(entry);
                }
            }
            // missing は外付け切断 / NAS offline でも起きる。検索結果から隠すだけで、
            // tags.db は絶対に変更しない。
            ClassifiedTagViewPath::Missing => {}
        }
    }
    if scan_truncated {
        truncated = true;
    }
    entries.sort_by(|a, b| {
        let time_order = match (sort, a.applied_at_ms, b.applied_at_ms) {
            (TagViewSort::PathOrder, _, _) => std::cmp::Ordering::Equal,
            (TagViewSort::AppliedAtDesc, Some(a), Some(b)) => b.cmp(&a),
            (TagViewSort::AppliedAtAsc, Some(a), Some(b)) => a.cmp(&b),
            (_, Some(_), None) => std::cmp::Ordering::Less,
            (_, None, Some(_)) => std::cmp::Ordering::Greater,
            (_, None, None) => std::cmp::Ordering::Equal,
        };
        time_order.then_with(|| a.path.cmp(&b.path))
    });
    Ok(TagViewResult {
        query: query.to_string(),
        kind_filter,
        summaries,
        entries,
        truncated,
        video_thumb_overrides: Default::default(),
    })
}

fn select_tag_view_matches(
    db: &crate::tags_db::TagsDb,
    query: &str,
    sort: TagViewSort,
    limit: usize,
) -> Vec<crate::tags_db::TagViewMatch> {
    let terms = parse_tag_view_query_terms(query);
    let order = sort.query_order();
    let exact = db.tag_view_matches_exact(query, order, limit);
    if terms.len() > 1 && exact.is_empty() {
        db.tag_view_matches_terms_and(&terms, order, limit)
    } else if exact.is_empty() {
        db.tag_view_matches_prefix(query, order, limit)
    } else {
        exact
    }
}

/// 本体タグビューとリモート閲覧で共有する item_key 選択規則の正本。
///
/// 完全一致を優先し、複数語かつ完全一致なしなら語の AND、最後に接頭辞へ進む。
pub(crate) fn select_tag_view_item_keys(
    db: &crate::tags_db::TagsDb,
    query: &str,
    limit: usize,
) -> Vec<String> {
    let terms = parse_tag_view_query_terms(query);
    let exact_keys = db.item_keys_by_tag_exact(query, limit);
    if terms.len() > 1 && exact_keys.is_empty() {
        db.item_keys_by_tag_terms_and(&terms, limit)
    } else if exact_keys.is_empty() {
        db.item_keys_by_tag_prefix(query, limit)
    } else {
        exact_keys
    }
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

pub(crate) struct ClassifiedTagViewEntry {
    pub entry: TagViewEntry,
    /// `TagViewItemKind::Folder` の既存 fallback と実フォルダを区別する。
    pub is_directory: bool,
}

pub(crate) enum ClassifiedTagViewPath {
    Existing(ClassifiedTagViewEntry),
    Missing,
}

pub(crate) fn classify_tag_view_path(path: PathBuf) -> ClassifiedTagViewPath {
    match std::fs::metadata(&path) {
        Ok(meta) => existing_tag_view_entry(
            crate::path_key::restore_existing_path_casing(&path).unwrap_or(path),
            meta,
        ),
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
            ClassifiedTagViewPath::Missing
        }
    }
}

fn existing_tag_view_entry(path: PathBuf, meta: std::fs::Metadata) -> ClassifiedTagViewPath {
    let mtime = crate::ui_helpers::mtime_secs(&meta);
    let file_size = if meta.is_file() { meta.len() as i64 } else { 0 };
    let is_directory = meta.is_dir();
    let kind = if is_directory {
        TagViewItemKind::Folder
    } else {
        classify_file_kind(&path)
    };
    ClassifiedTagViewPath::Existing(ClassifiedTagViewEntry {
        entry: TagViewEntry {
            path,
            kind,
            mtime,
            file_size,
            applied_at_ms: None,
        },
        is_directory,
    })
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
    } else if crate::folder_tree::is_audio_ext(&ext) {
        // 音声は Folder fallback に落とさず、結果適用側で音楽ビューへ接続する。
        // (review-v2.3.0 追補7: R1 第2波 P2-2)
        TagViewItemKind::Audio
    } else if crate::folder_tree::is_zip_extension(&ext) {
        TagViewItemKind::ZipFile
    } else if crate::folder_tree::is_paged_document_path(path) {
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
    fn file_type_projection_keeps_structural_directories_and_tag_data() {
        use crate::file_type_visibility::{FileTypeCategory, FileTypeVisibility};
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("visible.jpg");
        let hidden = temp.path().join("hidden.png");
        let directory = temp.path().join("passage.zip");
        std::fs::write(&image, b"image").unwrap();
        std::fs::write(&hidden, b"image").unwrap();
        std::fs::create_dir(&directory).unwrap();
        let mut db = crate::tags_db::TagsDb::open_at(&temp.path().join("tags.db")).unwrap();
        for path in [&image, &hidden, &directory] {
            db.set_item_tags(
                &crate::tags_db::item_key_for_path(path),
                ["visibility"],
                "test",
            )
            .unwrap();
        }
        drop(db);
        let mut visibility = FileTypeVisibility::default();
        visibility.excluded_extensions.insert("png".into());
        visibility.excluded_categories.insert(FileTypeCategory::Zip);
        let result = run_tag_view_search_with_visibility(
            temp.path(),
            "visibility",
            TagViewKindFilter::All,
            TagViewSort::PathOrder,
            &visibility,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.entries.len(), 2);
        assert!(result.entries.iter().any(|entry| entry.path == directory));
        assert!(result.entries.iter().any(|entry| entry.path == image));
        let db = crate::tags_db::TagsDb::open_at(&temp.path().join("tags.db")).unwrap();
        assert_eq!(
            db.display_tags_for_item(&crate::tags_db::item_key_for_path(&hidden)),
            ["#visibility"]
        );
    }

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

    #[test]
    fn timestamp_queries_preserve_shared_remote_key_selection() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = crate::tags_db::TagsDb::open_at(&temp.path().join("tags.db")).unwrap();
        for (item, tags) in [
            ("a", vec!["cat", "dog"]),
            ("b", vec!["cat", "dognap"]),
            ("c", vec!["catnap"]),
            ("d", vec!["unrelated"]),
        ] {
            db.set_item_tags(item, tags, "test").unwrap();
        }
        for query in ["#cat", "#cat #dog", "#cat #do", "#ca", "#missing"] {
            let remote_keys = select_tag_view_item_keys(&db, query, 20);
            let path_keys = select_tag_view_matches(&db, query, TagViewSort::PathOrder, 20)
                .into_iter()
                .map(|row| row.item_key)
                .collect::<Vec<_>>();
            assert_eq!(path_keys, remote_keys, "{query}");
            for sort in [TagViewSort::AppliedAtDesc, TagViewSort::AppliedAtAsc] {
                let mut time_keys = select_tag_view_matches(&db, query, sort, 20)
                    .into_iter()
                    .map(|row| row.item_key)
                    .collect::<Vec<_>>();
                time_keys.sort();
                assert_eq!(time_keys, remote_keys, "{query}");
            }
            assert_eq!(select_tag_view_item_keys(&db, query, 20), remote_keys);
        }
        assert_eq!(select_tag_view_item_keys(&db, "#cat", 1), ["a"]);
    }

    #[test]
    fn tag_view_worker_converts_seconds_and_preserves_time_order_across_categories() {
        use crate::grid_item::{GridItem, materialize_view_rows};
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("z-image.jpg");
        let zip = temp.path().join("a-book.zip");
        let folder = temp.path().join("b-folder");
        std::fs::write(&image, b"image").unwrap();
        std::fs::write(&zip, b"zip").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let db_path = temp.path().join("tags.db");
        let mut db = crate::tags_db::TagsDb::open_at(&db_path).unwrap();
        for path in [&image, &zip, &folder] {
            db.set_item_tags(
                &crate::tags_db::item_key_for_path(path),
                ["cat", "unrelated"],
                "test",
            )
            .unwrap();
        }
        drop(db);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE item_tags SET applied_at = 999 WHERE tag_key = 'unrelated'",
            [],
        )
        .unwrap();
        for (path, seconds) in [(&image, 30), (&zip, 10), (&folder, 10)] {
            conn.execute(
                "UPDATE item_tags SET applied_at = ?2 WHERE item_key = ?1 AND tag_key = 'cat'",
                rusqlite::params![crate::tags_db::item_key_for_path(path), seconds],
            )
            .unwrap();
        }
        drop(conn);
        let visibility = crate::file_type_visibility::FileTypeVisibility::default();
        for (sort, expected) in [
            (
                TagViewSort::PathOrder,
                vec![zip.clone(), folder.clone(), image.clone()],
            ),
            (
                TagViewSort::AppliedAtDesc,
                vec![image.clone(), zip.clone(), folder.clone()],
            ),
            (
                TagViewSort::AppliedAtAsc,
                vec![zip.clone(), folder.clone(), image.clone()],
            ),
        ] {
            let result = run_tag_view_search_with_visibility(
                temp.path(),
                "cat",
                TagViewKindFilter::All,
                sort,
                &visibility,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(
                result
                    .entries
                    .iter()
                    .map(|e| e.path.clone())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                result
                    .entries
                    .iter()
                    .find(|e| e.path == image)
                    .unwrap()
                    .applied_at_ms,
                Some(30_000)
            );
            let mut rows = result
                .entries
                .into_iter()
                .map(|entry| {
                    let item = match entry.kind {
                        TagViewItemKind::Image => GridItem::Image(entry.path.clone()),
                        TagViewItemKind::ZipFile => GridItem::ZipFile(entry.path.clone()),
                        TagViewItemKind::Folder => GridItem::Folder(entry.path.clone()),
                        _ => unreachable!(),
                    };
                    (entry, item)
                })
                .collect::<Vec<_>>();
            let (items, metas) = materialize_view_rows(
                &mut rows,
                &crate::settings::GridDisplayOrder::default(),
                sort.arranges_by_category(),
                |row| &row.1,
                |row| Some((row.0.mtime, row.0.file_size)),
            );
            assert_eq!(items.len(), metas.len());
            if sort != TagViewSort::PathOrder {
                assert_eq!(
                    rows.iter()
                        .map(|row| row.0.path.clone())
                        .collect::<Vec<_>>(),
                    expected
                );
                assert_eq!(
                    items.iter().map(GridItem::perf_key).collect::<Vec<_>>(),
                    rows.iter().map(|row| row.1.perf_key()).collect::<Vec<_>>()
                );
            }
        }
        let ClassifiedTagViewPath::Existing(classified) = classify_tag_view_path(image) else {
            panic!("fixture exists");
        };
        assert_eq!(classified.entry.applied_at_ms, None);
        assert_eq!(TagViewSort::default(), TagViewSort::PathOrder);
        assert_eq!(TagViewSort::PathOrder.label(), "パス順");
        assert!(!TagViewSort::PathOrder.arranges_by_category());
    }

    #[test]
    fn item_key_selection_keeps_exact_then_and_then_prefix_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = crate::tags_db::TagsDb::open_at(&temp.path().join("tags.db")).unwrap();
        db.set_item_tags("exact", ["cat"], "test").unwrap();
        db.set_item_tags("prefix", ["catnap"], "test").unwrap();
        db.set_item_tags("and", ["cat", "dog"], "test").unwrap();
        db.set_item_tags("and-prefix", ["cat", "dognap"], "test")
            .unwrap();

        assert_eq!(
            select_tag_view_item_keys(&db, "#cat", 20),
            vec![
                "and".to_owned(),
                "and-prefix".to_owned(),
                "exact".to_owned()
            ]
        );
        assert_eq!(
            select_tag_view_item_keys(&db, "#cat #dog", 20),
            vec!["and".to_owned()]
        );
        assert_eq!(
            select_tag_view_item_keys(&db, "#cat #do", 20),
            vec!["and".to_owned(), "and-prefix".to_owned()]
        );
        assert_eq!(
            select_tag_view_item_keys(&db, "#catn", 20),
            vec!["prefix".to_owned()]
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
    fn tag_view_search_hides_but_preserves_missing_path_metadata() {
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
        assert_eq!(db.display_tags_for_item(&missing_key), vec!["#cat"]);
        assert!(db.has_item_state(&missing_key));
    }

    #[test]
    fn tag_view_kind_filter_keeps_only_matching_item_kinds() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let image = temp.path().join("image.jpg");
        let video = temp.path().join("video.mp4");
        let audio = temp.path().join("audio.flac");
        let zip = temp.path().join("book.zip");
        std::fs::write(&image, b"jpg").unwrap();
        std::fs::write(&video, b"mp4").unwrap();
        std::fs::write(&audio, b"flac").unwrap();
        std::fs::write(&zip, b"zip").unwrap();

        let image_key = crate::tags_db::item_key_for_path(&image);
        let video_key = crate::tags_db::item_key_for_path(&video);
        let audio_key = crate::tags_db::item_key_for_path(&audio);
        let zip_key = crate::tags_db::item_key_for_path(&zip);
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(&image_key, ["cat"], "test").unwrap();
        db.set_item_tags(&video_key, ["cat"], "test").unwrap();
        db.set_item_tags(&audio_key, ["cat"], "test").unwrap();
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

        let result = run_tag_view_search(
            &data_dir,
            "#cat",
            TagViewKindFilter::Audio,
            &AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(result.kind_filter, TagViewKindFilter::Audio);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(
            crate::tags_db::item_key_for_path(&result.entries[0].path),
            audio_key
        );
        assert_eq!(result.entries[0].kind, TagViewItemKind::Audio);
    }

    #[test]
    fn tag_view_classifies_audio_without_folder_fallback() {
        assert_eq!(
            classify_file_kind(Path::new("Book.EPUB")),
            TagViewItemKind::PdfFile
        );
        assert_eq!(
            classify_file_kind(Path::new("MixedCase.FLAC")),
            TagViewItemKind::Audio
        );
        assert!(TagViewKindFilter::All.matches(TagViewItemKind::Audio));
        assert!(TagViewKindFilter::Audio.matches(TagViewItemKind::Audio));
        assert!(!TagViewKindFilter::Video.matches(TagViewItemKind::Audio));
        assert!(!TagViewKindFilter::Folder.matches(TagViewItemKind::Audio));
        assert!(TAG_VIEW_KIND_FILTER_CHOICES.contains(&TagViewKindFilter::Audio));
        assert_eq!(TagViewKindFilter::Audio.label(), "音声");
    }
    #[test]
    fn audio_sidecar_is_prepared_by_tag_worker_without_omitting_explicit_rows() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let audio = temp.path().join("song.flac");
        let cover = temp.path().join("song.png");
        std::fs::write(&audio, b"flac").unwrap();
        std::fs::write(&cover, b"image").unwrap();
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(
            &crate::tags_db::item_key_for_path(&audio),
            ["music"],
            "test",
        )
        .unwrap();
        db.set_item_tags(
            &crate::tags_db::item_key_for_path(&cover),
            ["music"],
            "test",
        )
        .unwrap();
        drop(db);
        for enabled in [true, false] {
            let mut settings = crate::settings::Settings::default();
            settings.video_thumb_use_sidecar_image = enabled;
            let pending = spawn_tag_view_search(
                data_dir.clone(),
                "#music".into(),
                TagViewKindFilter::All,
                settings,
            );
            let result = pending
                .rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
                .unwrap();
            assert_eq!(
                result.entries.len(),
                2,
                "aggregate source discovery must keep explicit image results"
            );
            let key = crate::path_key::normalize_keep_drive(&audio);
            assert_eq!(
                result.video_thumb_overrides.get(&key),
                enabled.then_some(&cover)
            );
        }
    }
}
