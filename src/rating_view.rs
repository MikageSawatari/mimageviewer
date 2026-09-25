//! レーティング一覧ビューの worker / 復元 / ソート。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::grid_item::GridItem;
use crate::rating_db::{RatingItemKind, RatingRow};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RatingViewSort {
    Normal(crate::settings::SortOrder),
    RatedAtDesc,
    RatedAtAsc,
}

impl Default for RatingViewSort {
    fn default() -> Self {
        Self::RatedAtDesc
    }
}

impl RatingViewSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal(order) => order.label(),
            Self::RatedAtDesc => "★設定時刻（新しい順）",
            Self::RatedAtAsc => "★設定時刻（古い順）",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Normal(order) => order.short_label(),
            Self::RatedAtDesc => "★時刻↓",
            Self::RatedAtAsc => "★時刻↑",
        }
    }

    /// カテゴリ再配置 (`grid_display_order`) を通すソートか。
    ///
    /// 時刻ソートは「★を付けた順に一列で見る」ことが要求そのものなので通さない。
    /// 通すとフォルダ / アーカイブが時刻に関係なく先頭へ出て、時刻順が壊れる
    /// (docs/next-release-backlog.md §1.142)。`Normal` はフォルダを先に見せる期待が
    /// 残るので従来どおり通す。
    pub fn arranges_by_category(self) -> bool {
        matches!(self, Self::Normal(_))
    }
}

#[derive(Clone)]
pub struct RatingViewRow {
    pub key: String,
    pub item: GridItem,
    pub image_meta: Option<(i64, i64)>,
    pub rated_at_ms: Option<i64>,
}

pub struct RatingViewBuildResult {
    pub stars: u8,
    pub rows: Vec<RatingViewRow>,
    pub skipped: usize,
    pub(crate) prepared: Option<RatingViewPreparedItems>,
}

pub(crate) struct RatingViewPreparedItems {
    pub(crate) items: Vec<GridItem>,
    pub(crate) image_metas: Vec<Option<(i64, i64)>>,
    pub(crate) existing_keys: std::collections::HashSet<String>,
    pub(crate) pin_map:
        std::collections::HashMap<String, crate::folder_thumb_pins::FolderPinSource>,
    pub(crate) video_items: Vec<(usize, PathBuf, u64)>,
    pub(crate) page_edits: crate::app::page_edit_snapshot::StablePageEditProjection,
    pub(crate) rating_stamp: crate::page_edit_write_epoch::WriteStamp,
    pub(crate) tag_stamp: crate::page_edit_write_epoch::WriteStamp,
    pub(crate) rating_cache: std::collections::HashMap<usize, u8>,
    pub(crate) tags_cache: std::collections::HashMap<String, Vec<String>>,
}

pub(crate) struct RatingViewPrepareOptions {
    pub(crate) sort: RatingViewSort,
    pub(crate) display_order: crate::settings::GridDisplayOrder,
    pub(crate) pin_db: Option<Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
    pub(crate) folder_thumb_sort: crate::settings::SortOrder,
    pub(crate) folder_thumb_depth: u32,
    pub(crate) edits: crate::app::page_edit_snapshot::PageEditAvailability,
    pub(crate) tags_db_path: Option<PathBuf>,
}

pub struct RatingViewPending {
    pub stars: u8,
    pub sequence: u64,
    pub source_generation: u64,
    pub(crate) context: crate::app::ViewerContextId,
    pub rating_write_generation: u64,
    pub sort: RatingViewSort,
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<RatingViewBuildResult, String>>,
}

impl RatingViewPending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn spawn_rating_view_build(
    db_path: PathBuf,
    stars: u8,
    sequence: u64,
    source_generation: u64,
    context: crate::app::ViewerContextId,
    rating_write_generation: u64,
    options: RatingViewPrepareOptions,
) -> RatingViewPending {
    let options_sort = options.sort;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    #[cfg(test)]
    let test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
    std::thread::Builder::new()
        .name("rating-view-build".to_string())
        .spawn(move || {
            #[cfg(test)]
            let _test_epoch_scope =
                test_epoch_scope.map(crate::page_edit_write_epoch::TestEpochScope::enter);
            let result = prepare_rating_view(db_path, stars, options, &cancel_worker);
            let _ = tx.send(result);
        })
        .ok();
    RatingViewPending {
        stars,
        sequence,
        source_generation,
        context,
        rating_write_generation,
        sort: options_sort,
        cancel,
        rx,
    }
}

fn build_rating_view_rows(
    db_path: PathBuf,
    stars: u8,
    cancel: &AtomicBool,
) -> Result<RatingViewBuildResult, rusqlite::Error> {
    // 既存 DB を読み取り専用で開く (マイグレーション DDL を再実行せず、main 接続と
    // 競合しない)。ファイルが無い等で失敗したら作成込みの open_at にフォールバックする。
    let db = match crate::rating_db::RatingDb::open_readonly(&db_path) {
        Ok(db) => db,
        Err(_) => crate::rating_db::RatingDb::open_at(&db_path)?,
    };
    let rows = db.list_by_stars(stars)?;
    let mut out = Vec::with_capacity(rows.len());
    let mut skipped = 0usize;
    for row in rows {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match rating_row_to_view_row(&row) {
            Some(view_row) => out.push(view_row),
            None => skipped += 1,
        }
    }
    Ok(RatingViewBuildResult {
        stars,
        rows: out,
        skipped,
        prepared: None,
    })
}

fn prepare_rating_view(
    db_path: PathBuf,
    stars: u8,
    options: RatingViewPrepareOptions,
    cancel: &AtomicBool,
) -> Result<RatingViewBuildResult, String> {
    let rating_before = crate::rating_db::RATING_WRITES.sample();
    if rating_before.active_writers != 0 {
        return Err("rating read changed during rating prepare".into());
    }
    let mut result = match build_rating_view_rows(db_path, stars, cancel) {
        Ok(result) => result,
        Err(error) => {
            if !crate::page_edit_write_epoch::EditWriteEpoch::read_is_stable(
                rating_before,
                crate::rating_db::RATING_WRITES.sample(),
            ) {
                return Err("rating read changed during rating prepare".into());
            }
            return Err(error.to_string());
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let (items, image_metas) =
        sort_and_materialize_rows(&mut result.rows, options.sort, &options.display_order);
    let video_items = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let GridItem::Video(path) = item else {
                return None;
            };
            let size = image_metas[index]
                .map(|(_, size)| size.max(0) as u64)
                .unwrap_or(0);
            Some((index, path.clone(), size))
        })
        .collect();
    let pin_map = if let Some(db) = options.pin_db.as_ref() {
        let containers = items
            .iter()
            .filter_map(GridItem::container_path)
            .collect::<Vec<_>>();
        db.lookup_many(containers)
    } else {
        Default::default()
    };
    let existing_keys = items
        .iter()
        .zip(image_metas.iter())
        .flat_map(|(item, meta)| {
            crate::app::folder_thumb_existing_keys_for(
                item,
                *meta,
                &pin_map,
                options.pin_db.as_deref(),
                Some(options.folder_thumb_sort),
                options.folder_thumb_depth,
                true,
            )
        })
        .collect();
    let page_edits = crate::app::page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
        &items,
        options.edits,
        cancel,
    )?
    .ok_or_else(|| "page-edit read changed during rating prepare".to_string())?;
    let rating_cache = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.accepts_rating())
        .map(|(index, _)| (index, stars))
        .collect();
    let tag_keys = items
        .iter()
        .filter_map(crate::app::tag_item_path)
        .map(crate::tags_db::item_key_for_path)
        .collect::<std::collections::HashSet<_>>();
    let tag_before = crate::tags_db::TAG_WRITES.sample();
    if tag_before.active_writers != 0 {
        return Err("tag read changed during rating prepare".into());
    }
    let mut loaded_tags = options
        .tags_db_path
        .as_deref()
        .and_then(|path| crate::tags_db::TagsDb::open_readonly(path).ok())
        .map(|db| db.get_many_display_tags(&tag_keys.iter().cloned().collect::<Vec<_>>()))
        .unwrap_or_default();
    let tags_cache = tag_keys
        .into_iter()
        .map(|key| {
            let tags = loaded_tags.remove(&key).unwrap_or_default();
            (key, tags)
        })
        .collect();
    let tag_stamp = crate::tags_db::TAG_WRITES.sample();
    if !crate::page_edit_write_epoch::EditWriteEpoch::read_is_stable(tag_before, tag_stamp) {
        return Err("tag read changed during rating prepare".into());
    }
    let rating_stamp = crate::rating_db::RATING_WRITES.sample();
    if !crate::page_edit_write_epoch::EditWriteEpoch::read_is_stable(rating_before, rating_stamp) {
        return Err("rating read changed during rating prepare".into());
    }
    result.prepared = Some(RatingViewPreparedItems {
        items,
        image_metas,
        existing_keys,
        pin_map,
        video_items,
        page_edits,
        rating_stamp,
        tag_stamp,
        rating_cache,
        tags_cache,
    });
    Ok(result)
}

pub fn sort_rows(rows: &mut [RatingViewRow], sort: RatingViewSort) {
    match sort {
        RatingViewSort::RatedAtDesc => rows.sort_by(|a, b| {
            cmp_optional_i64_none_last(a.rated_at_ms, b.rated_at_ms, false)
                .then_with(|| compare_row_names(a, b, crate::settings::SortOrder::FileName))
        }),
        RatingViewSort::RatedAtAsc => rows.sort_by(|a, b| {
            cmp_optional_i64_none_last(a.rated_at_ms, b.rated_at_ms, true)
                .then_with(|| compare_row_names(a, b, crate::settings::SortOrder::FileName))
        }),
        RatingViewSort::Normal(order) => rows.sort_by(|a, b| compare_row_names(a, b, order)),
    }
}

/// 行をソートし、そのまま grid の items と同位置メタデータへ落とす。
///
/// 時刻ソートの間はカテゴリ再配置を通さないので、返る items の順序は [`sort_rows`] の
/// 結果と一致する (§1.142)。`Normal` のときだけ再配置を通し、`rows` も再配置後の順序へ
/// 並べ直す。
pub fn sort_and_materialize_rows(
    rows: &mut Vec<RatingViewRow>,
    sort: RatingViewSort,
    display_order: &crate::settings::GridDisplayOrder,
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    sort_rows(rows, sort);
    crate::grid_item::materialize_view_rows(
        rows,
        display_order,
        sort.arranges_by_category(),
        |row| &row.item,
        |row| row.image_meta,
    )
}

fn compare_row_names(
    a: &RatingViewRow,
    b: &RatingViewRow,
    order: crate::settings::SortOrder,
) -> std::cmp::Ordering {
    let name_a = a.item.name();
    let name_b = b.item.name();
    let key_a = order.name_key(name_a.as_ref());
    let key_b = order.name_key(name_b.as_ref());
    let meta_a = crate::grid_item::listing_sort_metadata_for_item(
        &a.item,
        crate::settings::ListingSortMetadata::new(
            a.image_meta.map(|(mtime, _)| mtime).unwrap_or(0),
            a.image_meta.map(|(_, size)| size),
        ),
    );
    let meta_b = crate::grid_item::listing_sort_metadata_for_item(
        &b.item,
        crate::settings::ListingSortMetadata::new(
            b.image_meta.map(|(mtime, _)| mtime).unwrap_or(0),
            b.image_meta.map(|(_, size)| size),
        ),
    );
    order.compare_listing_keys(&key_a, meta_a, &key_b, meta_b)
}

fn cmp_optional_i64_none_last(
    a: Option<i64>,
    b: Option<i64>,
    ascending: bool,
) -> std::cmp::Ordering {
    match (a, b) {
        (Some(av), Some(bv)) if ascending => av.cmp(&bv),
        (Some(av), Some(bv)) => bv.cmp(&av),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

pub fn rating_row_to_view_row(row: &RatingRow) -> Option<RatingViewRow> {
    let item = match row.kind {
        Some(kind) => item_from_kind(row, kind).or_else(|| item_from_legacy_key(row))?,
        None => item_from_legacy_key(row)?,
    };
    let meta_path = source_path_for_item(&item)?;
    let fs_meta = std::fs::metadata(meta_path).ok()?;
    let image_meta = Some((
        mtime_secs(&fs_meta),
        fs_meta.len().min(i64::MAX as u64) as i64,
    ));
    Some(RatingViewRow {
        key: row.key.clone(),
        item,
        image_meta,
        rated_at_ms: row.rated_at_ms,
    })
}

fn item_from_kind(row: &RatingRow, kind: RatingItemKind) -> Option<GridItem> {
    match kind {
        RatingItemKind::Image => Some(GridItem::Image(existing_source_or_key_path(row)?)),
        RatingItemKind::Video => Some(GridItem::Video(existing_source_or_key_path(row)?)),
        RatingItemKind::Audio => Some(GridItem::Audio(existing_source_or_key_path(row)?)),
        RatingItemKind::Folder => Some(GridItem::Folder(existing_source_or_key_path(row)?)),
        RatingItemKind::ZipFile => Some(GridItem::ZipFile(existing_source_or_key_path(row)?)),
        RatingItemKind::PdfFile => Some(GridItem::PdfFile(existing_source_or_key_path(row)?)),
        RatingItemKind::ConvertibleArchive => {
            let path = existing_source_or_key_path(row)?;
            let format = row
                .archive_format
                .as_deref()
                .and_then(crate::reading_history_db::archive_format_from_str)
                .or_else(|| archive_format_for_path(&path))?;
            Some(GridItem::ConvertibleArchive { path, format })
        }
        RatingItemKind::ZipImage => {
            let zip_path = existing_source_or_key_path(row)?;
            let entry_name = row.entry_name.clone().or_else(|| {
                legacy_entry_from_key(&row.key)
                    .and_then(|entry| resolve_legacy_zip_entry_name(&zip_path, &entry))
            })?;
            Some(GridItem::ZipImage {
                zip_path,
                entry_name,
            })
        }
        RatingItemKind::PdfPage => {
            let pdf_path = existing_source_or_key_path(row)?;
            let page_num = row
                .page_num
                .or_else(|| legacy_pdf_page_from_key(&row.key))?;
            Some(GridItem::PdfPage {
                pdf_path,
                page_num,
                content_type: None,
            })
        }
        RatingItemKind::ZipDir => {
            let zip_path = existing_source_or_key_path(row)?;
            let dir_prefix = row.dir_prefix.clone()?;
            let is_archive = row
                .zipdir_is_archive
                .unwrap_or_else(|| zipdir_prefix_is_archive(&dir_prefix));
            Some(GridItem::ZipDir {
                zip_path,
                dir_prefix,
                is_archive,
                representative: row.zipdir_representative.clone(),
            })
        }
    }
}

fn item_from_legacy_key(row: &RatingRow) -> Option<GridItem> {
    let Some((left, right)) = row.key.split_once("::") else {
        return item_from_plain_path(&existing_path(PathBuf::from(&row.key))?);
    };
    let container = existing_path(PathBuf::from(left))?;
    let ext = ext_lower(&container);
    if ext == "pdf" {
        if let Some(page_num) = parse_page_key(right) {
            return Some(GridItem::PdfPage {
                pdf_path: container,
                page_num,
                content_type: None,
            });
        }
    }
    if entry_is_image(right) {
        let entry_name = resolve_legacy_zip_entry_name(&container, right)?;
        return Some(GridItem::ZipImage {
            zip_path: container,
            entry_name,
        });
    }
    None
}

fn item_from_plain_path(path: &Path) -> Option<GridItem> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        return Some(GridItem::Folder(path.to_path_buf()));
    }
    if !meta.is_file() {
        return None;
    }
    let ext = ext_lower(path);
    if crate::folder_tree::is_recognized_image_ext(&ext) {
        Some(GridItem::Image(path.to_path_buf()))
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(GridItem::Video(path.to_path_buf()))
    } else if crate::folder_tree::SUPPORTED_AUDIO_EXTENSIONS.contains(&ext.as_str()) {
        Some(GridItem::Audio(path.to_path_buf()))
    } else if crate::folder_tree::is_zip_extension(&ext) {
        Some(GridItem::ZipFile(path.to_path_buf()))
    } else if ext == "pdf" {
        Some(GridItem::PdfFile(path.to_path_buf()))
    } else if let Some(format) = crate::archive_converter::ArchiveFormat::from_extension(&ext) {
        Some(GridItem::ConvertibleArchive {
            path: path.to_path_buf(),
            format,
        })
    } else {
        None
    }
}

fn existing_source_or_key_path(row: &RatingRow) -> Option<PathBuf> {
    row.source_path
        .as_ref()
        .map(PathBuf::from)
        .and_then(existing_path)
        .or_else(|| key_source_path(&row.key).and_then(existing_path))
}

fn key_source_path(key: &str) -> Option<PathBuf> {
    let left = key.split_once("::").map(|(left, _)| left).unwrap_or(key);
    (!left.is_empty()).then(|| PathBuf::from(left))
}

fn existing_path(path: PathBuf) -> Option<PathBuf> {
    if matches!(path.try_exists(), Ok(true)) {
        return Some(path);
    }
    find_case_insensitive_sibling(&path)
}

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

fn source_path_for_item(item: &GridItem) -> Option<&Path> {
    match item {
        GridItem::Folder(p)
        | GridItem::Image(p)
        | GridItem::Video(p)
        | GridItem::Audio(p)
        | GridItem::ZipFile(p)
        | GridItem::PdfFile(p) => Some(p),
        GridItem::ConvertibleArchive { path, .. } => Some(path),
        GridItem::ZipImage { zip_path, .. } => Some(zip_path),
        GridItem::PdfPage { pdf_path, .. } => Some(pdf_path),
        GridItem::ZipDir { zip_path, .. } => Some(zip_path),
        _ => None,
    }
}

fn legacy_entry_from_key(key: &str) -> Option<String> {
    key.split_once("::")
        .map(|(_, right)| right.to_string())
        .filter(|right| !right.is_empty())
}

fn resolve_legacy_zip_entry_name(zip_path: &Path, legacy_entry: &str) -> Option<String> {
    let wanted = normalize_legacy_zip_entry_name(legacy_entry);
    let entries = crate::zip_loader::enumerate_image_entries(zip_path).ok()?;
    let mut matches = entries
        .into_iter()
        .filter(|entry| normalize_legacy_zip_entry_name(&entry.entry_name) == wanted);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.entry_name)
}

fn normalize_legacy_zip_entry_name(entry: &str) -> String {
    entry.replace('\\', "/").to_lowercase()
}

fn legacy_pdf_page_from_key(key: &str) -> Option<u32> {
    key.split_once("::")
        .and_then(|(_, right)| parse_page_key(right))
}

fn parse_page_key(raw: &str) -> Option<u32> {
    raw.strip_prefix("page_")?.parse::<u32>().ok()
}

fn entry_is_image(entry: &str) -> bool {
    let ext = entry
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    crate::folder_tree::is_recognized_image_ext(&ext)
}

fn ext_lower(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn archive_format_for_path(path: &Path) -> Option<crate::archive_converter::ArchiveFormat> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(crate::archive_converter::ArchiveFormat::from_extension)
}

fn zipdir_prefix_is_archive(prefix: &str) -> bool {
    let trimmed = prefix.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let ext = last
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    crate::folder_tree::is_zip_extension(&ext)
        || crate::archive_converter::ArchiveFormat::from_extension(&ext).is_some()
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rating_db::{RatingItemKind, RatingRow};
    use std::io::Write;

    fn row(key: String, kind: Option<RatingItemKind>, source_path: Option<String>) -> RatingRow {
        RatingRow {
            key,
            stars: 4,
            rated_at_ms: Some(100),
            source_path,
            kind,
            entry_name: None,
            page_num: None,
            dir_prefix: None,
            archive_format: None,
            zipdir_is_archive: None,
            zipdir_representative: None,
        }
    }

    /// §1.142 用: フォルダ / アーカイブ / 画像を混ぜ、時刻 NULL を 1 件含む行集合。
    ///
    /// 既定の `GridDisplayOrder` は 1 行目がフォルダ + アーカイブなので、カテゴリ再配置を
    /// 通すと時刻の新しい画像より前へフォルダ / アーカイブが繰り上がる。
    fn mixed_rows() -> Vec<RatingViewRow> {
        fn view_row(name: &str, item: GridItem, rated_at_ms: Option<i64>) -> RatingViewRow {
            RatingViewRow {
                key: name.to_string(),
                item,
                image_meta: Some((0, 0)),
                rated_at_ms,
            }
        }
        vec![
            view_row(
                "img-new",
                GridItem::Image(PathBuf::from(r"C:\x\img-new.jpg")),
                Some(500),
            ),
            view_row(
                "dir-old",
                GridItem::Folder(PathBuf::from(r"C:\x\dir-old")),
                Some(100),
            ),
            view_row(
                "zip-mid",
                GridItem::ZipFile(PathBuf::from(r"C:\x\zip-mid.zip")),
                Some(300),
            ),
            view_row(
                "img-old",
                GridItem::Image(PathBuf::from(r"C:\x\img-old.jpg")),
                Some(200),
            ),
            view_row(
                "zip-null",
                GridItem::ZipFile(PathBuf::from(r"C:\x\zip-null.zip")),
                None,
            ),
        ]
    }

    fn keys(rows: &[RatingViewRow]) -> Vec<String> {
        rows.iter().map(|row| row.key.clone()).collect()
    }

    fn item_keys(items: &[GridItem], rows: &[RatingViewRow]) -> Vec<String> {
        items
            .iter()
            .map(|item| {
                rows.iter()
                    .find(|row| row.item.perf_key() == item.perf_key())
                    .expect("materialized item must come from a source row")
                    .key
                    .clone()
            })
            .collect()
    }

    /// §1.142: ★時刻順の間はカテゴリ再配置を通さず、`sort_rows` の結果がそのまま出る。
    #[test]
    fn rated_at_sort_is_not_regrouped_by_category() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let display_order = crate::settings::GridDisplayOrder::default();
        let mut expected = mixed_rows();
        sort_rows(&mut expected, RatingViewSort::RatedAtDesc);
        assert_eq!(
            keys(&expected),
            vec!["img-new", "zip-mid", "img-old", "dir-old", "zip-null"],
            "前提: 時刻降順・NULL 末尾"
        );

        let mut rows = mixed_rows();
        let (items, metas) =
            sort_and_materialize_rows(&mut rows, RatingViewSort::RatedAtDesc, &display_order);

        assert_eq!(keys(&rows), keys(&expected));
        assert_eq!(item_keys(&items, &rows), keys(&expected));
        assert_eq!(metas.len(), items.len());
    }

    /// 昇順でも同じ。NULL は昇順でも末尾に残る。
    #[test]
    fn rated_at_ascending_sort_is_not_regrouped_by_category() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let display_order = crate::settings::GridDisplayOrder::default();
        let mut expected = mixed_rows();
        sort_rows(&mut expected, RatingViewSort::RatedAtAsc);
        assert_eq!(
            keys(&expected),
            vec!["dir-old", "img-old", "zip-mid", "img-new", "zip-null"]
        );

        let mut rows = mixed_rows();
        let (items, _) =
            sort_and_materialize_rows(&mut rows, RatingViewSort::RatedAtAsc, &display_order);
        assert_eq!(item_keys(&items, &rows), keys(&expected));
    }

    /// `Normal` は従来どおりカテゴリ順 (1 行目 = フォルダ + アーカイブ)。
    #[test]
    fn normal_sort_still_groups_by_category() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let display_order = crate::settings::GridDisplayOrder::default();
        let mut rows = mixed_rows();
        let (items, _) = sort_and_materialize_rows(
            &mut rows,
            RatingViewSort::Normal(crate::settings::SortOrder::FileName),
            &display_order,
        );

        assert_eq!(
            item_keys(&items, &rows),
            vec!["dir-old", "zip-mid", "zip-null", "img-new", "img-old"]
        );
        // 行も再配置後の順序へ揃っている (idx が items と 1 対 1 で対応する)。
        assert_eq!(keys(&rows), item_keys(&items, &rows));
    }

    #[test]
    fn normal_size_sort_distinguishes_real_zero_and_virtual_unknown() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut rows = vec![
            RatingViewRow {
                key: "virtual".into(),
                item: GridItem::PdfPage {
                    pdf_path: PathBuf::from(r"C:\x\book.pdf"),
                    page_num: 1,
                    content_type: None,
                },
                image_meta: Some((1, 1)),
                rated_at_ms: Some(1),
            },
            RatingViewRow {
                key: "ten".into(),
                item: GridItem::Image(PathBuf::from(r"C:\x\ten.jpg")),
                image_meta: Some((1, 10)),
                rated_at_ms: Some(1),
            },
            RatingViewRow {
                key: "zero".into(),
                item: GridItem::Image(PathBuf::from(r"C:\x\zero.jpg")),
                image_meta: Some((1, 0)),
                rated_at_ms: Some(1),
            },
        ];
        sort_rows(
            &mut rows,
            RatingViewSort::Normal(crate::settings::SortOrder::SizeAsc),
        );
        assert_eq!(keys(&rows), ["zero", "ten", "virtual"]);
    }

    #[test]
    fn normal_sort_applies_name_and_numeric_desc() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let base = vec![
            RatingViewRow {
                key: "two".into(),
                item: GridItem::Image(PathBuf::from(r"C:\x\page2.jpg")),
                image_meta: Some((1, 1)),
                rated_at_ms: Some(1),
            },
            RatingViewRow {
                key: "ten".into(),
                item: GridItem::Image(PathBuf::from(r"C:\x\page10.jpg")),
                image_meta: Some((1, 1)),
                rated_at_ms: Some(1),
            },
        ];
        for order in [
            crate::settings::SortOrder::FileNameDesc,
            crate::settings::SortOrder::NumericDesc,
        ] {
            let mut rows = base.clone();
            sort_rows(&mut rows, RatingViewSort::Normal(order));
            assert_eq!(keys(&rows), ["ten", "two"], "{order:?}");
        }
    }

    fn write_zip_entries(zip_path: &Path, entries: &[&str]) {
        let file = std::fs::File::create(zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for entry in entries {
            zip.start_file(entry, options).unwrap();
            zip.write_all(b"image bytes").unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn restores_explicit_pdf_page_without_index_conversion() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let pdf = temp.path().join("Book.pdf");
        std::fs::write(&pdf, b"pdf").unwrap();
        let mut r = row(
            "ignored".to_string(),
            Some(RatingItemKind::PdfPage),
            Some(pdf.to_string_lossy().to_string()),
        );
        r.page_num = Some(0);

        let restored = rating_row_to_view_row(&r).unwrap();
        match restored.item {
            GridItem::PdfPage { page_num, .. } => assert_eq!(page_num, 0),
            _ => panic!("expected PdfPage"),
        }
    }

    #[test]
    fn restores_legacy_zip_image_key() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("Book.zip");
        write_zip_entries(&zip, &["Dir/Page.JPG"]);
        let key = format!("{}::dir/page.jpg", zip.to_string_lossy());
        let r = row(key, None, None);

        let restored = rating_row_to_view_row(&r).unwrap();
        match restored.item {
            GridItem::ZipImage { entry_name, .. } => assert_eq!(entry_name, "Dir/Page.JPG"),
            _ => panic!("expected ZipImage"),
        }
    }

    #[test]
    fn skips_ambiguous_legacy_zip_image_key() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("Book.zip");
        write_zip_entries(&zip, &["Dir/Page.JPG", "dir/page.jpg"]);
        let key = format!("{}::dir/page.jpg", zip.to_string_lossy());
        let r = row(key, None, None);

        assert!(rating_row_to_view_row(&r).is_none());
    }

    #[test]
    fn rated_at_sort_keeps_null_last() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a.jpg");
        let b = temp.path().join("b.jpg");
        let c = temp.path().join("c.jpg");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        std::fs::write(&c, b"c").unwrap();
        let mut rows = vec![
            RatingViewRow {
                key: "a".to_string(),
                item: GridItem::Image(a),
                image_meta: Some((0, 1)),
                rated_at_ms: None,
            },
            RatingViewRow {
                key: "b".to_string(),
                item: GridItem::Image(b),
                image_meta: Some((0, 1)),
                rated_at_ms: Some(20),
            },
            RatingViewRow {
                key: "c".to_string(),
                item: GridItem::Image(c),
                image_meta: Some((0, 1)),
                rated_at_ms: Some(10),
            },
        ];

        sort_rows(&mut rows, RatingViewSort::RatedAtDesc);
        assert_eq!(rows[0].rated_at_ms, Some(20));
        assert_eq!(rows[1].rated_at_ms, Some(10));
        assert_eq!(rows[2].rated_at_ms, None);

        sort_rows(&mut rows, RatingViewSort::RatedAtAsc);
        assert_eq!(rows[0].rated_at_ms, Some(10));
        assert_eq!(rows[1].rated_at_ms, Some(20));
        assert_eq!(rows[2].rated_at_ms, None);
    }
}
