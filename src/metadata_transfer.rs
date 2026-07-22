//! 明示操作によるポータブル・メタ情報のエクスポート / インポート。
//!
//! 既存の `mimageviewer.dat`（編集情報の自動 sidecar）とは責務を分離し、
//! `mimageviewer.meta.miv` 1 個にフォルダ配下の評価・タグ・ブックマークをまとめる。
//! このモジュールの公開 API は worker スレッドから呼ぶこと。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const SIDECAR_FILENAME: &str = "mimageviewer.meta.miv";
const FORMAT_NAME: &str = "mimageviewer-portable-metadata";
const FORMAT_VERSION: u32 = 1;
const MAX_SIDECAR_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 500_000;
const MAX_PATH_CHARS: usize = 32_768;
const MAX_MEMBER_KEY_CHARS: usize = 65_536;
const MAX_TITLE_CHARS: usize = 1_024;
const MAX_BOOKMARKS_PER_ENTRY: usize = 100_000;
const MAX_RECURSION_DEPTH: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferPhase {
    Scanning,
    ReadingMetadata,
    WritingSidecar,
    ReadingSidecar,
    Importing,
}

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub phase: TransferPhase,
    pub processed: usize,
    pub total: usize,
    pub current_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    pub entries: usize,
    pub ratings: usize,
    pub tagged_items: usize,
    pub timed_bookmarks: usize,
    pub book_bookmarks: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPreview {
    pub entries: usize,
    pub existing_entries: usize,
    pub missing_entries: usize,
    pub changed_files: usize,
    pub recursive: bool,
    pub exported_at_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub total_entries: usize,
    pub applied_entries: usize,
    pub skipped_missing: usize,
    pub skipped_changed: usize,
    pub failed_entries: usize,
    pub cancelled: bool,
    /// XMP rating hydration が明示 import の「評価なし」を直後に復活させないための
    /// session-local suppression key。UI 表示には使わない。
    pub applied_rating_keys: Vec<String>,
}

#[derive(Debug)]
pub enum TransferError {
    Cancelled,
    Io(String),
    Invalid(String),
    Database(String),
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "キャンセルされました"),
            Self::Io(message) => write!(f, "ファイル操作に失敗しました: {message}"),
            Self::Invalid(message) => write!(f, "メタ情報ファイルが不正です: {message}"),
            Self::Database(message) => write!(f, "メタ情報DBの操作に失敗しました: {message}"),
        }
    }
}

impl std::error::Error for TransferError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    exported_at_ms: i64,
    recursive: bool,
    sections: ManifestSections,
    entries: Vec<PortableEntry>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct ManifestSections {
    ratings: bool,
    tags: bool,
    timed_bookmarks: bool,
    book_bookmarks: bool,
}

impl Default for ManifestSections {
    fn default() -> Self {
        Self {
            ratings: true,
            tags: true,
            timed_bookmarks: true,
            book_bookmarks: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PortableEntryKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileFingerprint {
    size: u64,
    modified_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableEntry {
    path: String,
    kind: PortableEntryKind,
    fingerprint: Option<FileFingerprint>,
    rating: Option<PortableRating>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    timed_bookmarks: Vec<PortableTimedBookmark>,
    #[serde(default)]
    book_bookmarks: Vec<PortableBookBookmark>,
    #[serde(default)]
    virtual_items: Vec<PortableVirtualItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableVirtualItem {
    member_key: String,
    rating: Option<PortableRating>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableRating {
    stars: u8,
    rated_at_ms: Option<i64>,
    kind: Option<String>,
    entry_name: Option<String>,
    page_num: Option<u32>,
    dir_prefix: Option<String>,
    archive_format: Option<String>,
    zipdir_is_archive: Option<bool>,
    zipdir_representative: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableTimedBookmark {
    pts_secs: f64,
    title: Option<String>,
    created_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableBookBookmark {
    container_kind: String,
    page_kind: String,
    page_value: String,
    page_index_hint: usize,
    created_at_ms: i64,
    title: Option<String>,
}

struct EnumeratedEntry {
    path: PathBuf,
    portable: PortableEntry,
}

/// `root` と同じフォルダの固定 sidecar へエクスポートする。
/// 完成するまで sibling temp に書き、成功時だけ既存 sidecar と原子的に置換する。
pub fn export_at<F>(
    data_dir: &Path,
    root: &Path,
    recursive: bool,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<ExportSummary, TransferError>
where
    F: FnMut(TransferProgress),
{
    validate_root(root)?;
    let mut entries = enumerate_entries(root, recursive, cancel, &mut progress)?;
    check_cancel(cancel)?;
    progress(TransferProgress {
        phase: TransferPhase::ReadingMetadata,
        processed: 0,
        total: 0,
        current_path: None,
    });
    attach_metadata(data_dir, &mut entries, cancel, &mut progress)?;
    check_cancel(cancel)?;

    let exported_at_ms = now_ms();
    let manifest = Manifest {
        format: FORMAT_NAME.to_string(),
        version: FORMAT_VERSION,
        exported_at_ms,
        recursive,
        sections: ManifestSections::default(),
        entries: entries.into_iter().map(|entry| entry.portable).collect(),
    };
    validate_manifest(&manifest)?;
    let summary = summarize_export(&manifest);
    progress(TransferProgress {
        phase: TransferPhase::WritingSidecar,
        processed: 0,
        total: manifest.entries.len(),
        current_path: Some(SIDECAR_FILENAME.to_string()),
    });
    write_manifest_atomic(root, &manifest, cancel)?;
    progress(TransferProgress {
        phase: TransferPhase::WritingSidecar,
        processed: manifest.entries.len(),
        total: manifest.entries.len(),
        current_path: Some(SIDECAR_FILENAME.to_string()),
    });
    Ok(summary)
}

/// sidecar を検証し、適用前の確認画面に必要な件数を返す。DB は変更しない。
pub fn inspect_import_at<F>(
    root: &Path,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<ImportPreview, TransferError>
where
    F: FnMut(TransferProgress),
{
    validate_root(root)?;
    progress(TransferProgress {
        phase: TransferPhase::ReadingSidecar,
        processed: 0,
        total: 0,
        current_path: Some(SIDECAR_FILENAME.to_string()),
    });
    let manifest = read_manifest(root, cancel)?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| TransferError::Io(format!("{}: {error}", root.display())))?;
    let mut preview = ImportPreview {
        entries: manifest.entries.len(),
        recursive: manifest.recursive,
        exported_at_ms: manifest.exported_at_ms,
        ..ImportPreview::default()
    };
    for (index, entry) in manifest.entries.iter().enumerate() {
        check_cancel(cancel)?;
        let path = resolve_entry_path(root, &canonical_root, &entry.path)?;
        match verify_target(&path, entry) {
            TargetState::Ready => preview.existing_entries += 1,
            TargetState::Missing => preview.missing_entries += 1,
            TargetState::Changed => preview.changed_files += 1,
        }
        progress(TransferProgress {
            phase: TransferPhase::ReadingSidecar,
            processed: index + 1,
            total: manifest.entries.len(),
            current_path: Some(entry.path.clone()),
        });
    }
    Ok(preview)
}

/// sidecar に記載された物理項目を 1 件ずつ上書きする。
/// キャンセル時は完了済み項目を保持し、未着手項目には触れない。
pub fn import_at<F>(
    data_dir: &Path,
    root: &Path,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<ImportSummary, TransferError>
where
    F: FnMut(TransferProgress),
{
    validate_root(root)?;
    let manifest = read_manifest(root, cancel)?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| TransferError::Io(format!("{}: {error}", root.display())))?;
    // Sidecar は信頼しない。1 件でも root 外を指す既存 reparse path があれば、
    // DB を触る前にファイル全体を拒否する。
    let resolved_paths = manifest
        .entries
        .iter()
        .map(|entry| resolve_entry_path(root, &canonical_root, &entry.path))
        .collect::<Result<Vec<_>, _>>()?;
    ensure_database_schemas(data_dir)?;
    let mut conn = open_import_connection(data_dir)?;
    let mut summary = ImportSummary {
        total_entries: manifest.entries.len(),
        ..ImportSummary::default()
    };

    for (index, (entry, path)) in manifest
        .entries
        .iter()
        .zip(resolved_paths.iter())
        .enumerate()
    {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        progress(TransferProgress {
            phase: TransferPhase::Importing,
            processed: index,
            total: manifest.entries.len(),
            current_path: Some(entry.path.clone()),
        });
        match verify_target(&path, entry) {
            TargetState::Missing => {
                summary.skipped_missing += 1;
                progress(TransferProgress {
                    phase: TransferPhase::Importing,
                    processed: index + 1,
                    total: manifest.entries.len(),
                    current_path: Some(entry.path.clone()),
                });
                continue;
            }
            TargetState::Changed => {
                summary.skipped_changed += 1;
                progress(TransferProgress {
                    phase: TransferPhase::Importing,
                    processed: index + 1,
                    total: manifest.entries.len(),
                    current_path: Some(entry.path.clone()),
                });
                continue;
            }
            TargetState::Ready => {}
        }
        match apply_entry(
            &mut conn,
            path,
            entry,
            manifest.sections,
            manifest.exported_at_ms,
        ) {
            Ok(()) => {
                summary.applied_entries += 1;
                summary
                    .applied_rating_keys
                    .push(crate::path_key::normalize_keep_drive(path));
            }
            Err(error) => {
                crate::logger::log(format!(
                    "metadata import: failed to apply {}: {error}",
                    entry.path
                ));
                summary.failed_entries += 1;
            }
        }
        progress(TransferProgress {
            phase: TransferPhase::Importing,
            processed: index + 1,
            total: manifest.entries.len(),
            current_path: Some(entry.path.clone()),
        });
    }
    Ok(summary)
}

fn validate_root(root: &Path) -> Result<(), TransferError> {
    if !root.is_dir() {
        return Err(TransferError::Invalid(format!(
            "対象フォルダが存在しません: {}",
            root.display()
        )));
    }
    Ok(())
}

fn enumerate_entries<F>(
    root: &Path,
    recursive: bool,
    cancel: &AtomicBool,
    progress: &mut F,
) -> Result<Vec<EnumeratedEntry>, TransferError>
where
    F: FnMut(TransferProgress),
{
    let mut out = Vec::new();
    out.push(make_enumerated(
        root,
        ".".to_string(),
        PortableEntryKind::Directory,
    )?);
    let mut visited = HashSet::new();
    crate::fs_entry::mark_directory_visited(root, &mut visited);
    enumerate_directory(
        root,
        root,
        recursive,
        0,
        cancel,
        progress,
        &mut visited,
        &mut out,
    )?;
    out.sort_by(|a, b| a.portable.path.cmp(&b.portable.path));
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn enumerate_directory<F>(
    root: &Path,
    dir: &Path,
    recursive: bool,
    depth: usize,
    cancel: &AtomicBool,
    progress: &mut F,
    visited: &mut HashSet<String>,
    out: &mut Vec<EnumeratedEntry>,
) -> Result<(), TransferError>
where
    F: FnMut(TransferProgress),
{
    if depth > MAX_RECURSION_DEPTH {
        return Err(TransferError::Invalid(format!(
            "フォルダ階層が深すぎます（上限 {MAX_RECURSION_DEPTH} 階層）"
        )));
    }
    let mut children = fs::read_dir(dir)
        .map_err(|error| TransferError::Io(format!("{}: {error}", dir.display())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| TransferError::Io(error.to_string()))?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        check_cancel(cancel)?;
        if is_sidecar_name(&child.file_name()) || is_temp_sidecar_name(&child.file_name()) {
            continue;
        }
        let file_type = child
            .file_type()
            .map_err(|error| TransferError::Io(error.to_string()))?;
        let kind = crate::fs_entry::classify_dir_entry(&child, &file_type);
        let portable_kind = match kind {
            crate::fs_entry::DirEntryKind::Directory
            | crate::fs_entry::DirEntryKind::ReparseDirectory => PortableEntryKind::Directory,
            crate::fs_entry::DirEntryKind::File => PortableEntryKind::File,
            crate::fs_entry::DirEntryKind::Other => continue,
        };
        let path = child.path();
        let rel = relative_string(root, &path)?;
        out.push(make_enumerated(&path, rel.clone(), portable_kind)?);
        if out.len() > MAX_ENTRIES {
            return Err(TransferError::Invalid(format!(
                "対象が多すぎます（上限 {MAX_ENTRIES} 件）"
            )));
        }
        progress(TransferProgress {
            phase: TransferPhase::Scanning,
            processed: out.len(),
            total: 0,
            current_path: Some(rel),
        });
        if recursive
            && kind == crate::fs_entry::DirEntryKind::Directory
            && crate::fs_entry::mark_directory_visited(&path, visited)
        {
            enumerate_directory(root, &path, true, depth + 1, cancel, progress, visited, out)?;
        }
    }
    Ok(())
}

fn make_enumerated(
    path: &Path,
    rel: String,
    kind: PortableEntryKind,
) -> Result<EnumeratedEntry, TransferError> {
    let fingerprint = if kind == PortableEntryKind::File {
        let metadata = fs::metadata(path)
            .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
        Some(FileFingerprint {
            size: metadata.len(),
            modified_ms: metadata.modified().ok().and_then(system_time_ms),
        })
    } else {
        None
    };
    Ok(EnumeratedEntry {
        path: path.to_path_buf(),
        portable: PortableEntry {
            path: rel,
            kind,
            fingerprint,
            rating: None,
            tags: Vec::new(),
            timed_bookmarks: Vec::new(),
            book_bookmarks: Vec::new(),
            virtual_items: Vec::new(),
        },
    })
}

fn attach_metadata<F>(
    data_dir: &Path,
    entries: &mut [EnumeratedEntry],
    cancel: &AtomicBool,
    progress: &mut F,
) -> Result<(), TransferError>
where
    F: FnMut(TransferProgress),
{
    let index: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (crate::path_key::normalize_keep_drive(&entry.path), index))
        .collect();
    let mut virtual_index: HashMap<(usize, String), usize> = HashMap::new();
    let mut metadata_rows = 0usize;

    let rating_path = data_dir.join("rating.db");
    if rating_path.is_file() {
        let conn = open_readonly(&rating_path)?;
        let mut stmt = conn
            .prepare(
                "SELECT path, stars, rated_at_ms, kind, entry_name, page_num, dir_prefix,
                        archive_format, zipdir_is_archive, zipdir_representative FROM ratings",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                let stars: i64 = row.get(1)?;
                let kind: Option<i64> = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    PortableRating {
                        stars: stars.clamp(0, 5) as u8,
                        rated_at_ms: row.get(2)?,
                        kind: kind.and_then(rating_kind_name).map(str::to_string),
                        entry_name: row.get(4)?,
                        page_num: row
                            .get::<_, Option<i64>>(5)?
                            .and_then(|value| u32::try_from(value).ok()),
                        dir_prefix: row.get(6)?,
                        archive_format: row.get(7)?,
                        zipdir_is_archive: row.get::<_, Option<i64>>(8)?.map(|value| value != 0),
                        zipdir_representative: row.get(9)?,
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, rating) = row.map_err(db_error)?;
            report_metadata_progress(&mut metadata_rows, &key, progress);
            if rating.stars == 0 {
                continue;
            }
            if let Some((entry_index, member)) = locate_item_key(&key, &index) {
                if let Some(member) = member {
                    let virtual_item =
                        get_virtual_item(entries, &mut virtual_index, entry_index, member);
                    virtual_item.rating = Some(rating);
                } else {
                    entries[entry_index].portable.rating = Some(rating);
                }
            }
        }
    }

    let tags_path = data_dir.join("tags.db");
    if tags_path.is_file() {
        let conn = open_readonly(&tags_path)?;
        let mut tags_by_key: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut stmt = conn
                .prepare("SELECT item_key, tag FROM item_tags ORDER BY applied_at, tag_key")
                .map_err(db_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(db_error)?;
            for row in rows {
                check_cancel(cancel)?;
                let (key, tag) = row.map_err(db_error)?;
                report_metadata_progress(&mut metadata_rows, &key, progress);
                tags_by_key.entry(key).or_default().push(tag);
            }
        }
        let mut decided = HashSet::new();
        {
            let mut stmt = conn
                .prepare("SELECT item_key FROM tag_item_state")
                .map_err(db_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(db_error)?;
            for row in rows {
                check_cancel(cancel)?;
                let key = row.map_err(db_error)?;
                report_metadata_progress(&mut metadata_rows, &key, progress);
                decided.insert(key);
            }
        }
        decided.extend(tags_by_key.keys().cloned());
        for key in decided {
            check_cancel(cancel)?;
            let tags = tags_by_key.remove(&key).unwrap_or_default();
            if let Some((entry_index, member)) = locate_item_key(&key, &index) {
                if let Some(member) = member {
                    get_virtual_item(entries, &mut virtual_index, entry_index, member).tags = tags;
                } else {
                    entries[entry_index].portable.tags = tags;
                }
            }
        }
    }

    let video_path = data_dir.join("video_bookmarks.db");
    if video_path.is_file() {
        let conn = open_readonly(&video_path)?;
        let mut stmt = conn
            .prepare(
                "SELECT path, pts_secs, title, created_at
                   FROM video_bookmarks ORDER BY path, pts_secs, id",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                let created_at: i64 = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    PortableTimedBookmark {
                        pts_secs: row.get(1)?,
                        title: row
                            .get::<_, Option<String>>(2)?
                            .filter(|value| !value.is_empty()),
                        created_at_ms: created_at.saturating_mul(1000),
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, bookmark) = row.map_err(db_error)?;
            report_metadata_progress(&mut metadata_rows, &key, progress);
            if let Some(&entry_index) = index.get(&key) {
                entries[entry_index].portable.timed_bookmarks.push(bookmark);
            }
        }
    }

    let book_path = data_dir.join("book_bookmarks.db");
    if book_path.is_file() {
        let conn = open_readonly(&book_path)?;
        let mut stmt = conn
            .prepare(
                "SELECT container_key, container_kind, page_kind, page_value,
                        page_index_hint, created_at_ms, title
                   FROM book_bookmarks ORDER BY container_key, page_index_hint, id",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    PortableBookBookmark {
                        container_kind: row.get(1)?,
                        page_kind: row.get(2)?,
                        page_value: row.get(3)?,
                        page_index_hint: row
                            .get::<_, i64>(4)?
                            .max(0)
                            .try_into()
                            .unwrap_or(usize::MAX),
                        created_at_ms: row.get(5)?,
                        title: row
                            .get::<_, Option<String>>(6)?
                            .filter(|value| !value.is_empty()),
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, bookmark) = row.map_err(db_error)?;
            report_metadata_progress(&mut metadata_rows, &key, progress);
            if let Some(&entry_index) = index.get(&key) {
                entries[entry_index].portable.book_bookmarks.push(bookmark);
            }
        }
    }

    for entry in entries {
        entry
            .portable
            .virtual_items
            .sort_by(|a, b| a.member_key.cmp(&b.member_key));
    }
    progress(TransferProgress {
        phase: TransferPhase::ReadingMetadata,
        processed: metadata_rows,
        total: 0,
        current_path: None,
    });
    Ok(())
}

fn report_metadata_progress<F>(processed: &mut usize, key: &str, progress: &mut F)
where
    F: FnMut(TransferProgress),
{
    *processed += 1;
    if *processed == 1 || *processed % 128 == 0 {
        progress(TransferProgress {
            phase: TransferPhase::ReadingMetadata,
            processed: *processed,
            total: 0,
            current_path: Some(key.to_string()),
        });
    }
}

fn get_virtual_item<'a>(
    entries: &'a mut [EnumeratedEntry],
    index: &mut HashMap<(usize, String), usize>,
    entry_index: usize,
    member: String,
) -> &'a mut PortableVirtualItem {
    let key = (entry_index, member.clone());
    let virtual_index = if let Some(&index) = index.get(&key) {
        index
    } else {
        let new_index = entries[entry_index].portable.virtual_items.len();
        entries[entry_index]
            .portable
            .virtual_items
            .push(PortableVirtualItem {
                member_key: member,
                rating: None,
                tags: Vec::new(),
            });
        index.insert(key, new_index);
        new_index
    };
    &mut entries[entry_index].portable.virtual_items[virtual_index]
}

fn locate_item_key(
    key: &str,
    physical_index: &HashMap<String, usize>,
) -> Option<(usize, Option<String>)> {
    if let Some(&index) = physical_index.get(key) {
        return Some((index, None));
    }
    let (base, member) = key.split_once("::")?;
    physical_index
        .get(base)
        .copied()
        .map(|index| (index, Some(member.to_string())))
}

fn summarize_export(manifest: &Manifest) -> ExportSummary {
    let mut summary = ExportSummary {
        entries: manifest.entries.len(),
        ..ExportSummary::default()
    };
    for entry in &manifest.entries {
        summary.ratings += usize::from(entry.rating.is_some());
        summary.tagged_items += usize::from(!entry.tags.is_empty());
        summary.timed_bookmarks += entry.timed_bookmarks.len();
        summary.book_bookmarks += entry.book_bookmarks.len();
        for virtual_item in &entry.virtual_items {
            summary.ratings += usize::from(virtual_item.rating.is_some());
            summary.tagged_items += usize::from(!virtual_item.tags.is_empty());
        }
    }
    summary
}

fn read_manifest(root: &Path, cancel: &AtomicBool) -> Result<Manifest, TransferError> {
    check_cancel(cancel)?;
    let path = root.join(SIDECAR_FILENAME);
    let metadata = fs::metadata(&path)
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
    if metadata.len() > MAX_SIDECAR_BYTES {
        return Err(TransferError::Invalid(format!(
            "ファイルサイズが上限 {} MiB を超えています",
            MAX_SIDECAR_BYTES / 1024 / 1024
        )));
    }
    let file = File::open(&path)
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
    let reader = BufReader::new(file.take(MAX_SIDECAR_BYTES + 1));
    let reader = CancelReader {
        inner: reader,
        cancel,
    };
    let manifest: Manifest = serde_json::from_reader(reader).map_err(|error| {
        if cancel.load(Ordering::Relaxed) {
            TransferError::Cancelled
        } else {
            TransferError::Invalid(format!("JSON: {error}"))
        }
    })?;
    validate_manifest(&manifest)?;
    check_cancel(cancel)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &Manifest) -> Result<(), TransferError> {
    if manifest.format != FORMAT_NAME {
        return Err(TransferError::Invalid(
            "形式識別子が一致しません".to_string(),
        ));
    }
    if manifest.version != FORMAT_VERSION {
        return Err(TransferError::Invalid(format!(
            "未対応のバージョンです: {}",
            manifest.version
        )));
    }
    if manifest.entries.len() > MAX_ENTRIES {
        return Err(TransferError::Invalid(format!(
            "項目数が上限 {MAX_ENTRIES} 件を超えています"
        )));
    }
    let mut paths = HashSet::new();
    for entry in &manifest.entries {
        validate_relative_path(&entry.path)?;
        let normalized = entry.path.replace('\\', "/").to_lowercase();
        if !paths.insert(normalized) {
            return Err(TransferError::Invalid(format!(
                "パスが重複しています: {}",
                entry.path
            )));
        }
        if entry.kind == PortableEntryKind::File && entry.fingerprint.is_none() {
            return Err(TransferError::Invalid(format!(
                "ファイルの照合情報がありません: {}",
                entry.path
            )));
        }
        validate_rating(entry.rating.as_ref())?;
        validate_tags(&entry.tags)?;
        if entry.timed_bookmarks.len() > MAX_BOOKMARKS_PER_ENTRY
            || entry.book_bookmarks.len() > MAX_BOOKMARKS_PER_ENTRY
            || entry.virtual_items.len() > MAX_BOOKMARKS_PER_ENTRY
        {
            return Err(TransferError::Invalid(format!(
                "1項目あたりのメタ情報が多すぎます: {}",
                entry.path
            )));
        }
        for bookmark in &entry.timed_bookmarks {
            if !bookmark.pts_secs.is_finite() || bookmark.pts_secs < 0.0 {
                return Err(TransferError::Invalid(format!(
                    "ブックマーク位置が不正です: {}",
                    entry.path
                )));
            }
            validate_title(bookmark.title.as_deref())?;
        }
        for bookmark in &entry.book_bookmarks {
            if !matches!(
                bookmark.container_kind.as_str(),
                "compiled_book" | "image_folder" | "zip" | "pdf" | "other_archive"
            ) || !matches!(
                bookmark.page_kind.as_str(),
                "relative_path" | "archive_entry" | "pdf_page"
            ) {
                return Err(TransferError::Invalid(format!(
                    "本ブックマークの種類が不正です: {}",
                    entry.path
                )));
            }
            if bookmark.page_value.chars().count() > MAX_MEMBER_KEY_CHARS
                || bookmark.page_value.contains('\0')
                || (bookmark.page_kind == "pdf_page" && bookmark.page_value.parse::<u32>().is_err())
            {
                return Err(TransferError::Invalid(format!(
                    "本ブックマークのページ指定が不正です: {}",
                    entry.path
                )));
            }
            validate_title(bookmark.title.as_deref())?;
        }
        let mut member_keys = HashSet::new();
        for item in &entry.virtual_items {
            if item.member_key.is_empty()
                || item.member_key.chars().count() > MAX_MEMBER_KEY_CHARS
                || item.member_key.contains('\0')
                || !member_keys.insert(item.member_key.to_lowercase())
            {
                return Err(TransferError::Invalid(format!(
                    "仮想項目キーが不正または重複しています: {}",
                    entry.path
                )));
            }
            validate_rating(item.rating.as_ref())?;
            validate_tags(&item.tags)?;
        }
    }
    Ok(())
}

fn validate_rating(rating: Option<&PortableRating>) -> Result<(), TransferError> {
    let Some(rating) = rating else {
        return Ok(());
    };
    if !(1..=5).contains(&rating.stars) {
        return Err(TransferError::Invalid(
            "評価値は1〜5である必要があります".to_string(),
        ));
    }
    if let Some(kind) = &rating.kind {
        if rating_kind_value(kind).is_none() {
            return Err(TransferError::Invalid(format!(
                "評価項目の種類が不正です: {kind}"
            )));
        }
    }
    for value in [
        rating.entry_name.as_deref(),
        rating.dir_prefix.as_deref(),
        rating.archive_format.as_deref(),
        rating.zipdir_representative.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.chars().count() > MAX_MEMBER_KEY_CHARS || value.contains('\0') {
            return Err(TransferError::Invalid(
                "評価復元情報が長すぎます".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_tags(tags: &[String]) -> Result<(), TransferError> {
    if tags.len() > MAX_BOOKMARKS_PER_ENTRY {
        return Err(TransferError::Invalid("タグ数が多すぎます".to_string()));
    }
    if tags
        .iter()
        .any(|tag| !crate::tags_db::is_valid_tag_display_name(tag))
    {
        return Err(TransferError::Invalid("不正なタグ名があります".to_string()));
    }
    Ok(())
}

fn validate_title(title: Option<&str>) -> Result<(), TransferError> {
    if title.is_some_and(|value| value.chars().count() > MAX_TITLE_CHARS || value.contains('\0')) {
        Err(TransferError::Invalid(
            "ブックマーク名が長すぎます".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn validate_relative_path(value: &str) -> Result<(), TransferError> {
    if value.is_empty()
        || value.chars().count() > MAX_PATH_CHARS
        || value.contains('\0')
        || value.contains('\\')
    {
        return Err(TransferError::Invalid(format!(
            "不正な相対パスです: {value}"
        )));
    }
    if value == "." {
        return Ok(());
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(TransferError::Invalid(format!(
            "不正な相対パスです: {value}"
        )));
    }
    Ok(())
}

fn resolve_entry_path(
    root: &Path,
    canonical_root: &Path,
    relative: &str,
) -> Result<PathBuf, TransferError> {
    validate_relative_path(relative)?;
    let path = if relative == "." {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    if let Ok(canonical_target) = fs::canonicalize(&path) {
        let root_key = crate::path_key::normalize_keep_drive(canonical_root);
        let target_key = crate::path_key::normalize_keep_drive(&canonical_target);
        let descendant_prefix = if root_key.ends_with('/') {
            root_key.clone()
        } else {
            format!("{root_key}/")
        };
        if target_key != root_key && !target_key.starts_with(&descendant_prefix) {
            return Err(TransferError::Invalid(format!(
                "フォルダ外を指すパスです: {relative}"
            )));
        }
    }
    Ok(path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetState {
    Ready,
    Missing,
    Changed,
}

fn verify_target(path: &Path, entry: &PortableEntry) -> TargetState {
    let Ok(metadata) = fs::metadata(path) else {
        return TargetState::Missing;
    };
    match entry.kind {
        PortableEntryKind::Directory if metadata.is_dir() => TargetState::Ready,
        PortableEntryKind::File if metadata.is_file() => {
            if entry
                .fingerprint
                .as_ref()
                .is_some_and(|fingerprint| fingerprint.size == metadata.len())
            {
                TargetState::Ready
            } else {
                TargetState::Changed
            }
        }
        _ => TargetState::Changed,
    }
}

fn ensure_database_schemas(data_dir: &Path) -> Result<(), TransferError> {
    fs::create_dir_all(data_dir).map_err(|error| TransferError::Io(error.to_string()))?;
    drop(crate::rating_db::RatingDb::open_at(data_dir.join("rating.db")).map_err(db_error)?);
    drop(crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).map_err(db_error)?);
    drop(
        crate::video_bookmarks::VideoBookmarkDb::open_at(&data_dir.join("video_bookmarks.db"))
            .map_err(db_error)?,
    );
    crate::book_bookmarks::ensure_schema_at(&data_dir.join("book_bookmarks.db"))
        .map_err(db_error)?;
    Ok(())
}

fn open_import_connection(data_dir: &Path) -> Result<Connection, TransferError> {
    let conn = Connection::open(data_dir.join("rating.db")).map_err(db_error)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_error)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS tags",
        [data_dir.join("tags.db").to_string_lossy().as_ref()],
    )
    .map_err(db_error)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS video",
        [data_dir
            .join("video_bookmarks.db")
            .to_string_lossy()
            .as_ref()],
    )
    .map_err(db_error)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS book",
        [data_dir
            .join("book_bookmarks.db")
            .to_string_lossy()
            .as_ref()],
    )
    .map_err(db_error)?;
    Ok(conn)
}

fn apply_entry(
    conn: &mut Connection,
    path: &Path,
    entry: &PortableEntry,
    sections: ManifestSections,
    fallback_time_ms: i64,
) -> Result<(), TransferError> {
    let base_key = crate::path_key::normalize_keep_drive(path);
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if sections.ratings {
        delete_key_family(&tx, "ratings", "path", &base_key)?;
        if let Some(rating) = &entry.rating {
            insert_rating(&tx, &base_key, path, rating)?;
        }
        for item in &entry.virtual_items {
            if let Some(rating) = &item.rating {
                insert_rating(
                    &tx,
                    &format!("{base_key}::{}", item.member_key),
                    path,
                    rating,
                )?;
            }
        }
    }
    if sections.tags {
        delete_key_family(&tx, "tags.item_tags", "item_key", &base_key)?;
        delete_key_family(&tx, "tags.tag_item_state", "item_key", &base_key)?;
        insert_tags(&tx, &base_key, &entry.tags, fallback_time_ms)?;
        for item in &entry.virtual_items {
            insert_tags(
                &tx,
                &format!("{base_key}::{}", item.member_key),
                &item.tags,
                fallback_time_ms,
            )?;
        }
    }
    if sections.timed_bookmarks {
        tx.execute(
            "DELETE FROM video.video_bookmarks WHERE path = ?1",
            [&base_key],
        )
        .map_err(db_error)?;
        for bookmark in &entry.timed_bookmarks {
            tx.execute(
                "INSERT INTO video.video_bookmarks
                    (path, pts_secs, title, thumb_webp, created_at)
                 VALUES (?1, ?2, ?3, NULL, ?4)",
                params![
                    base_key,
                    bookmark.pts_secs,
                    bookmark.title.as_deref(),
                    bookmark.created_at_ms.div_euclid(1000),
                ],
            )
            .map_err(db_error)?;
        }
    }
    if sections.book_bookmarks {
        tx.execute(
            "DELETE FROM book.book_bookmarks WHERE container_key = ?1",
            [&base_key],
        )
        .map_err(db_error)?;
        for bookmark in &entry.book_bookmarks {
            let page_key = match bookmark.page_kind.as_str() {
                "relative_path" | "archive_entry" => {
                    bookmark.page_value.replace('\\', "/").to_lowercase()
                }
                "pdf_page" => bookmark.page_value.clone(),
                _ => {
                    return Err(TransferError::Invalid(
                        "本ブックマーク種別が不正です".into(),
                    ));
                }
            };
            tx.execute(
                "INSERT INTO book.book_bookmarks
                    (container_key, container_path, container_kind, page_kind, page_value,
                     page_key, page_index_hint, created_at_ms, title)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    base_key,
                    path.to_string_lossy().as_ref(),
                    bookmark.container_kind,
                    bookmark.page_kind,
                    bookmark.page_value,
                    page_key,
                    bookmark.page_index_hint.min(i64::MAX as usize) as i64,
                    bookmark.created_at_ms,
                    bookmark.title.as_deref(),
                ],
            )
            .map_err(db_error)?;
        }
    }
    tx.commit().map_err(db_error)
}

fn delete_key_family(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    base_key: &str,
) -> Result<(), TransferError> {
    let sql = format!(
        "DELETE FROM {table}
          WHERE {column} = ?1
             OR (substr({column}, 1, length(?1) + 2) = ?1 || '::')"
    );
    tx.execute(&sql, [base_key]).map_err(db_error)?;
    Ok(())
}

fn insert_rating(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    source_path: &Path,
    rating: &PortableRating,
) -> Result<(), TransferError> {
    tx.execute(
        "INSERT INTO ratings
            (path, stars, rated_at_ms, source_path, kind, entry_name, page_num,
             dir_prefix, archive_format, zipdir_is_archive, zipdir_representative)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            key,
            rating.stars,
            rating.rated_at_ms,
            source_path.to_string_lossy().as_ref(),
            rating.kind.as_deref().and_then(rating_kind_value),
            rating.entry_name.as_deref(),
            rating.page_num.map(i64::from),
            rating.dir_prefix.as_deref(),
            rating.archive_format.as_deref(),
            rating.zipdir_is_archive.map(i64::from),
            rating.zipdir_representative.as_deref(),
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

fn insert_tags(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    tags: &[String],
    fallback_time_ms: i64,
) -> Result<(), TransferError> {
    let applied_at = fallback_time_ms.div_euclid(1000);
    let collapsed = crate::tags_db::collapse_tags(tags, applied_at);
    for tag in collapsed {
        tx.execute(
            "INSERT INTO tags.item_tags (item_key, tag, tag_key, applied_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![key, tag.tag, tag.tag_key, tag.applied_at],
        )
        .map_err(db_error)?;
    }
    tx.execute(
        "INSERT INTO tags.tag_item_state (item_key, decided_at, source)
         VALUES (?1, ?2, ?3)",
        params![key, applied_at, crate::tags_db::source::METADATA_IMPORT],
    )
    .map_err(db_error)?;
    Ok(())
}

fn open_readonly(path: &Path) -> Result<Connection, TransferError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_error)?;
    Ok(conn)
}

fn write_manifest_atomic(
    root: &Path,
    manifest: &Manifest,
    cancel: &AtomicBool,
) -> Result<(), TransferError> {
    let destination = root.join(SIDECAR_FILENAME);
    let temp = root.join(format!(".{SIDECAR_FILENAME}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let file = File::create(&temp)
            .map_err(|error| TransferError::Io(format!("{}: {error}", temp.display())))?;
        let mut writer = BufWriter::new(file);
        {
            let mut cancel_writer = CancelWriter {
                inner: &mut writer,
                cancel,
                written: 0,
                max_bytes: MAX_SIDECAR_BYTES,
            };
            serde_json::to_writer_pretty(&mut cancel_writer, manifest).map_err(|error| {
                if cancel.load(Ordering::Relaxed) {
                    TransferError::Cancelled
                } else {
                    TransferError::Io(error.to_string())
                }
            })?;
        }
        writer
            .flush()
            .map_err(|error| TransferError::Io(error.to_string()))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| TransferError::Io(error.to_string()))?;
        check_cancel(cancel)?;
        replace_file_atomic(&temp, &destination)
            .map_err(|error| TransferError::Io(format!("{}: {error}", destination.display())))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(windows)]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let temp_wide: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(temp_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temp, destination)
}

fn relative_string(root: &Path, path: &Path) -> Result<String, TransferError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| TransferError::Invalid(format!("対象外のパスです: {}", path.display())))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(TransferError::Invalid(format!(
                "相対パスへ変換できません: {}",
                path.display()
            )));
        };
        let Some(value) = value.to_str() else {
            return Err(TransferError::Invalid(format!(
                "Unicodeで表せないファイル名です: {}",
                path.display()
            )));
        };
        parts.push(value);
    }
    Ok(parts.join("/"))
}

fn is_temp_sidecar_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        name.starts_with(&format!(".{SIDECAR_FILENAME}.")) && name.ends_with(".tmp")
    })
}

fn is_sidecar_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(SIDECAR_FILENAME))
}

fn rating_kind_name(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("image"),
        1 => Some("video"),
        2 => Some("folder"),
        3 => Some("zip_file"),
        4 => Some("pdf_file"),
        5 => Some("convertible_archive"),
        6 => Some("zip_image"),
        7 => Some("pdf_page"),
        8 => Some("zip_dir"),
        9 => Some("audio"),
        _ => None,
    }
}

fn rating_kind_value(value: &str) -> Option<i64> {
    match value {
        "image" => Some(0),
        "video" => Some(1),
        "folder" => Some(2),
        "zip_file" => Some(3),
        "pdf_file" => Some(4),
        "convertible_archive" => Some(5),
        "zip_image" => Some(6),
        "pdf_page" => Some(7),
        "zip_dir" => Some(8),
        "audio" => Some(9),
        _ => None,
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), TransferError> {
    if cancel.load(Ordering::Relaxed) {
        Err(TransferError::Cancelled)
    } else {
        Ok(())
    }
}

fn system_time_ms(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn now_ms() -> i64 {
    system_time_ms(SystemTime::now()).unwrap_or(0)
}

fn db_error(error: rusqlite::Error) -> TransferError {
    TransferError::Database(error.to_string())
}

struct CancelReader<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
}

impl<R: Read> Read for CancelReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "metadata transfer cancelled",
            ));
        }
        self.inner.read(buffer)
    }
}

struct CancelWriter<'a, W> {
    inner: W,
    cancel: &'a AtomicBool,
    written: u64,
    max_bytes: u64,
}

impl<W: Write> Write for CancelWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "metadata transfer cancelled",
            ));
        }
        let requested = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if self.written.saturating_add(requested) > self.max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "metadata sidecar exceeds {} MiB",
                    self.max_bytes / 1024 / 1024
                ),
            ));
        }
        let written = self.inner.write(buffer)?;
        self.written = self
            .written
            .saturating_add(u64::try_from(written).unwrap_or(u64::MAX));
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_progress(_: TransferProgress) {}

    fn init_data_dir(path: &Path) {
        ensure_database_schemas(path).unwrap();
    }

    fn set_rating(data_dir: &Path, path: &Path, stars: i64) {
        let conn = Connection::open(data_dir.join("rating.db")).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO ratings (path, stars, rated_at_ms, source_path, kind)
             VALUES (?1, ?2, 1234, ?3, 0)",
            params![
                crate::path_key::normalize_keep_drive(path),
                stars,
                path.to_string_lossy().as_ref()
            ],
        )
        .unwrap();
    }

    fn rating(data_dir: &Path, path: &Path) -> Option<i64> {
        let conn = Connection::open(data_dir.join("rating.db")).unwrap();
        conn.query_row(
            "SELECT stars FROM ratings WHERE path = ?1",
            [crate::path_key::normalize_keep_drive(path)],
            |row| row.get(0),
        )
        .ok()
    }

    fn set_tags(data_dir: &Path, path: &Path, tags: &[&str]) {
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(
            &crate::path_key::normalize_keep_drive(path),
            tags,
            crate::tags_db::source::EDIT,
        )
        .unwrap();
    }

    fn tags(data_dir: &Path, path: &Path) -> Vec<String> {
        crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
            .unwrap()
            .get_item_tags(&crate::path_key::normalize_keep_drive(path))
            .into_iter()
            .map(|tag| tag.tag)
            .collect()
    }

    #[test]
    fn round_trip_maps_physical_and_virtual_metadata_to_new_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(source.join("a.jpg"), b"abc").unwrap();
        fs::write(destination.join("a.jpg"), b"abc").unwrap();
        fs::write(source.join("nested/book.zip"), b"zip").unwrap();
        fs::write(destination.join("nested/book.zip"), b"zip").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        set_rating(&source_data, &source.join("a.jpg"), 4);
        set_tags(&source_data, &source.join("a.jpg"), &["旅行", "青"]);
        let virtual_source_key = format!(
            "{}::page/001.jpg",
            crate::path_key::normalize_keep_drive(&source.join("nested/book.zip"))
        );
        let conn = Connection::open(source_data.join("rating.db")).unwrap();
        conn.execute(
            "INSERT INTO ratings (path, stars, rated_at_ms, source_path, kind, entry_name)
             VALUES (?1, 5, 5678, ?2, 6, 'page/001.jpg')",
            params![
                virtual_source_key,
                source.join("nested/book.zip").to_string_lossy().as_ref()
            ],
        )
        .unwrap();
        let mut tags_db = crate::tags_db::TagsDb::open_at(&source_data.join("tags.db")).unwrap();
        tags_db
            .set_item_tags(
                &virtual_source_key,
                ["お気に入り"],
                crate::tags_db::source::EDIT,
            )
            .unwrap();
        let video = Connection::open(source_data.join("video_bookmarks.db")).unwrap();
        video
            .execute(
                "INSERT INTO video_bookmarks (path, pts_secs, title, created_at)
                 VALUES (?1, 12.5, '場面', 99)",
                [crate::path_key::normalize_keep_drive(&source.join("a.jpg"))],
            )
            .unwrap();
        let book = Connection::open(source_data.join("book_bookmarks.db")).unwrap();
        let book_path = source.join("nested/book.zip");
        book.execute(
            "INSERT INTO book_bookmarks
                (container_key, container_path, container_kind, page_kind, page_value,
                 page_key, page_index_hint, created_at_ms, title)
             VALUES (?1, ?2, 'zip', 'archive_entry', 'page/001.jpg',
                     'page/001.jpg', 0, 777, '表紙')",
            params![
                crate::path_key::normalize_keep_drive(&book_path),
                book_path.to_string_lossy().as_ref()
            ],
        )
        .unwrap();

        let cancel = AtomicBool::new(false);
        let exported = export_at(&source_data, &source, true, &cancel, no_progress).unwrap();
        assert_eq!(exported.ratings, 2);
        fs::copy(
            source.join(SIDECAR_FILENAME),
            destination.join(SIDECAR_FILENAME),
        )
        .unwrap();
        // 実アプリと同様、各 DB のアイドル接続が開いたままでも別 worker 接続から
        // attached transaction を実行できることを確認する。
        let _open_rating =
            crate::rating_db::RatingDb::open_at(destination_data.join("rating.db")).unwrap();
        let _open_tags =
            crate::tags_db::TagsDb::open_at(&destination_data.join("tags.db")).unwrap();
        let _open_video = crate::video_bookmarks::VideoBookmarkDb::open_at(
            &destination_data.join("video_bookmarks.db"),
        )
        .unwrap();
        let _open_book = Connection::open(destination_data.join("book_bookmarks.db")).unwrap();
        let imported = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(imported.failed_entries, 0);
        assert_eq!(
            rating(&destination_data, &destination.join("a.jpg")),
            Some(4)
        );
        assert_eq!(
            tags(&destination_data, &destination.join("a.jpg")),
            vec!["旅行", "青"]
        );

        let virtual_destination_key = format!(
            "{}::page/001.jpg",
            crate::path_key::normalize_keep_drive(&destination.join("nested/book.zip"))
        );
        let conn = Connection::open(destination_data.join("rating.db")).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT stars FROM ratings WHERE path = ?1",
                [&virtual_destination_key],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            5
        );
        let virtual_tags = crate::tags_db::TagsDb::open_at(&destination_data.join("tags.db"))
            .unwrap()
            .get_item_tags(&virtual_destination_key)
            .into_iter()
            .map(|tag| tag.tag)
            .collect::<Vec<_>>();
        assert_eq!(virtual_tags, vec!["お気に入り"]);
        let video = Connection::open(destination_data.join("video_bookmarks.db")).unwrap();
        let video_row = video
            .query_row(
                "SELECT path, pts_secs, title, created_at FROM video_bookmarks",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            video_row,
            (
                crate::path_key::normalize_keep_drive(&destination.join("a.jpg")),
                12.5,
                "場面".to_string(),
                99,
            )
        );
        let book = Connection::open(destination_data.join("book_bookmarks.db")).unwrap();
        let book_row = book
            .query_row(
                "SELECT container_key, container_path, page_value, title FROM book_bookmarks",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            book_row,
            (
                crate::path_key::normalize_keep_drive(&destination.join("nested/book.zip")),
                destination
                    .join("nested/book.zip")
                    .to_string_lossy()
                    .into_owned(),
                "page/001.jpg".to_string(),
                "表紙".to_string(),
            )
        );
    }

    #[test]
    fn import_clears_listed_empty_metadata_and_preserves_unlisted_items() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("listed.jpg"), b"x").unwrap();
        fs::write(destination.join("listed.jpg"), b"x").unwrap();
        fs::write(destination.join("unlisted.jpg"), b"y").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&destination_data, &destination.join("listed.jpg"), 5);
        set_tags(
            &destination_data,
            &destination.join("listed.jpg"),
            &["古い"],
        );
        let stale_virtual_key = format!(
            "{}::stale-page",
            crate::path_key::normalize_keep_drive(&destination.join("listed.jpg"))
        );
        Connection::open(destination_data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings (path, stars) VALUES (?1, 4)",
                [&stale_virtual_key],
            )
            .unwrap();
        crate::tags_db::TagsDb::open_at(&destination_data.join("tags.db"))
            .unwrap()
            .set_item_tags(
                &stale_virtual_key,
                ["古いページ"],
                crate::tags_db::source::EDIT,
            )
            .unwrap();
        set_rating(&destination_data, &destination.join("unlisted.jpg"), 3);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        fs::copy(
            source.join(SIDECAR_FILENAME),
            destination.join(SIDECAR_FILENAME),
        )
        .unwrap();
        import_at(&destination_data, &destination, &cancel, no_progress).unwrap();

        assert_eq!(
            rating(&destination_data, &destination.join("listed.jpg")),
            None
        );
        assert!(tags(&destination_data, &destination.join("listed.jpg")).is_empty());
        assert_eq!(
            Connection::open(destination_data.join("rating.db"))
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM ratings WHERE path = ?1",
                    [&stale_virtual_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert!(
            crate::tags_db::TagsDb::open_at(&destination_data.join("tags.db"))
                .unwrap()
                .get_item_tags(&stale_virtual_key)
                .is_empty()
        );
        assert_eq!(
            rating(&destination_data, &destination.join("unlisted.jpg")),
            Some(3)
        );
    }

    #[test]
    fn nonrecursive_export_does_not_include_nested_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("top.jpg"), b"x").unwrap();
        fs::write(root.join("nested/deep.jpg"), b"y").unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        let summary = export_at(&data, &root, false, &cancel, no_progress).unwrap();
        assert_eq!(summary.entries, 3); // root + immediate file + immediate directory
        let manifest = read_manifest(&root, &cancel).unwrap();
        assert!(
            !manifest
                .entries
                .iter()
                .any(|entry| entry.path == "nested/deep.jpg")
        );
    }

    #[test]
    fn recursive_export_rejects_excessive_depth_instead_of_truncating() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        let mut nested = root.clone();
        for index in 0..=MAX_RECURSION_DEPTH {
            nested.push(format!("d{index}"));
            fs::create_dir(&nested).unwrap();
        }
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            export_at(&data, &root, true, &cancel, no_progress),
            Err(TransferError::Invalid(message)) if message.contains("深すぎます")
        ));
        assert!(!root.join(SIDECAR_FILENAME).exists());
    }

    #[test]
    fn sidecar_writer_rejects_data_beyond_its_limit() {
        let cancel = AtomicBool::new(false);
        let mut output = Vec::new();
        let mut writer = CancelWriter {
            inner: &mut output,
            cancel: &cancel,
            written: 0,
            max_bytes: 3,
        };
        assert!(writer.write_all(b"1234").is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn export_cancel_preserves_existing_sidecar() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.jpg"), b"x").unwrap();
        fs::write(root.join(SIDECAR_FILENAME), b"old").unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Cancelled)
        ));
        assert_eq!(fs::read(root.join(SIDECAR_FILENAME)).unwrap(), b"old");
    }

    #[test]
    fn import_cancel_keeps_completed_entries_without_touching_remaining_entries() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in ["a.jpg", "b.jpg"] {
            fs::write(source.join(name), b"x").unwrap();
            fs::write(destination.join(name), b"x").unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&source_data, &source.join("a.jpg"), 1);
        set_rating(&source_data, &source.join("b.jpg"), 2);
        set_rating(&destination_data, &destination.join("a.jpg"), 4);
        set_rating(&destination_data, &destination.join("b.jpg"), 5);

        let export_cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &export_cancel, no_progress).unwrap();
        fs::copy(
            source.join(SIDECAR_FILENAME),
            destination.join(SIDECAR_FILENAME),
        )
        .unwrap();

        let import_cancel = AtomicBool::new(false);
        let summary = import_at(
            &destination_data,
            &destination,
            &import_cancel,
            |progress| {
                // 並びは root, a.jpg, b.jpg。a.jpg の transaction 完了後に止める。
                if progress.phase == TransferPhase::Importing && progress.processed == 2 {
                    import_cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.applied_entries, 2);
        assert_eq!(
            rating(&destination_data, &destination.join("a.jpg")),
            Some(1)
        );
        assert_eq!(
            rating(&destination_data, &destination.join("b.jpg")),
            Some(5)
        );
    }

    #[test]
    fn changed_file_is_reported_and_skipped() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("a.jpg"), b"short").unwrap();
        fs::write(destination.join("a.jpg"), b"different-size").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&source_data, &source.join("a.jpg"), 1);
        set_rating(&destination_data, &destination.join("a.jpg"), 5);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        fs::copy(
            source.join(SIDECAR_FILENAME),
            destination.join(SIDECAR_FILENAME),
        )
        .unwrap();
        let preview = inspect_import_at(&destination, &cancel, no_progress).unwrap();
        assert_eq!(preview.changed_files, 1);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.skipped_changed, 1);
        assert_eq!(
            rating(&destination_data, &destination.join("a.jpg")),
            Some(5)
        );
    }

    #[test]
    fn rejects_parent_path_and_newer_version() {
        let base = Manifest {
            format: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            exported_at_ms: 0,
            recursive: false,
            sections: ManifestSections::default(),
            entries: vec![PortableEntry {
                path: "../escape.jpg".to_string(),
                kind: PortableEntryKind::File,
                fingerprint: Some(FileFingerprint {
                    size: 1,
                    modified_ms: None,
                }),
                rating: None,
                tags: Vec::new(),
                timed_bookmarks: Vec::new(),
                book_bookmarks: Vec::new(),
                virtual_items: Vec::new(),
            }],
        };
        assert!(matches!(
            validate_manifest(&base),
            Err(TransferError::Invalid(_))
        ));
        let mut newer = base;
        newer.entries.clear();
        newer.version += 1;
        assert!(matches!(
            validate_manifest(&newer),
            Err(TransferError::Invalid(_))
        ));
    }

    #[test]
    fn import_rejects_existing_reparse_path_outside_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.jpg"), b"x").unwrap();
        let link = root.join("link");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            // Developer Mode / symlink privilege がない環境では作成不能。
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let manifest = Manifest {
            format: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            exported_at_ms: 0,
            recursive: true,
            sections: ManifestSections::default(),
            entries: vec![PortableEntry {
                path: "link/secret.jpg".to_string(),
                kind: PortableEntryKind::File,
                fingerprint: Some(FileFingerprint {
                    size: 1,
                    modified_ms: None,
                }),
                rating: None,
                tags: Vec::new(),
                timed_bookmarks: Vec::new(),
                book_bookmarks: Vec::new(),
                virtual_items: Vec::new(),
            }],
        };
        fs::write(
            root.join(SIDECAR_FILENAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            inspect_import_at(&root, &cancel, no_progress),
            Err(TransferError::Invalid(message)) if message.contains("フォルダ外")
        ));
    }
}
