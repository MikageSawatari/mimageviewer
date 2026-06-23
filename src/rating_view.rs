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
}

pub struct RatingViewPending {
    pub stars: u8,
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<RatingViewBuildResult, String>>,
}

impl RatingViewPending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn spawn_rating_view_build(db_path: PathBuf, stars: u8) -> RatingViewPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rating-view-build".to_string())
        .spawn(move || {
            let result = build_rating_view_rows(db_path, stars, &cancel_worker)
                .map_err(|err| err.to_string());
            let _ = tx.send(result);
        })
        .ok();
    RatingViewPending { stars, cancel, rx }
}

fn build_rating_view_rows(
    db_path: PathBuf,
    stars: u8,
    cancel: &AtomicBool,
) -> Result<RatingViewBuildResult, rusqlite::Error> {
    let db = crate::rating_db::RatingDb::open_at(db_path)?;
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
    })
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

fn compare_row_names(
    a: &RatingViewRow,
    b: &RatingViewRow,
    order: crate::settings::SortOrder,
) -> std::cmp::Ordering {
    let name_a = a.item.name();
    let name_b = b.item.name();
    let key_a = order.name_key(name_a.as_ref());
    let key_b = order.name_key(name_b.as_ref());
    let mtime_a = a.image_meta.map(|(mtime, _)| mtime).unwrap_or(0);
    let mtime_b = b.image_meta.map(|(mtime, _)| mtime).unwrap_or(0);
    order.compare_name_keys(&key_a, mtime_a, &key_b, mtime_b)
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
            let entry_name = row
                .entry_name
                .clone()
                .or_else(|| legacy_entry_from_key(&row.key))?;
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
        return Some(GridItem::ZipImage {
            zip_path: container,
            entry_name: right.to_string(),
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

    #[test]
    fn restores_explicit_pdf_page_without_index_conversion() {
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
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("Book.zip");
        std::fs::write(&zip, b"zip").unwrap();
        let key = format!("{}::dir/page.jpg", zip.to_string_lossy());
        let r = row(key, None, None);

        let restored = rating_row_to_view_row(&r).unwrap();
        match restored.item {
            GridItem::ZipImage { entry_name, .. } => assert_eq!(entry_name, "dir/page.jpg"),
            _ => panic!("expected ZipImage"),
        }
    }

    #[test]
    fn rated_at_sort_keeps_null_last() {
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
