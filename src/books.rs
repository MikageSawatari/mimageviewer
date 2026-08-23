use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use eframe::egui;

use crate::book_fs_journal::{
    BookFileIdentity, BookFsOperationPlan, BookFsStep, execute_forward, execute_rollback,
};

pub const DEFAULT_BOOK_NAME: &str = "名前なし";
pub const MAX_BOOK_PAGES: usize = 9999;

#[derive(Clone, Debug)]
pub struct BookInfo {
    pub name: String,
    pub path: PathBuf,
    pub page_count: usize,
}

#[derive(Clone, Debug)]
pub struct BookPageEntry {
    pub path: PathBuf,
    pub display_name: String,
}

#[derive(Debug)]
pub struct BookAppendSummary {
    pub book_name: String,
    pub folder: PathBuf,
    pub added: usize,
    pub first_path: Option<PathBuf>,
    pub edit_copies: Vec<BookPathMapping>,
    pub semantic_copies: Vec<BookPathMapping>,
    pub erase_fallback_pages: usize,
}

#[derive(Clone, Debug)]
pub struct BookPathMapping {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookTransferKind {
    Copy,
    Move,
}

#[derive(Debug)]
pub struct BookTransferSummary {
    pub source_folder: PathBuf,
    pub target_book_name: String,
    pub target_folder: PathBuf,
    pub pages: usize,
    pub kind: BookTransferKind,
    pub source_entries: Vec<BookPageEntry>,
    pub edit_moves: Vec<BookPathMapping>,
    pub edit_copies: Vec<BookPathMapping>,
    /// Filesystem 変更前に永続化した本ブックマーク移行 intent。
    /// UI は `edit_moves` と一緒に bookmark worker へ渡し、DB commit と同時に消去する。
    pub bookmark_migration_journal_id: Option<String>,
}

#[derive(Debug)]
pub enum BookOpResult {
    Append(BookAppendSummary),
    Transfer(BookTransferSummary),
    List(Vec<BookInfo>),
    Created {
        name: String,
        path: PathBuf,
    },
    Renamed {
        old_name: String,
        new_name: String,
        edit_moves: Vec<BookPathMapping>,
        bookmark_migration_journal_id: Option<String>,
    },
    Deleted {
        name: String,
    },
    Reordered {
        folder: PathBuf,
        count: usize,
        edit_moves: Vec<BookPathMapping>,
        bookmark_migration_journal_id: Option<String>,
    },
}

/// Filesystem と SQLite を跨ぐ本ページ移動の phase-aware journal owner。
/// Drop で消せるのは filesystem decision 前の Prepared だけ。Applying 以降は
/// recovery が forward/rollback plan を再開できるよう必ず行を残す。
struct PreparedBookmarkMigration {
    journal: Option<crate::book_bookmarks::PathMigrationJournalWriter>,
    discard_if_prepared: bool,
}

impl PreparedBookmarkMigration {
    fn prepare(
        job_id: String,
        mappings: &[BookPathMapping],
        plan: &BookFsOperationPlan,
    ) -> Result<Self, String> {
        let mappings = mappings
            .iter()
            .map(|mapping| (mapping.from.clone(), mapping.to.clone()))
            .collect::<Vec<_>>();
        let journal = crate::book_bookmarks::prepare_path_migration(&job_id, &mappings, plan)
            .map_err(|error| format!("本ブックマークの移行準備に失敗しました: {error}"))?;
        Ok(Self {
            journal,
            discard_if_prepared: true,
        })
    }

    fn execute(mut self, plan: &BookFsOperationPlan) -> Result<Option<String>, String> {
        let Some(journal) = self.journal.take() else {
            execute_forward(plan, 0, |_| Ok(())).map_err(|error| error.message)?;
            return Ok(None);
        };
        let job_id = journal.job_id().to_string();
        if let Err(error) = journal.begin() {
            if let Err(discard_error) = journal.discard_prepared() {
                crate::logger::log(format!(
                    "book bookmark prepared journal discard failed job={job_id}: {discard_error}"
                ));
            }
            return Err(format!("filesystem 適用開始を記録できません: {error}"));
        }
        self.discard_if_prepared = false;

        let forward = execute_forward(plan, 0, |next_step| {
            journal.record_progress(false, next_step)
        });
        if let Err(error) = forward {
            // The rollback decision itself must be durable before touching the
            // filesystem in reverse. If this write fails, Applying is retained
            // and startup recovery safely completes forward instead.
            if let Err(phase_error) = journal.begin_rollback(error.affected_steps, &error.message) {
                return Err(format!(
                    "{} / rollback 開始を記録できないため recovery に保持しました: {phase_error}",
                    error.message
                ));
            }
            match execute_rollback(plan, error.affected_steps, |next_step| {
                journal.record_progress(true, next_step)
            }) {
                Ok(()) => {
                    journal.discard_rolled_back().map_err(|discard_error| {
                            format!(
                                "{} / rollback は完了しましたが journal を破棄できません: {discard_error}",
                                error.message
                            )
                        })?;
                    return Err(error.message);
                }
                Err(rollback_error) => {
                    return Err(format!(
                        "{} / rollback も失敗したため journal を保持しました: {}",
                        error.message, rollback_error.message
                    ));
                }
            }
        }

        journal
            .mark_filesystem_committed(plan.len())
            .map_err(|error| {
                format!("filesystem commit 済み状態を記録できません（journal を保持）: {error}")
            })?;
        self.discard_if_prepared = false;
        Ok(Some(journal.into_job_id()))
    }
}

impl Drop for PreparedBookmarkMigration {
    fn drop(&mut self) {
        if !self.discard_if_prepared {
            return;
        }
        let Some(journal) = self.journal.as_ref() else {
            return;
        };
        if let Err(error) = journal.discard_prepared() {
            crate::logger::log(format!(
                "book bookmark migration journal discard failed job={}: {error}",
                journal.job_id()
            ));
        }
    }
}

pub struct BookOpPending {
    pub rx: std::sync::mpsc::Receiver<Result<BookOpResult, String>>,
}

pub enum BookPageSource {
    File {
        src: PathBuf,
        original_name: String,
    },
    ZipEntry {
        zip_path: PathBuf,
        entry_name: String,
        original_name: String,
    },
    /// UI thread で edit DB / settings / comic assets を snapshot 済みの headless 合成。
    Composited {
        source: CompositeSource,
        basename: String,
        edits: BakedEditSnapshot,
    },
    Rendered {
        work: crate::capture::CapturePixelWork,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
    VideoFrame {
        path: PathBuf,
        target_secs: f64,
        basename: String,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
    ClipboardImage {
        basename: String,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
}

pub enum CompositeSource {
    File {
        path: PathBuf,
    },
    ZipEntry {
        zip_path: PathBuf,
        entry_name: String,
    },
    PdfPage {
        pdf_path: PathBuf,
        page_num: u32,
        password: Option<String>,
    },
}

#[derive(Clone)]
pub struct BookMaskSnapshot {
    pub bitmap: Vec<bool>,
    pub shapes: Vec<crate::mask_db::Shape>,
    pub size: [usize; 2],
}

pub struct BookEraseResult {
    pub image: egui::ColorImage,
    pub used_diffusion_fallback: bool,
}

pub type BookEraseRunner = Box<
    dyn Fn(
            &egui::ColorImage,
            &[bool],
            &[crate::mask_db::Shape],
            &Arc<AtomicBool>,
        ) -> Result<BookEraseResult, String>
        + Send,
>;

pub struct BookEraseSnapshot {
    pub mask: BookMaskSnapshot,
    pub run: BookEraseRunner,
}

pub struct BookConcealSnapshot {
    pub mask: BookMaskSnapshot,
    pub preset: crate::conceal::ConcealPreset,
}

pub struct BookComicSnapshot {
    pub objects: Vec<comic_core::AnnotationObject>,
    pub fonts: Arc<comic_core::FontSet>,
    pub stamp_cache: std::collections::HashMap<String, Option<Arc<comic_core::RgbaOverlay>>>,
}

pub struct BakedEditSnapshot {
    pub params: crate::adjustment::AdjustParams,
    pub rotation: crate::rotation_db::Rotation,
    pub conceal: Option<BookConcealSnapshot>,
    pub erase: Option<BookEraseSnapshot>,
    pub local_adjust: Option<Vec<local_adjust_core::LocalAdjustmentLayer>>,
    pub comic: Option<BookComicSnapshot>,
    pub comic_source_dims: Option<[usize; 2]>,
    pub export_crop: Option<crate::export_crop::CropSettings>,
    /// idx を持たない stack member の legacy crop を、decode 後の最初のラスタ寸法で
    /// 中央 DB へ遅延移行するための `(db_path, page_key)`。
    pub crop_legacy_writeback: Option<(PathBuf, String)>,
    pub format: crate::capture::CaptureFormat,
    pub jpeg_matte: crate::capture::JpegMatte,
}

pub fn page_requires_full_composite(
    params: &crate::adjustment::AdjustParams,
    rotation: crate::rotation_db::Rotation,
    has_conceal: bool,
    has_erase: bool,
    has_local_adjust: bool,
    has_comic: bool,
    has_export_crop: bool,
) -> bool {
    !params.is_color_identity()
        || !rotation.is_none()
        || has_conceal
        || has_erase
        || has_local_adjust
        || has_comic
        || has_export_crop
}

pub fn default_books_root() -> PathBuf {
    crate::capture::default_output_dir().join("books")
}

pub fn settings_books_root(settings: &crate::settings::Settings) -> PathBuf {
    settings
        .book_root
        .clone()
        .unwrap_or_else(default_books_root)
}

pub fn normalize_book_name(input: &str) -> String {
    sanitize_filename(input, DEFAULT_BOOK_NAME)
}

pub fn book_folder(root: &Path, name: &str) -> PathBuf {
    root.join(normalize_book_name(name))
}

pub fn active_book_folder(settings: &crate::settings::Settings) -> PathBuf {
    book_folder(&settings_books_root(settings), &settings.active_book_name)
}

pub fn path_is_under_or_equal(path: &Path, root: &Path) -> bool {
    let path_norm = crate::search_index_db::normalize_path(path);
    let root_norm = crate::search_index_db::normalize_path(root);
    path_norm == root_norm
        || path_norm.starts_with(&(root_norm.trim_end_matches('/').to_owned() + "/"))
}

pub fn path_is_under_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path_is_under_or_equal(path, root))
}

pub fn is_direct_book_folder(root: &Path, path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| crate::folder_tree::path_eq(parent, root))
}

pub fn containing_book_folder(root: &Path, path: &Path) -> Option<PathBuf> {
    let parent = if path.is_dir() { path } else { path.parent()? };
    if is_direct_book_folder(root, parent) {
        return Some(parent.to_path_buf());
    }
    None
}

pub fn list_books(root: &Path) -> Result<Vec<BookInfo>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in
        fs::read_dir(root).map_err(|e| format!("本棚を読み取れません: {}: {e}", root.display()))?
    {
        let entry = entry.map_err(|e| format!("本棚の項目を読み取れません: {e}"))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|e| format!("本棚の項目種別を読み取れません: {}: {e}", path.display()))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        rows.push(BookInfo {
            page_count: book_page_count(&path)?,
            path,
            name,
        });
    }
    rows.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(rows)
}

/// 追加完了結果を UI が保持する本棚一覧へ反映する。
///
/// 一覧を `None` にして再走査すると、その間だけ固定本棚ボタンが全て消えてツールバーが
/// 縮み、再取得時に元へ戻るちらつきになる。Append は追加先と件数が確定しているため、
/// キャッシュを保ったまま件数だけ更新できる。
pub fn apply_append_to_cached_list(
    rows: &mut Vec<BookInfo>,
    book_name: &str,
    folder: &Path,
    added: usize,
) {
    if let Some(row) = rows.iter_mut().find(|row| row.name == book_name) {
        row.path = folder.to_path_buf();
        row.page_count = row.page_count.saturating_add(added);
        return;
    }
    rows.push(BookInfo {
        name: book_name.to_string(),
        path: folder.to_path_buf(),
        page_count: added,
    });
    rows.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
}

pub fn create_book(root: &Path, name: &str) -> Result<BookOpResult, String> {
    let name = normalize_book_name(name);
    let path = book_folder(root, &name);
    if path.exists() {
        return Err(format!("同名の本が既にあります: {name}"));
    }
    fs::create_dir_all(root)
        .map_err(|e| format!("本棚フォルダを作成できません: {}: {e}", root.display()))?;
    fs::create_dir(&path).map_err(|e| format!("本を作成できません: {}: {e}", path.display()))?;
    Ok(BookOpResult::Created { name, path })
}

pub fn rename_book(root: &Path, old_name: &str, new_name: &str) -> Result<BookOpResult, String> {
    let old_name = normalize_book_name(old_name);
    let new_name = normalize_book_name(new_name);
    if old_name == new_name {
        return Ok(BookOpResult::Renamed {
            old_name,
            new_name,
            edit_moves: Vec::new(),
            bookmark_migration_journal_id: None,
        });
    }
    let from = book_folder(root, &old_name);
    let to = book_folder(root, &new_name);
    ensure_direct_book_target(root, &from)?;
    if to.exists() {
        return Err(format!("同名の本が既にあります: {new_name}"));
    }
    let edit_moves = book_page_paths(&from)?
        .into_iter()
        .filter_map(|old_path| {
            let name = old_path.file_name()?.to_owned();
            Some(BookPathMapping {
                from: old_path,
                to: to.join(name),
            })
        })
        .collect::<Vec<_>>();
    let job_id = crate::book_bookmarks::new_path_migration_job_id();
    let plan = BookFsOperationPlan::new(vec![BookFsStep::Rename {
        from: from.clone(),
        to: to.clone(),
    }]);
    let migration = PreparedBookmarkMigration::prepare(job_id, &edit_moves, &plan)?;
    let bookmark_migration_journal_id = migration.execute(&plan)?;
    Ok(BookOpResult::Renamed {
        old_name,
        new_name,
        edit_moves,
        bookmark_migration_journal_id,
    })
}

pub fn delete_book(root: &Path, name: &str) -> Result<BookOpResult, String> {
    let name = normalize_book_name(name);
    let path = book_folder(root, &name);
    ensure_direct_book_target(root, &path)?;
    fs::remove_dir_all(&path)
        .map_err(|e| format!("本を削除できません: {}: {e}", path.display()))?;
    Ok(BookOpResult::Deleted { name })
}

pub fn append_pages(
    root: PathBuf,
    book_name: String,
    sources: Vec<BookPageSource>,
) -> Result<BookOpResult, String> {
    append_pages_at(crate::data_dir::get(), root, book_name, sources)
}

pub(crate) fn append_pages_at(
    data_dir: PathBuf,
    root: PathBuf,
    book_name: String,
    sources: Vec<BookPageSource>,
) -> Result<BookOpResult, String> {
    if sources.is_empty() {
        return Err("追加するページがありません".to_string());
    }
    let added = sources.len();
    let book_name = normalize_book_name(&book_name);
    let folder = book_folder(&root, &book_name);
    fs::create_dir_all(&folder)
        .map_err(|e| format!("本フォルダを作成できません: {}: {e}", folder.display()))?;
    ensure_direct_book_target(&root, &folder)?;

    let start = next_page_number(&folder)?;
    if start + sources.len() - 1 > MAX_BOOK_PAGES {
        return Err(format!(
            "本のページ数が上限 {} を超えます (現在 {}, 追加 {})",
            MAX_BOOK_PAGES,
            start.saturating_sub(1),
            sources.len()
        ));
    }

    let mut first_path = None;
    let mut edit_copies = Vec::new();
    let mut semantic_copies = Vec::new();
    let mut erase_fallback_pages = 0;
    let mut restore_declines =
        crate::content_identity::InternalByteCopyDeclineRecorder::new(&data_dir);
    for (offset, source) in sources.into_iter().enumerate() {
        let page_no = start + offset;
        let dest = destination_for_source(&folder, page_no, &source)?;
        let byte_copy_source = match &source {
            BookPageSource::File { src, .. } => Some(src.clone()),
            _ => None,
        };
        if let Some(copy) = source_edit_copy_path(&root, &folder, &source) {
            let mapping = BookPathMapping {
                from: copy.path().to_path_buf(),
                to: dest.clone(),
            };
            match copy {
                BookSourceCopy::Full(_) => edit_copies.push(mapping),
                BookSourceCopy::Semantic(_) => semantic_copies.push(mapping),
            }
        }
        if write_source(source, &dest)? {
            erase_fallback_pages += 1;
        }
        if let Some(source_path) = byte_copy_source {
            restore_declines.record(&source_path, &dest);
        }
        if first_path.is_none() {
            first_path = Some(dest);
        }
    }
    let restore_declines = restore_declines.finish();
    crate::logger::log(format!(
        "book append restore declines: requested={} recorded={} existing={} source_untracked={} source_hash_unavailable={} errors={}",
        restore_declines.requested,
        restore_declines.recorded,
        restore_declines.already_recorded,
        restore_declines.source_not_tracked,
        restore_declines.source_hash_unavailable,
        restore_declines.errors.len(),
    ));

    Ok(BookOpResult::Append(BookAppendSummary {
        book_name,
        folder,
        added,
        first_path,
        edit_copies,
        semantic_copies,
        erase_fallback_pages,
    }))
}

pub fn flush_reorder(folder: PathBuf, ordered_paths: Vec<PathBuf>) -> Result<BookOpResult, String> {
    let edit_moves = plan_reorder_paths(&folder, &ordered_paths)?;
    let job_id = crate::book_bookmarks::new_path_migration_job_id();
    let plan = BookFsOperationPlan::new(plan_reorder_filesystem_steps(
        &folder,
        &edit_moves,
        &job_id,
        "reorder",
    )?);
    let migration = PreparedBookmarkMigration::prepare(job_id, &edit_moves, &plan)?;
    let bookmark_migration_journal_id = migration.execute(&plan)?;
    let count = edit_moves.len();
    Ok(BookOpResult::Reordered {
        folder,
        count,
        edit_moves,
        bookmark_migration_journal_id,
    })
}

pub fn transfer_pages_between_books(
    root: PathBuf,
    source_folder: PathBuf,
    current_order_paths: Vec<PathBuf>,
    selected_paths: Vec<PathBuf>,
    target_book_name: String,
    kind: BookTransferKind,
) -> Result<BookOpResult, String> {
    if selected_paths.is_empty() {
        return Err("移動/コピーするページがありません".to_string());
    }
    if current_order_paths.is_empty() {
        return Err("本にページがありません".to_string());
    }
    ensure_direct_book_target(&root, &source_folder)?;
    let target_book_name = normalize_book_name(&target_book_name);
    let target_folder = book_folder(&root, &target_book_name);
    ensure_direct_book_target(&root, &target_folder)?;
    if crate::folder_tree::path_eq(&source_folder, &target_folder) {
        return Err("同じ本への移動/コピーはまだ対応していません".to_string());
    }

    let selected_keys = selected_paths
        .iter()
        .map(|path| crate::search_index_db::normalize_path(path))
        .collect::<HashSet<_>>();
    let current_keys = current_order_paths
        .iter()
        .map(|path| crate::search_index_db::normalize_path(path))
        .collect::<HashSet<_>>();
    if !selected_keys.iter().all(|key| current_keys.contains(key)) {
        return Err("選択ページが現在の本に見つかりません".to_string());
    }
    let start = next_page_number(&target_folder)?;
    if start + selected_paths.len() - 1 > MAX_BOOK_PAGES {
        return Err(format!(
            "移動先の本のページ数が上限 {} を超えます (現在 {}, 追加 {})",
            MAX_BOOK_PAGES,
            start.saturating_sub(1),
            selected_paths.len()
        ));
    }

    // commit / transfer / compaction の最終 mapping を、最初の filesystem 変更前に確定する。
    // 一時ファイル名は journal に含めない。
    let commit_mappings = plan_reorder_paths(&source_folder, &current_order_paths)?;
    let committed_paths = commit_mappings
        .iter()
        .map(|mapping| mapping.to.clone())
        .collect::<Vec<_>>();
    let mut selected_original = Vec::new();
    let mut selected_committed = Vec::new();
    let mut remaining_committed = Vec::new();
    for (old_path, committed_path) in current_order_paths.iter().zip(committed_paths.iter()) {
        if selected_keys.contains(&crate::search_index_db::normalize_path(old_path)) {
            selected_original.push(old_path.clone());
            selected_committed.push(committed_path.clone());
        } else {
            remaining_committed.push(committed_path.clone());
        }
    }
    if selected_committed.is_empty() {
        return Err("選択ページが現在の本に見つかりません".to_string());
    }

    let mut transfer_mappings = Vec::with_capacity(selected_committed.len());
    for (offset, src) in selected_committed.iter().enumerate() {
        let dest = destination_for_existing_page(&target_folder, start + offset, src)?;
        if !filesystem_path_is_missing(&dest)? {
            return Err(format!("移動先ページが既に存在します: {}", dest.display()));
        }
        transfer_mappings.push(BookPathMapping {
            from: src.clone(),
            to: dest,
        });
    }

    let edit_copies = if kind == BookTransferKind::Copy {
        transfer_mappings.clone()
    } else {
        Vec::new()
    };

    let compact_mappings = if kind == BookTransferKind::Move {
        plan_reorder_paths(&source_folder, &remaining_committed)?
    } else {
        Vec::new()
    };
    // Move may run commit -> transfer -> compaction in one worker. Collapse those
    // phases into original-key -> final-key mappings before the DB remap layer
    // applies its simultaneous two-pass move.
    let edit_moves = if kind == BookTransferKind::Move {
        compose_book_path_mapping_phases(&[&commit_mappings, &transfer_mappings, &compact_mappings])
    } else {
        commit_mappings.clone()
    };
    let job_id = crate::book_bookmarks::new_path_migration_job_id();
    let mut filesystem_steps = Vec::new();
    if filesystem_path_is_missing(&target_folder)? {
        filesystem_steps.push(BookFsStep::CreateDir {
            path: target_folder.clone(),
        });
    }
    filesystem_steps.extend(plan_reorder_filesystem_steps(
        &source_folder,
        &commit_mappings,
        &job_id,
        "commit",
    )?);
    for (transfer_idx, (original, mapping)) in
        selected_original.iter().zip(&transfer_mappings).enumerate()
    {
        let identity = BookFileIdentity::read(original)
            .map_err(|error| format!("移動元ページの identity を読み取れません: {error}"))?;
        let staging = journal_transfer_staging_path(&mapping.to, &job_id, transfer_idx, "forward")?;
        if !filesystem_path_is_missing(&staging)? {
            return Err(format!(
                "転送用の一時ファイルが既に存在します: {}",
                staging.display()
            ));
        }
        filesystem_steps.push(match kind {
            BookTransferKind::Copy => BookFsStep::CopyFile {
                from: mapping.from.clone(),
                to: mapping.to.clone(),
                staging: Some(staging),
                identity,
            },
            BookTransferKind::Move => {
                let rollback_staging = journal_transfer_staging_path(
                    &mapping.from,
                    &job_id,
                    transfer_idx,
                    "rollback",
                )?;
                if !filesystem_path_is_missing(&rollback_staging)? {
                    return Err(format!(
                        "復旧用の一時ファイルが既に存在します: {}",
                        rollback_staging.display()
                    ));
                }
                BookFsStep::MoveFile {
                    from: mapping.from.clone(),
                    to: mapping.to.clone(),
                    staging: Some(staging),
                    rollback_staging: Some(rollback_staging),
                    identity,
                }
            }
        });
    }
    if kind == BookTransferKind::Move {
        filesystem_steps.extend(plan_reorder_filesystem_steps(
            &source_folder,
            &compact_mappings,
            &job_id,
            "compact",
        )?);
    }
    let plan = BookFsOperationPlan::new(filesystem_steps);
    let migration = PreparedBookmarkMigration::prepare(job_id, &edit_moves, &plan)?;
    let bookmark_migration_journal_id = migration.execute(&plan)?;

    let source_after_paths = if kind == BookTransferKind::Move {
        compact_mappings
            .iter()
            .map(|mapping| mapping.to.clone())
            .collect::<Vec<_>>()
    } else {
        committed_paths
    };
    let source_entries = source_after_paths
        .into_iter()
        .map(|path| {
            let display_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("page")
                .to_string();
            BookPageEntry { path, display_name }
        })
        .collect::<Vec<_>>();

    Ok(BookOpResult::Transfer(BookTransferSummary {
        source_folder,
        target_book_name,
        target_folder,
        pages: selected_committed.len(),
        kind,
        source_entries,
        edit_moves,
        edit_copies,
        bookmark_migration_journal_id,
    }))
}

fn compose_book_path_mapping_phases(phases: &[&[BookPathMapping]]) -> Vec<BookPathMapping> {
    let mut active: Vec<BookPathMapping> = Vec::new();
    for phase in phases {
        if phase.is_empty() {
            continue;
        }
        let mut by_from = phase
            .iter()
            .map(|mapping| {
                (
                    crate::search_index_db::normalize_path(&mapping.from),
                    mapping.to.clone(),
                )
            })
            .collect::<HashMap<_, _>>();

        for mapping in &mut active {
            let current_key = crate::search_index_db::normalize_path(&mapping.to);
            if let Some(next) = by_from.remove(&current_key) {
                mapping.to = next;
            }
        }

        for mapping in *phase {
            let key = crate::search_index_db::normalize_path(&mapping.from);
            if by_from.remove(&key).is_some() {
                active.push(mapping.clone());
            }
        }
    }

    active
        .into_iter()
        .filter(|mapping| !crate::folder_tree::path_eq(&mapping.from, &mapping.to))
        .collect()
}

fn plan_reorder_paths(
    folder: &Path,
    ordered_paths: &[PathBuf],
) -> Result<Vec<BookPathMapping>, String> {
    if ordered_paths.len() > MAX_BOOK_PAGES {
        return Err(format!("本のページ数が上限 {} を超えます", MAX_BOOK_PAGES));
    }
    if !folder.is_dir() {
        return Err(format!("本フォルダではありません: {}", folder.display()));
    }
    for path in ordered_paths {
        if path
            .parent()
            .is_none_or(|parent| !crate::folder_tree::path_eq(parent, &folder))
        {
            return Err(format!(
                "本フォルダ外のページは並べ替えできません: {}",
                path.display()
            ));
        }
    }

    Ok(ordered_paths
        .iter()
        .enumerate()
        .map(|(idx, from)| BookPathMapping {
            from: from.clone(),
            to: folder.join(final_reorder_name(idx + 1, from)),
        })
        .collect())
}

fn plan_reorder_filesystem_steps(
    folder: &Path,
    mappings: &[BookPathMapping],
    job_id: &str,
    label: &str,
) -> Result<Vec<BookFsStep>, String> {
    let mut temp_paths = Vec::with_capacity(mappings.len());
    for idx in 0..mappings.len() {
        let temp = folder.join(format!(".miv-book-op-{job_id}-{label}-{idx:04}.tmp"));
        if !filesystem_path_is_missing(&temp)? {
            return Err(format!(
                "並べ替え用の一時ファイルが既に存在します: {}",
                temp.display()
            ));
        }
        temp_paths.push(temp);
    }
    let mut steps = Vec::with_capacity(mappings.len() * 2);
    for (mapping, temp) in mappings.iter().zip(&temp_paths) {
        steps.push(BookFsStep::Rename {
            from: mapping.from.clone(),
            to: temp.clone(),
        });
    }
    for (mapping, temp) in mappings.iter().zip(&temp_paths) {
        steps.push(BookFsStep::Rename {
            from: temp.clone(),
            to: mapping.to.clone(),
        });
    }
    Ok(steps)
}

fn journal_transfer_staging_path(
    anchor: &Path,
    job_id: &str,
    index: usize,
    direction: &str,
) -> Result<PathBuf, String> {
    let parent = anchor
        .parent()
        .ok_or_else(|| format!("転送先に親フォルダがありません: {}", anchor.display()))?;
    Ok(parent.join(format!(
        ".miv-book-op-{job_id}-transfer-{index:04}-{direction}.tmp"
    )))
}

fn write_source(source: BookPageSource, dest: &Path) -> Result<bool, String> {
    match source {
        BookPageSource::File { src, .. } => {
            copy_file_snapshot(&src, dest)?;
            Ok(false)
        }
        BookPageSource::ZipEntry {
            zip_path,
            entry_name,
            ..
        } => {
            let bytes = crate::zip_loader::read_entry_bytes(&zip_path, &entry_name)
                .map_err(|e| format!("ZIP 内画像を読み取れません: {}: {e}", entry_name))?;
            write_bytes_create_new(dest, &bytes)?;
            Ok(false)
        }
        BookPageSource::Composited { source, edits, .. } => {
            let image = decode_composite_source(&source)?;
            let result = compose_book_page(image, &edits)?;
            write_composited_color_image(dest, &result.image, edits.format, edits.jpeg_matte)?;
            Ok(result.used_diffusion_fallback)
        }
        BookPageSource::Rendered {
            work,
            format,
            jpeg_matte,
        } => {
            let (_basename, width, height, rgba) = crate::capture::run_pixel_work(work)?;
            crate::capture::save_rgba_exact_with_matte(
                dest, format, jpeg_matte, width, height, &rgba,
            )?;
            Ok(false)
        }
        BookPageSource::VideoFrame {
            path,
            target_secs,
            format,
            jpeg_matte,
            ..
        } => {
            let frame = crate::video::screenshot::capture_frame(&path, target_secs)
                .map_err(|e| format!("動画フレーム取得に失敗しました: {e}"))?;
            crate::capture::save_rgba_exact_with_matte(
                dest,
                format,
                jpeg_matte,
                frame.width,
                frame.height,
                &frame.rgba,
            )?;
            Ok(false)
        }
        BookPageSource::ClipboardImage {
            format, jpeg_matte, ..
        } => {
            let (width, height, rgba) = read_clipboard_rgba_image()?;
            crate::capture::save_rgba_exact_with_matte(
                dest, format, jpeg_matte, width, height, &rgba,
            )?;
            Ok(false)
        }
    }
}

enum BookSourceCopy<'a> {
    Full(&'a Path),
    Semantic(&'a Path),
}

impl BookSourceCopy<'_> {
    fn path(&self) -> &Path {
        match self {
            Self::Full(path) | Self::Semantic(path) => path,
        }
    }
}

fn source_edit_copy_path<'a>(
    root: &Path,
    dest_folder: &Path,
    source: &'a BookPageSource,
) -> Option<BookSourceCopy<'a>> {
    let copy = match source {
        BookPageSource::File { src, .. } => BookSourceCopy::Full(src),
        BookPageSource::Composited {
            source: CompositeSource::File { path },
            ..
        } => BookSourceCopy::Semantic(path),
        _ => return None,
    };
    let source_book = containing_book_folder(root, copy.path())?;
    (!crate::folder_tree::path_eq(&source_book, dest_folder)).then_some(copy)
}

fn decode_file_color_image(path: &Path) -> Result<egui::ColorImage, String> {
    let image = image::open(path)
        .or_else(|_| {
            crate::wic_decoder::decode_to_dynamic_image(path).ok_or_else(|| {
                image::ImageError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "wic decode failed",
                ))
            })
        })
        .or_else(|_| {
            crate::susie_loader::decode_file(path, true, None).map_err(image::ImageError::IoError)
        })
        .map_err(|e| format!("画像をデコードできません: {}: {e}", path.display()))?;
    let image = crate::thumb_loader::apply_exif_orientation(image, path);
    Ok(dynamic_image_to_color_image(&image))
}

fn decode_bytes_color_image(hint: &str, bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let image = image::load_from_memory(bytes)
        .or_else(|_| {
            crate::wic_decoder::decode_to_dynamic_image_from_bytes(bytes).ok_or_else(|| {
                image::ImageError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "wic decode failed",
                ))
            })
        })
        .or_else(|_| {
            crate::susie_loader::decode_bytes(hint, bytes, true, None)
                .map_err(image::ImageError::IoError)
        })
        .map_err(|e| format!("ZIP 内画像をデコードできません: {hint}: {e}"))?;
    let image = crate::thumb_loader::apply_exif_orientation_from_bytes(image, bytes);
    Ok(dynamic_image_to_color_image(&image))
}

fn decode_composite_source(source: &CompositeSource) -> Result<egui::ColorImage, String> {
    match source {
        CompositeSource::File { path } => decode_file_color_image(path),
        CompositeSource::ZipEntry {
            zip_path,
            entry_name,
        } => {
            let bytes = crate::zip_loader::read_entry_bytes(zip_path, entry_name)
                .map_err(|e| format!("ZIP 内画像を読み取れません: {entry_name}: {e}"))?;
            decode_bytes_color_image(entry_name, &bytes)
        }
        CompositeSource::PdfPage {
            pdf_path,
            page_num,
            password,
        } => {
            let result = crate::pdf_loader::render_page(
                pdf_path,
                *page_num,
                4096,
                password.as_deref(),
                None,
                crate::pdf_loader::JobPriority::Critical,
                0,
                crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
            )
            .map_err(|e| format!("PDF ページを描画できません: {}: {e}", pdf_path.display()))?;
            Ok(dynamic_image_to_color_image(&result.image))
        }
    }
}

struct BookCompositeResult {
    image: egui::ColorImage,
    used_diffusion_fallback: bool,
}

/// 表示の source 解像度 edit chain を headless に再構成する。
/// global AI upscale / denoise だけを除外し、それ以外は Ctrl+E と同じ順で適用する。
fn compose_book_page(
    mut image: egui::ColorImage,
    edits: &BakedEditSnapshot,
) -> Result<BookCompositeResult, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut used_diffusion_fallback = false;

    if let Some(erase) = &edits.erase {
        let mask = resize_book_mask(&erase.mask, image.size)?;
        let result = (erase.run)(&image, &mask.bitmap, &mask.shapes, &cancel)?;
        image = result.image;
        used_diffusion_fallback = result.used_diffusion_fallback;
    }

    if let Some(layers) = &edits.local_adjust {
        let [width, height] = image.size;
        let mut layers = layers.clone();
        for layer in &mut layers {
            layer.resize_masks_to(width, height);
        }
        let rgba = crate::capture::color_image_to_rgba(&image);
        let source = local_adjust_core::RgbaImageRef {
            width,
            height,
            pixels: &rgba,
        };
        let rendered = local_adjust_core::apply_layers_with_progress(
            source,
            &layers,
            Some(cancel.as_ref()),
            |_| {},
        )
        .map_err(|e| format!("補正レイヤーを合成できません: {e}"))?;
        image = egui::ColorImage::from_rgba_unmultiplied(
            [rendered.width, rendered.height],
            &rendered.pixels,
        );
    }

    if let Some(conceal) = &edits.conceal {
        let mut mask = resize_book_mask(&conceal.mask, image.size)?;
        crate::mask_db::rasterize_shapes_into(
            &mut mask.bitmap,
            &mask.shapes,
            image.size[0],
            image.size[1],
        );
        image = crate::conceal_compose::compose_with_preset(&image, &mask.bitmap, &conceal.preset);
    }

    image = crate::adjustment::apply_adjustments_fast(&image, &edits.params);

    if let Some(comic) = &edits.comic {
        let scaled_objects = edits.comic_source_dims.and_then(|[source_w, source_h]| {
            let s_bake =
                image.size[0].max(image.size[1]) as f32 / source_w.max(source_h).max(1) as f32;
            ((s_bake - 1.0).abs() > 1e-4).then(|| comic_core::scale_scene(&comic.objects, s_bake))
        });
        let objects = scaled_objects.as_deref().unwrap_or(&comic.objects);
        let mut stamp_cache = comic.stamp_cache.clone();
        let (stamps, _, _) = crate::comic_stamp::build_stamp_images_from_cache_snapshot(
            objects,
            &mut stamp_cache,
            cancel.as_ref(),
        );
        let layers = comic_core::bake_annotation_layers(
            objects,
            image.size[0],
            image.size[1],
            &comic.fonts,
            &stamps,
        );
        image = crate::comic_overlay::composite_annotation_layers(&image, &layers);
    }

    let crop = edits.export_crop.map(|crop| {
        let was_legacy = crop.valid_source_size().is_none();
        let resolved = crop.with_legacy_source_size(image.size);
        if was_legacy
            && let Some((db_path, page_key)) = &edits.crop_legacy_writeback
        {
            match crate::export_crop::CropDb::open_at(db_path) {
                Ok(db) => {
                    if let Err(error) = db.set(page_key, resolved) {
                        crate::logger::log(format!(
                            "books: failed to adopt legacy crop source size key={page_key} error={error}"
                        ));
                    }
                }
                Err(error) => crate::logger::log(format!(
                    "books: failed to open crop DB for legacy migration key={page_key} error={error}"
                )),
            }
        }
        let crop = resolved.scaled_to(image.size).rect;
        crop_after_rotation(crop, image.size, edits.rotation)
    });
    if !edits.rotation.is_none() {
        image = crate::capture::rotate_color_image(&image, edits.rotation);
    }
    if let Some(crop) = crop {
        image = crate::export_crop::crop_color_image(&image, crop)?;
    }

    Ok(BookCompositeResult {
        image,
        used_diffusion_fallback,
    })
}

fn crop_after_rotation(
    crop: crate::export_crop::CropRect,
    source_size: [usize; 2],
    rotation: crate::rotation_db::Rotation,
) -> crate::export_crop::CropRect {
    let [width, height] = source_size;
    let crop = crop.sanitized(width, height);
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    match rotation {
        crate::rotation_db::Rotation::None => crop,
        crate::rotation_db::Rotation::Cw90 => crate::export_crop::CropRect {
            min_x: height - crop.max_y,
            min_y: crop.min_x,
            max_x: height - crop.min_y,
            max_y: crop.max_x,
        },
        crate::rotation_db::Rotation::Cw180 => crate::export_crop::CropRect {
            min_x: width - crop.max_x,
            min_y: height - crop.max_y,
            max_x: width - crop.min_x,
            max_y: height - crop.min_y,
        },
        crate::rotation_db::Rotation::Cw270 => crate::export_crop::CropRect {
            min_x: crop.min_y,
            min_y: width - crop.max_x,
            max_x: crop.max_y,
            max_y: width - crop.min_x,
        },
    }
}

fn resize_book_mask(
    snapshot: &BookMaskSnapshot,
    target: [usize; 2],
) -> Result<BookMaskSnapshot, String> {
    let [source_w, source_h] = snapshot.size;
    let [target_w, target_h] = target;
    if source_w == 0 || source_h == 0 || snapshot.bitmap.len() != source_w.saturating_mul(source_h)
    {
        return Err("編集マスクの保存サイズが不正です".to_string());
    }
    if snapshot.size == target {
        return Ok(snapshot.clone());
    }
    let mut bitmap = vec![false; target_w.saturating_mul(target_h)];
    for y in 0..target_h {
        let source_y = y.saturating_mul(source_h) / target_h.max(1);
        for x in 0..target_w {
            let source_x = x.saturating_mul(source_w) / target_w.max(1);
            bitmap[y * target_w + x] =
                snapshot.bitmap[source_y.min(source_h - 1) * source_w + source_x.min(source_w - 1)];
        }
    }
    let sx = target_w as f32 / source_w as f32;
    let sy = target_h as f32 / source_h as f32;
    let mut shapes = snapshot.shapes.clone();
    for shape in &mut shapes {
        shape.scale_xy(sx, sy);
    }
    Ok(BookMaskSnapshot {
        bitmap,
        shapes,
        size: target,
    })
}

fn write_composited_color_image(
    dest: &Path,
    image: &egui::ColorImage,
    format: crate::capture::CaptureFormat,
    jpeg_matte: crate::capture::JpegMatte,
) -> Result<(), String> {
    let src_format = if format.jpeg_quality().is_some() {
        crate::save_with_metadata::SrcFormat::Jpeg
    } else {
        crate::save_with_metadata::SrcFormat::Png
    };
    let options = crate::save_with_metadata::SaveOptions {
        jpeg_quality: format.jpeg_quality().unwrap_or(95),
        jpeg_matte,
        // 焼き込み済みの本ページは新しい完成画像。元ページの EXIF/XMP/prompt は
        // 転記せず、byte-copy fast path だけが原本 metadata を保持する。
        include_metadata: false,
        ..Default::default()
    };
    crate::save_with_metadata::save_image_with_metadata(
        image, None, None, dest, src_format, &options,
    )
    .map_err(|e| format!("本ページを保存できません: {}: {e}", dest.display()))
}

fn dynamic_image_to_color_image(img: &image::DynamicImage) -> egui::ColorImage {
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

#[cfg(windows)]
fn read_clipboard_rgba_image() -> Result<(u32, u32, Vec<u8>), String> {
    use windows::Win32::Foundation::{HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5};

    unsafe {
        if OpenClipboard(Some(HWND::default())).is_err() {
            return Err("クリップボードを開けません".to_string());
        }

        let result = (|| {
            let hmem = GetClipboardData(CF_DIB.0 as u32)
                .or_else(|_| GetClipboardData(CF_DIBV5.0 as u32))
                .map_err(|_| "クリップボードに画像がありません".to_string())?;
            if hmem.is_invalid() {
                return Err("クリップボードに画像がありません".to_string());
            }
            let global = HGLOBAL(hmem.0);
            let size = GlobalSize(global);
            if size == 0 {
                return Err("クリップボード画像が空です".to_string());
            }
            let ptr = GlobalLock(global) as *const u8;
            if ptr.is_null() {
                return Err("クリップボード画像を読み取れません".to_string());
            }
            let bytes = std::slice::from_raw_parts(ptr, size);
            let decoded = decode_cf_dib_rgba(bytes);
            let _ = GlobalUnlock(global);
            decoded
        })();

        let _ = CloseClipboard();
        result
    }
}

#[cfg(not(windows))]
fn read_clipboard_rgba_image() -> Result<(u32, u32, Vec<u8>), String> {
    Err("この環境ではクリップボード画像を読み取れません".to_string())
}

fn decode_cf_dib_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;

    let header_size = read_u32_le(bytes, 0)? as usize;
    if header_size < 40 || bytes.len() < header_size {
        return Err("クリップボード画像のヘッダーが不正です".to_string());
    }
    let width_i = read_i32_le(bytes, 4)?;
    let height_i = read_i32_le(bytes, 8)?;
    if width_i <= 0 || height_i == 0 || height_i == i32::MIN {
        return Err("クリップボード画像のサイズが不正です".to_string());
    }
    let width = width_i as u32;
    let top_down = height_i < 0;
    let height = if top_down {
        (-height_i) as u32
    } else {
        height_i as u32
    };
    let planes = read_u16_le(bytes, 12)?;
    let bit_count = read_u16_le(bytes, 14)?;
    let compression = read_u32_le(bytes, 16)?;
    if planes != 1 {
        return Err("クリップボード画像の形式が不正です".to_string());
    }
    if !matches!(bit_count, 16 | 24 | 32) {
        return Err("対応していないクリップボード画像形式です".to_string());
    }
    if compression != BI_RGB && compression != BI_BITFIELDS {
        return Err("対応していないクリップボード画像形式です".to_string());
    }
    if compression == BI_RGB && bit_count == 16 {
        return Err("対応していないクリップボード画像形式です".to_string());
    }

    let color_table_bytes = color_table_bytes(bytes, bit_count)?;
    let (pixel_offset, masks) = if compression == BI_BITFIELDS {
        let masks = dib_bitfield_masks(bytes, header_size)?;
        let offset = if header_size == 40 {
            40usize
                .checked_add(12)
                .and_then(|v| v.checked_add(color_table_bytes))
                .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?
        } else {
            header_size
                .checked_add(color_table_bytes)
                .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?
        };
        (offset, Some(masks))
    } else {
        (
            header_size
                .checked_add(color_table_bytes)
                .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?,
            None,
        )
    };

    let stride = dib_stride(width, bit_count)?;
    let needed = (height as usize)
        .checked_sub(1)
        .and_then(|last| last.checked_mul(stride))
        .and_then(|v| v.checked_add((width as usize * bit_count as usize).div_ceil(8)))
        .and_then(|v| v.checked_add(pixel_offset))
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?;
    if bytes.len() < needed {
        return Err("クリップボード画像のデータが不足しています".to_string());
    }

    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?;
    let mut rgba = vec![
        0u8;
        (pixel_count as usize)
            .checked_mul(4)
            .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?
    ];
    let mut any_alpha = bit_count != 32;
    for y in 0..height {
        let src_y = if top_down { y } else { height - 1 - y };
        let row = pixel_offset + src_y as usize * stride;
        for x in 0..width {
            let dst = (y as usize * width as usize + x as usize) * 4;
            match (bit_count, masks) {
                (24, _) => {
                    let src = row + x as usize * 3;
                    rgba[dst] = bytes[src + 2];
                    rgba[dst + 1] = bytes[src + 1];
                    rgba[dst + 2] = bytes[src];
                    rgba[dst + 3] = 255;
                }
                (32, Some((red, green, blue, alpha))) => {
                    let src = row + x as usize * 4;
                    let value = u32::from_le_bytes([
                        bytes[src],
                        bytes[src + 1],
                        bytes[src + 2],
                        bytes[src + 3],
                    ]);
                    rgba[dst] = mask_channel_to_u8(value, red);
                    rgba[dst + 1] = mask_channel_to_u8(value, green);
                    rgba[dst + 2] = mask_channel_to_u8(value, blue);
                    rgba[dst + 3] = if alpha == 0 {
                        255
                    } else {
                        let a = mask_channel_to_u8(value, alpha);
                        any_alpha |= a != 0;
                        a
                    };
                }
                (32, None) => {
                    let src = row + x as usize * 4;
                    rgba[dst] = bytes[src + 2];
                    rgba[dst + 1] = bytes[src + 1];
                    rgba[dst + 2] = bytes[src];
                    rgba[dst + 3] = bytes[src + 3];
                    any_alpha |= bytes[src + 3] != 0;
                }
                (16, Some((red, green, blue, alpha))) => {
                    let src = row + x as usize * 2;
                    let value = u16::from_le_bytes([bytes[src], bytes[src + 1]]) as u32;
                    rgba[dst] = mask_channel_to_u8(value, red);
                    rgba[dst + 1] = mask_channel_to_u8(value, green);
                    rgba[dst + 2] = mask_channel_to_u8(value, blue);
                    rgba[dst + 3] = if alpha == 0 {
                        255
                    } else {
                        mask_channel_to_u8(value, alpha)
                    };
                }
                _ => return Err("対応していないクリップボード画像形式です".to_string()),
            }
        }
    }

    if !any_alpha {
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    Ok((width, height, rgba))
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "クリップボード画像のヘッダーが不正です".to_string())?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "クリップボード画像のヘッダーが不正です".to_string())?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_i32_le(bytes: &[u8], offset: usize) -> Result<i32, String> {
    Ok(read_u32_le(bytes, offset)? as i32)
}

fn color_table_bytes(bytes: &[u8], bit_count: u16) -> Result<usize, String> {
    let clr_used = read_u32_le(bytes, 32)? as usize;
    if clr_used == 0 || bit_count > 8 {
        return Ok(0);
    }
    clr_used
        .checked_mul(4)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())
}

fn dib_bitfield_masks(bytes: &[u8], header_size: usize) -> Result<(u32, u32, u32, u32), String> {
    if header_size >= 56 {
        Ok((
            read_u32_le(bytes, 40)?,
            read_u32_le(bytes, 44)?,
            read_u32_le(bytes, 48)?,
            read_u32_le(bytes, 52)?,
        ))
    } else if header_size == 40 {
        Ok((
            read_u32_le(bytes, 40)?,
            read_u32_le(bytes, 44)?,
            read_u32_le(bytes, 48)?,
            0,
        ))
    } else {
        Err("クリップボード画像のマスク情報が不正です".to_string())
    }
}

fn dib_stride(width: u32, bit_count: u16) -> Result<usize, String> {
    let bits = (width as usize)
        .checked_mul(bit_count as usize)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?;
    bits.checked_add(31)
        .map(|v| (v / 32) * 4)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())
}

fn mask_channel_to_u8(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let raw = ((value & mask) >> shift) as u64;
    let max = if bits >= 32 {
        u32::MAX as u64
    } else {
        (1u64 << bits) - 1
    };
    ((raw * 255 + max / 2) / max) as u8
}

fn copy_file_snapshot(src: &Path, dest: &Path) -> Result<(), String> {
    let Some(parent) = dest.parent() else {
        return Err(format!("保存先が不正です: {}", dest.display()));
    };
    fs::create_dir_all(parent)
        .map_err(|e| format!("本フォルダを作成できません: {}: {e}", parent.display()))?;
    let mut input =
        fs::File::open(src).map_err(|e| format!("画像を開けません: {}: {e}", src.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| format!("ページを作成できません: {}: {e}", dest.display()))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|e| format!("ページを書き込めません: {}: {e}", dest.display()))?;
    output
        .flush()
        .map_err(|e| format!("ページを flush できません: {}: {e}", dest.display()))
}

fn write_bytes_create_new(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let Some(parent) = dest.parent() else {
        return Err(format!("保存先が不正です: {}", dest.display()));
    };
    fs::create_dir_all(parent)
        .map_err(|e| format!("本フォルダを作成できません: {}: {e}", parent.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| format!("ページを作成できません: {}: {e}", dest.display()))?;
    output
        .write_all(bytes)
        .map_err(|e| format!("ページを書き込めません: {}: {e}", dest.display()))?;
    output
        .flush()
        .map_err(|e| format!("ページを flush できません: {}: {e}", dest.display()))
}

fn destination_for_source(
    folder: &Path,
    page_no: usize,
    source: &BookPageSource,
) -> Result<PathBuf, String> {
    let raw_name = match source {
        BookPageSource::File { original_name, .. } => sanitize_filename(original_name, "page"),
        BookPageSource::ZipEntry { original_name, .. } => sanitize_filename(original_name, "page"),
        BookPageSource::Composited {
            basename, edits, ..
        } => format!(
            "{}.{}",
            crate::capture::basename_from_text(basename),
            edits.format.extension()
        ),
        BookPageSource::Rendered { work, format, .. } => {
            let basename = match work {
                crate::capture::CapturePixelWork::Single(job) => job.basename.as_str(),
                crate::capture::CapturePixelWork::Spread { basename, .. } => basename.as_str(),
            };
            format!(
                "{}.{}",
                crate::capture::basename_from_text(basename),
                format.extension()
            )
        }
        BookPageSource::VideoFrame {
            basename, format, ..
        } => format!(
            "{}.{}",
            crate::capture::basename_from_text(basename),
            format.extension()
        ),
        BookPageSource::ClipboardImage {
            basename, format, ..
        } => format!(
            "{}.{}",
            crate::capture::basename_from_text(basename),
            format.extension()
        ),
    };
    let name = sanitize_filename(&raw_name, "page");
    let path = folder.join(format!("{page_no:04}_{name}"));
    if path.exists() {
        return Err(format!("ページ番号が既に存在します: {}", path.display()));
    }
    Ok(path)
}

fn destination_for_existing_page(
    folder: &Path,
    page_no: usize,
    source_path: &Path,
) -> Result<PathBuf, String> {
    let path = folder.join(final_reorder_name(page_no, source_path));
    if path.exists() {
        return Err(format!("ページ番号が既に存在します: {}", path.display()));
    }
    Ok(path)
}

fn next_page_number(folder: &Path) -> Result<usize, String> {
    let mut max_page = 0usize;
    if !folder.exists() {
        return Ok(1);
    }
    for entry in fs::read_dir(folder)
        .map_err(|e| format!("本フォルダを読み取れません: {}: {e}", folder.display()))?
    {
        let entry = entry.map_err(|e| format!("本フォルダの項目を読み取れません: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(num) = page_number_from_name(&name) {
            max_page = max_page.max(num);
        }
    }
    if max_page >= MAX_BOOK_PAGES {
        return Err(format!(
            "本のページ数が上限 {} に達しています",
            MAX_BOOK_PAGES
        ));
    }
    Ok(max_page + 1)
}

fn book_page_count(folder: &Path) -> Result<usize, String> {
    Ok(book_page_paths(folder)?.len())
}

fn book_page_paths(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(folder)
        .map_err(|e| format!("本フォルダを読み取れません: {}: {e}", folder.display()))?
    {
        let entry = entry.map_err(|e| format!("本フォルダの項目を読み取れません: {e}"))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if page_number_from_name(&name).is_some() && is_supported_book_image_path(&path) {
            paths.push(path);
        }
    }
    paths.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(page_number_from_name)
            .unwrap_or(MAX_BOOK_PAGES + 1)
    });
    Ok(paths)
}

fn page_number_from_name(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() < 6 || bytes[4] != b'_' {
        return None;
    }
    let digits = &bytes[0..4];
    if !digits.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value = std::str::from_utf8(digits).ok()?.parse::<usize>().ok()?;
    (1..=MAX_BOOK_PAGES).contains(&value).then_some(value)
}

fn is_supported_book_image_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "webp"
            | "bmp"
            | "gif"
            | "heic"
            | "heif"
            | "avif"
            | "jxl"
            | "tif"
            | "tiff"
    )
}

fn ensure_direct_book_target(root: &Path, path: &Path) -> Result<(), String> {
    if !is_direct_book_folder(root, path) {
        return Err(format!("本棚直下の本ではありません: {}", path.display()));
    }
    Ok(())
}

fn filesystem_path_is_missing(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!(
            "ファイル状態を確認できません: {}: {error}",
            path.display()
        )),
    }
}

fn final_reorder_name(page_no: usize, original: &Path) -> String {
    let original_name = original
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("page");
    let suffix = if page_number_from_name(original_name).is_some() && original_name.len() > 5 {
        &original_name[5..]
    } else {
        original_name
    };
    format!("{page_no:04}_{}", sanitize_filename(suffix, "page"))
}

fn sanitize_filename(input: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\u{1F}' => {
                out.push('_');
            }
            _ => out.push(ch),
        }
    }
    let trimmed = out.trim_matches([' ', '.']).to_string();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_books_root_is_the_capture_output_books_directory() {
        assert_eq!(
            default_books_root(),
            crate::capture::default_output_dir().join("books")
        );
    }

    fn empty_baked_edits() -> BakedEditSnapshot {
        BakedEditSnapshot {
            params: crate::adjustment::AdjustParams::default(),
            rotation: crate::rotation_db::Rotation::None,
            conceal: None,
            erase: None,
            local_adjust: None,
            comic: None,
            comic_source_dims: None,
            export_crop: None,
            crop_legacy_writeback: None,
            format: crate::capture::CaptureFormat::Png,
            jpeg_matte: crate::capture::JpegMatte::Black,
        }
    }

    #[test]
    fn append_updates_cached_book_count_without_dropping_other_rows() {
        let mut rows = vec![
            BookInfo {
                name: "A".to_string(),
                path: PathBuf::from("books/A"),
                page_count: 3,
            },
            BookInfo {
                name: "B".to_string(),
                path: PathBuf::from("books/B"),
                page_count: 7,
            },
        ];

        apply_append_to_cached_list(&mut rows, "B", Path::new("books/B"), 2);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].page_count, 3);
        assert_eq!(rows[1].page_count, 9);
    }

    #[test]
    fn append_inserts_missing_cached_book_in_list_order() {
        let mut rows = vec![BookInfo {
            name: "B".to_string(),
            path: PathBuf::from("books/B"),
            page_count: 1,
        }];

        apply_append_to_cached_list(&mut rows, "A", Path::new("books/A"), 2);

        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(rows[0].page_count, 2);
    }

    #[test]
    fn full_composite_trigger_excludes_display_only_enhancements() {
        let identity = crate::adjustment::AdjustParams::default();
        assert!(!page_requires_full_composite(
            &identity,
            crate::rotation_db::Rotation::None,
            false,
            false,
            false,
            false,
            false,
        ));

        let mut color = identity.clone();
        color.brightness = 1.0;
        assert!(page_requires_full_composite(
            &color,
            crate::rotation_db::Rotation::None,
            false,
            false,
            false,
            false,
            false,
        ));
        let mut smart_sharpen = identity.clone();
        smart_sharpen.smart_sharpen = 1;
        assert!(!page_requires_full_composite(
            &smart_sharpen,
            crate::rotation_db::Rotation::None,
            false,
            false,
            false,
            false,
            false,
        ));
        let mut post_filter = identity.clone();
        post_filter.post_filter = crate::adjustment::PostFilter::Sepia;
        assert!(!page_requires_full_composite(
            &post_filter,
            crate::rotation_db::Rotation::None,
            false,
            false,
            false,
            false,
            false,
        ));
        assert!(page_requires_full_composite(
            &identity,
            crate::rotation_db::Rotation::Cw90,
            false,
            false,
            false,
            false,
            false,
        ));
        assert!(page_requires_full_composite(
            &identity,
            crate::rotation_db::Rotation::None,
            true,
            false,
            false,
            false,
            false,
        ));
        for edit in 1..5 {
            let mut flags = [false; 5];
            flags[edit] = true;
            assert!(page_requires_full_composite(
                &identity,
                crate::rotation_db::Rotation::None,
                flags[0],
                flags[1],
                flags[2],
                flags[3],
                flags[4],
            ));
        }

        let mut ai_only = identity;
        ai_only.upscale_model = Some("auto".to_string());
        ai_only.denoise_model = Some("jpeg".to_string());
        assert!(!page_requires_full_composite(
            &ai_only,
            crate::rotation_db::Rotation::None,
            false,
            false,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn full_composite_ignores_display_only_filter_and_sharpen() {
        let base = egui::ColorImage::new(
            [4, 2],
            vec![
                egui::Color32::from_rgb(10, 20, 30),
                egui::Color32::from_rgb(40, 50, 60),
                egui::Color32::from_rgb(70, 80, 90),
                egui::Color32::from_rgb(100, 110, 120),
                egui::Color32::from_rgb(130, 140, 150),
                egui::Color32::from_rgb(160, 170, 180),
                egui::Color32::from_rgb(190, 200, 210),
                egui::Color32::from_rgb(220, 230, 240),
            ],
        );
        let mut edits = empty_baked_edits();
        edits.params.brightness = 10.0;
        edits.params.smart_sharpen = 100;
        edits.params.post_filter = crate::adjustment::PostFilter::Sepia;
        let expected = crate::adjustment::apply_adjustments_fast(&base, &edits.params);

        let result = compose_book_page(base, &edits).unwrap();

        assert_eq!(result.image.size, [4, 2]);
        assert_eq!(result.image.pixels, expected.pixels);
    }

    #[test]
    fn full_composite_applies_erase_before_conceal() {
        let base = egui::ColorImage::new(
            [3, 1],
            vec![
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::BLUE,
            ],
        );
        let mask = BookMaskSnapshot {
            bitmap: vec![false, true, false],
            shapes: Vec::new(),
            size: [3, 1],
        };
        let run: BookEraseRunner = Box::new(|base, _, _, _| {
            let mut image = base.clone();
            image.pixels[1] = egui::Color32::WHITE;
            Ok(BookEraseResult {
                image,
                used_diffusion_fallback: true,
            })
        });
        let mut edits = empty_baked_edits();
        edits.erase = Some(BookEraseSnapshot {
            mask: mask.clone(),
            run,
        });
        edits.conceal = Some(BookConcealSnapshot {
            mask,
            preset: crate::conceal::ConcealPreset {
                conceal_type: crate::conceal::ConcealType::BlackFill,
                fill_opacity_percent: 50,
                ..Default::default()
            },
        });

        let result = compose_book_page(base, &edits).unwrap();

        assert!(result.used_diffusion_fallback);
        let middle = result.image.pixels[1].to_srgba_unmultiplied();
        assert!(middle[0] >= 126 && middle[0] <= 129);
        assert_eq!(middle[0], middle[1]);
        assert_eq!(middle[1], middle[2]);
    }

    #[test]
    fn full_composite_applies_local_adjust_adjustment_rotation_and_crop() {
        let base = egui::ColorImage::new(
            [2, 1],
            vec![egui::Color32::from_rgb(10, 20, 30), egui::Color32::WHITE],
        );
        let mut edits = empty_baked_edits();
        edits.local_adjust = Some(vec![local_adjust_core::LocalAdjustmentLayer::new(
            "invert",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::Invert(local_adjust_core::InvertParams::default()),
        )]);
        edits.params.brightness = 10.0;
        edits.rotation = crate::rotation_db::Rotation::Cw90;
        edits.export_crop = Some(crate::export_crop::CropSettings::authored(
            crate::export_crop::CropRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 1.0,
                max_y: 1.0,
            },
            crate::export_crop::CropAspectMode::Free,
            [2, 1],
        ));

        let result = compose_book_page(base, &edits).unwrap();

        assert_eq!(result.image.size, [1, 1]);
        assert_ne!(result.image.pixels[0], egui::Color32::from_rgb(10, 20, 30));
    }

    #[test]
    fn full_composite_applies_comic_stamp_on_top() {
        let base = egui::ColorImage::new([8, 8], vec![egui::Color32::BLACK; 64]);
        let source = comic_core::StampSource::Emoji("book-test".to_string());
        let object = comic_core::AnnotationObject::new_stamp(
            1,
            (4.0, 4.0),
            comic_core::StampObject {
                source: source.clone(),
                half_w: 2.0,
                half_h: 2.0,
                ..Default::default()
            },
        );
        let mut stamp = comic_core::RgbaOverlay::new(2, 2);
        stamp.pixels = [255u8, 0, 0, 255].repeat(4);
        let mut stamp_cache = std::collections::HashMap::new();
        stamp_cache.insert(
            crate::comic_stamp::stamp_source_key(&source),
            Some(Arc::new(stamp)),
        );
        let mut edits = empty_baked_edits();
        edits.comic = Some(BookComicSnapshot {
            objects: vec![object],
            fonts: Arc::new(comic_core::FontSet::new()),
            stamp_cache,
        });

        let result = compose_book_page(base, &edits).unwrap();

        assert!(
            result
                .image
                .pixels
                .iter()
                .any(|pixel| pixel.r() > 0 && pixel.g() == 0 && pixel.b() == 0)
        );
    }

    #[test]
    fn full_composite_applies_multiply_marker() {
        let base = egui::ColorImage::new([8, 8], vec![egui::Color32::WHITE; 64]);
        let mut bubble = comic_core::BubbleObject::default();
        bubble.shape = comic_core::BubbleShape::RoundRect {
            half_w: 2.0,
            half_h: 2.0,
            corner_px: 0.0,
        };
        bubble.fill = Some(comic_core::Rgba::new(255, 235, 59, 255));
        bubble.fill_opacity = 1.0;
        bubble.blend = comic_core::FillBlend::Multiply;
        bubble.outline.width_px = 0.0;
        bubble.text = comic_core::TextBlock::default();
        bubble.auto_size = false;
        let object = comic_core::AnnotationObject::new_bubble(1, (4.0, 4.0), bubble);
        let mut edits = empty_baked_edits();
        edits.comic = Some(BookComicSnapshot {
            objects: vec![object],
            fonts: Arc::new(comic_core::FontSet::new()),
            stamp_cache: std::collections::HashMap::new(),
        });

        let result = compose_book_page(base, &edits).unwrap();

        assert_eq!(
            result.image.pixels[4 * 8 + 4].to_srgba_unmultiplied(),
            [255, 235, 59, 255]
        );
    }

    #[test]
    fn full_composite_scales_comic_from_authoring_source_dims() {
        let base = egui::ColorImage::new([8, 8], vec![egui::Color32::BLACK; 64]);
        let source = comic_core::StampSource::Emoji("book-scaled-test".to_string());
        let object = comic_core::AnnotationObject::new_stamp(
            1,
            (12.0, 4.0),
            comic_core::StampObject {
                source: source.clone(),
                half_w: 2.0,
                half_h: 2.0,
                ..Default::default()
            },
        );
        let mut stamp = comic_core::RgbaOverlay::new(2, 2);
        stamp.pixels = [255u8, 0, 0, 255].repeat(4);
        let mut stamp_cache = std::collections::HashMap::new();
        stamp_cache.insert(
            crate::comic_stamp::stamp_source_key(&source),
            Some(Arc::new(stamp)),
        );
        let mut edits = empty_baked_edits();
        edits.comic = Some(BookComicSnapshot {
            objects: vec![object],
            fonts: Arc::new(comic_core::FontSet::new()),
            stamp_cache,
        });
        edits.comic_source_dims = Some([16, 16]);

        let result = compose_book_page(base, &edits).unwrap();
        let red_pixels = result
            .image
            .pixels
            .iter()
            .enumerate()
            .filter_map(|(index, pixel)| (pixel.r() > 0).then_some((index % 8, index / 8)))
            .collect::<Vec<_>>();

        assert!(!red_pixels.is_empty());
        assert!(red_pixels.iter().all(|&(x, y)| x < 8 && y < 4));
        assert!(red_pixels.iter().any(|&(x, y)| x >= 5 && y >= 1));
    }

    #[test]
    fn full_composite_scales_crop_from_authoring_source_dims() {
        let pixels = (0..4)
            .flat_map(|y| {
                (0..8).map(move |x| egui::Color32::from_rgb((x * 20) as u8, (y * 30) as u8, 0))
            })
            .collect();
        let base = egui::ColorImage::new([8, 4], pixels);
        let mut edits = empty_baked_edits();
        edits.export_crop = Some(crate::export_crop::CropSettings::authored(
            crate::export_crop::CropRect {
                min_x: 8.0,
                min_y: 0.0,
                max_x: 16.0,
                max_y: 8.0,
            },
            crate::export_crop::CropAspectMode::Free,
            [16, 8],
        ));

        let result = compose_book_page(base, &edits).unwrap();

        assert_eq!(result.image.size, [4, 4]);
        assert_eq!(result.image.pixels[0], egui::Color32::from_rgb(80, 0, 0));
        assert_eq!(result.image.pixels[3], egui::Color32::from_rgb(140, 0, 0));
    }

    #[test]
    fn full_composite_adopts_legacy_crop_basis_after_decode() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("export_crop.db");
        let key = "c:/legacy.png";
        let legacy = crate::export_crop::CropSettings {
            rect: crate::export_crop::CropRect {
                min_x: 2.0,
                min_y: 1.0,
                max_x: 6.0,
                max_y: 3.0,
            },
            aspect_mode: crate::export_crop::CropAspectMode::Free,
            source_size: None,
        };
        crate::export_crop::CropDb::open_at(&db_path)
            .unwrap()
            .set(key, legacy)
            .unwrap();
        let mut edits = empty_baked_edits();
        edits.export_crop = Some(legacy);
        edits.crop_legacy_writeback = Some((db_path.clone(), key.to_string()));
        let base = egui::ColorImage::new([8, 4], vec![egui::Color32::WHITE; 32]);

        let result = compose_book_page(base, &edits).unwrap();

        assert_eq!(result.image.size, [4, 2]);
        assert_eq!(
            crate::export_crop::CropDb::open_at(&db_path)
                .unwrap()
                .get(key)
                .unwrap()
                .source_size,
            Some([8, 4])
        );
    }

    #[test]
    fn sanitize_book_name_preserves_japanese_and_replaces_windows_invalids() {
        assert_eq!(normalize_book_name("  名前:なし?  "), "名前_なし_");
        assert_eq!(normalize_book_name("..."), DEFAULT_BOOK_NAME);
    }

    #[test]
    fn page_number_requires_four_digits_and_underscore() {
        assert_eq!(page_number_from_name("0001_a.jpg"), Some(1));
        assert_eq!(page_number_from_name("9999_a.jpg"), Some(9999));
        assert_eq!(page_number_from_name("10000_a.jpg"), None);
        assert_eq!(page_number_from_name("0000_a.jpg"), None);
        assert_eq!(page_number_from_name("001_a.jpg"), None);
    }

    #[test]
    fn reorder_name_renumbers_but_preserves_original_suffix() {
        assert_eq!(
            final_reorder_name(12, Path::new("0007_表紙?.png")),
            "0012_表紙_.png"
        );
        assert_eq!(
            final_reorder_name(1, Path::new("loose.jpg")),
            "0001_loose.jpg"
        );
    }

    #[test]
    fn decode_cf_dib_rgba_reads_bottom_up_24bpp() {
        let mut dib = vec![0u8; 40 + 8];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib[40..43].copy_from_slice(&[0, 0, 255]);
        dib[43..46].copy_from_slice(&[0, 255, 0]);

        let (width, height, rgba) = decode_cf_dib_rgba(&dib).unwrap();

        assert_eq!((width, height), (2, 1));
        assert_eq!(
            rgba,
            vec![
                255, 0, 0, 255, //
                0, 255, 0, 255,
            ]
        );
    }

    #[test]
    fn decode_cf_dib_rgba_reads_top_down_32bpp_bitfields() {
        let mut dib = vec![0u8; 56 + 4];
        dib[0..4].copy_from_slice(&56u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes());
        dib[40..44].copy_from_slice(&0x00ff_0000u32.to_le_bytes());
        dib[44..48].copy_from_slice(&0x0000_ff00u32.to_le_bytes());
        dib[48..52].copy_from_slice(&0x0000_00ffu32.to_le_bytes());
        dib[52..56].copy_from_slice(&0xff00_0000u32.to_le_bytes());
        dib[56..60].copy_from_slice(&[255, 0, 0, 128]);

        let (width, height, rgba) = decode_cf_dib_rgba(&dib).unwrap();

        assert_eq!((width, height), (1, 1));
        assert_eq!(rgba, vec![0, 0, 255, 128]);
    }

    #[test]
    fn create_book_rejects_existing_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        create_book(&root, "既存").unwrap();
        assert!(create_book(&root, "既存").is_err());
    }

    #[test]
    fn flush_reorder_rolls_back_when_final_name_conflicts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let folder = tmp.path().join("books").join("book");
        fs::create_dir_all(&folder).unwrap();
        let conflict = folder.join("0001_a.jpg");
        let page_a = folder.join("0002_a.jpg");
        let page_b = folder.join("0003_b.jpg");
        fs::write(&conflict, b"conflict").unwrap();
        fs::write(&page_a, b"a").unwrap();
        fs::write(&page_b, b"b").unwrap();

        let result = flush_reorder(folder.clone(), vec![page_a.clone(), page_b.clone()]);

        assert!(result.is_err());
        assert_eq!(fs::read(&conflict).unwrap(), b"conflict");
        assert_eq!(fs::read(&page_a).unwrap(), b"a");
        assert_eq!(fs::read(&page_b).unwrap(), b"b");
        let temp_left = fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".miv-book-tmp-")
            });
        assert!(!temp_left);
    }

    #[test]
    fn transfer_copy_commits_current_order_before_copying_selected_pages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("src");
        let target = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        let a = source.join("0001_a.jpg");
        let b = source.join("0002_b.jpg");
        let c = source.join("0003_c.jpg");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&c, b"c").unwrap();

        let result = transfer_pages_between_books(
            root.clone(),
            source.clone(),
            vec![c.clone(), a.clone(), b.clone()],
            vec![c.clone(), b.clone()],
            "dst".to_string(),
            BookTransferKind::Copy,
        )
        .unwrap();

        let BookOpResult::Transfer(summary) = result else {
            panic!("expected transfer result");
        };
        assert_eq!(fs::read(source.join("0001_c.jpg")).unwrap(), b"c");
        assert_eq!(fs::read(source.join("0002_a.jpg")).unwrap(), b"a");
        assert_eq!(fs::read(source.join("0003_b.jpg")).unwrap(), b"b");
        assert_eq!(fs::read(target.join("0001_c.jpg")).unwrap(), b"c");
        assert_eq!(fs::read(target.join("0002_b.jpg")).unwrap(), b"b");
        assert_eq!(
            summary
                .edit_copies
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (source.join("0001_c.jpg"), target.join("0001_c.jpg")),
                (source.join("0003_b.jpg"), target.join("0002_b.jpg")),
            ]
        );
        assert_eq!(
            summary
                .edit_moves
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (c, source.join("0001_c.jpg")),
                (a, source.join("0002_a.jpg")),
                (b, source.join("0003_b.jpg")),
            ]
        );
    }

    #[test]
    fn transfer_move_renames_selected_pages_and_compacts_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("src");
        let target = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        let a = source.join("0001_a.jpg");
        let b = source.join("0002_b.jpg");
        let c = source.join("0003_c.jpg");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&c, b"c").unwrap();

        let result = transfer_pages_between_books(
            root,
            source.clone(),
            vec![a.clone(), b.clone(), c.clone()],
            vec![b.clone(), c.clone()],
            "dst".to_string(),
            BookTransferKind::Move,
        )
        .unwrap();
        let BookOpResult::Transfer(summary) = result else {
            panic!("expected transfer result");
        };

        assert_eq!(fs::read(source.join("0001_a.jpg")).unwrap(), b"a");
        assert!(!source.join("0002_b.jpg").exists());
        assert!(!source.join("0003_c.jpg").exists());
        assert_eq!(fs::read(target.join("0001_b.jpg")).unwrap(), b"b");
        assert_eq!(fs::read(target.join("0002_c.jpg")).unwrap(), b"c");
        assert!(summary.edit_copies.is_empty());
        assert_eq!(
            summary
                .edit_moves
                .iter()
                .filter(|m| m.from != m.to)
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (source.join("0002_b.jpg"), target.join("0001_b.jpg")),
                (source.join("0003_c.jpg"), target.join("0002_c.jpg")),
            ]
        );
    }

    #[test]
    fn transfer_move_after_reorder_reports_net_edit_move_mappings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("src");
        let target = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        let a = source.join("0001_a.jpg");
        let b = source.join("0002_b.jpg");
        let c = source.join("0003_c.jpg");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&c, b"c").unwrap();

        let result = transfer_pages_between_books(
            root,
            source.clone(),
            vec![c.clone(), a.clone(), b.clone()],
            vec![c.clone(), a.clone()],
            "dst".to_string(),
            BookTransferKind::Move,
        )
        .unwrap();
        let BookOpResult::Transfer(summary) = result else {
            panic!("expected transfer result");
        };

        assert_eq!(fs::read(source.join("0001_b.jpg")).unwrap(), b"b");
        assert_eq!(fs::read(target.join("0001_c.jpg")).unwrap(), b"c");
        assert_eq!(fs::read(target.join("0002_a.jpg")).unwrap(), b"a");
        assert!(summary.edit_copies.is_empty());
        assert_eq!(
            summary
                .edit_moves
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (c, target.join("0001_c.jpg")),
                (a, target.join("0002_a.jpg")),
                (b, source.join("0001_b.jpg")),
            ]
        );
    }

    #[test]
    fn append_book_page_reports_edit_copy_mapping() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();
        let page = source.join("0001_a.jpg");
        fs::write(&page, b"a").unwrap();

        let result = append_pages_at(
            tmp.path().join("data"),
            root.clone(),
            "dst".to_string(),
            vec![BookPageSource::File {
                src: page.clone(),
                original_name: "0001_a.jpg".to_string(),
            }],
        )
        .unwrap();

        let BookOpResult::Append(summary) = result else {
            panic!("expected append result");
        };
        assert_eq!(
            fs::read(root.join("dst").join("0001_0001_a.jpg")).unwrap(),
            b"a"
        );
        assert_eq!(
            summary
                .edit_copies
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![(page, root.join("dst").join("0001_0001_a.jpg"))]
        );
    }

    #[test]
    fn rename_book_reports_edit_move_mappings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("old");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("0001_a.jpg"), b"a").unwrap();
        fs::write(source.join("0002_b.jpg"), b"b").unwrap();

        let result = rename_book(&root, "old", "new").unwrap();

        let BookOpResult::Renamed { edit_moves, .. } = result else {
            panic!("expected rename result");
        };
        assert_eq!(
            edit_moves
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    root.join("old").join("0001_a.jpg"),
                    root.join("new").join("0001_a.jpg")
                ),
                (
                    root.join("old").join("0002_b.jpg"),
                    root.join("new").join("0002_b.jpg")
                ),
            ]
        );
    }
}
