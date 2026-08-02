//! 明示操作による孤児メタデータ整理。
//!
//! `rename_key_migration::STORES` を正本として全 path-keyed SQLite ストアを列挙し、
//! 「実体は存在しないが、その直上の親ディレクトリには到達できる」行だけを候補にする。
//! 外付けドライブ / NAS がオフライン、権限エラー、判定不能なキーは常に残す。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc;

use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};

use crate::rename_key_migration::{STORES, StoreDescriptor, StoreKeyNormalization};

const PHASE_SCANNING: u8 = 1;
const PHASE_DELETING: u8 = 2;

pub(crate) const DELETE_PURGE_JOURNAL_FILE: &str = "delete_purge_journal.json";
static DELETE_PURGE_JOURNAL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct DeletePurgeJournalEntry {
    path: PathBuf,
    #[serde(default)]
    pdf_paths: Vec<PathBuf>,
}

#[derive(Debug, Default)]
#[allow(dead_code)] // lib target では bin-only App poller が無い
pub(crate) struct DeletePurgeRetryReport {
    pub(crate) attempted: usize,
    pub(crate) purged: usize,
    pub(crate) remaining: usize,
    pub(crate) rows: usize,
    pub(crate) errors: Vec<String>,
}

#[allow(dead_code)] // lib target では bin-only App poller が無い
pub(crate) struct DeletePurgeRetryPending {
    pub(crate) rx: mpsc::Receiver<DeletePurgeRetryReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreCount {
    pub store: String,
    pub rows: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcludedStore {
    pub store: String,
    pub rows: usize,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct CleanupCandidate {
    descriptor_index: usize,
    key: String,
    physical_path: PathBuf,
    rows: usize,
    store: String,
}

#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    pub orphan_by_store: Vec<StoreCount>,
    pub protected_by_store: Vec<StoreCount>,
    pub excluded: Vec<ExcludedStore>,
    pub errors: Vec<String>,
    pub scanned_rows: usize,
    pub candidates: Vec<CleanupCandidate>,
}

impl ScanReport {
    pub fn orphan_total(&self) -> usize {
        self.orphan_by_store.iter().map(|row| row.rows).sum()
    }

    pub fn protected_total(&self) -> usize {
        self.protected_by_store.iter().map(|row| row.rows).sum()
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeleteReport {
    pub deleted_by_store: Vec<StoreCount>,
    pub protected_after_scan: Vec<StoreCount>,
    pub errors: Vec<String>,
    pub deleted_keys: Vec<String>,
    pub canceled: bool,
}

impl DeleteReport {
    pub fn deleted_total(&self) -> usize {
        self.deleted_by_store.iter().map(|row| row.rows).sum()
    }
}

pub enum WorkerResult {
    Scan(ScanReport),
    Delete(DeleteReport),
    ScanCanceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupTask {
    Scan,
    Delete,
}

pub struct CleanupProgress {
    phase: AtomicU8,
    processed: AtomicUsize,
    total: AtomicUsize,
    store_index: AtomicUsize,
}

impl CleanupProgress {
    fn new(phase: u8) -> Self {
        Self {
            phase: AtomicU8::new(phase),
            processed: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            store_index: AtomicUsize::new(0),
        }
    }

    pub fn snapshot(&self) -> (usize, usize, usize, bool) {
        (
            self.processed.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
            self.store_index.load(Ordering::Relaxed),
            self.phase.load(Ordering::Relaxed) == PHASE_DELETING,
        )
    }
}

pub struct CleanupPending {
    pub task: CleanupTask,
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<CleanupProgress>,
    pub rx: mpsc::Receiver<WorkerResult>,
}

impl CleanupPending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Shell delete succeeded but one or more metadata stores stayed busy. Record one
/// entry per removed root so the normal orphan safety check can be applied on retry.
pub(crate) fn journal_failed_delete_purge(
    data_dir: &Path,
    removed: &[PathBuf],
    pdf_paths: &[PathBuf],
) -> bool {
    let _guard = DELETE_PURGE_JOURNAL_LOCK.lock().unwrap();
    let mut entries = match load_delete_purge_journal_unlocked(data_dir) {
        Ok(entries) => entries,
        Err(error) => {
            crate::logger::log(format!("[delete-purge] journal load failed: {error}"));
            return false;
        }
    };
    for path in removed {
        let matching_pdf_paths = pdf_paths
            .iter()
            .filter(|pdf| delete_root_contains(path, pdf))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(existing) = entries.iter_mut().find(|entry| entry.path == *path) {
            existing.pdf_paths.extend(matching_pdf_paths);
            existing.pdf_paths.sort();
            existing.pdf_paths.dedup();
        } else {
            entries.push(DeletePurgeJournalEntry {
                path: path.clone(),
                pdf_paths: matching_pdf_paths,
            });
        }
    }
    match save_delete_purge_journal_unlocked(data_dir, &entries) {
        Ok(()) => true,
        Err(error) => {
            crate::logger::log(format!("[delete-purge] journal save failed: {error}"));
            false
        }
    }
}

#[allow(dead_code)] // lib target では bin-only App poller が無い
pub(crate) fn spawn_delete_purge_retry(data_dir: PathBuf) -> DeletePurgeRetryPending {
    let (tx, rx) = mpsc::channel();
    let fallback_tx = tx.clone();
    let result = std::thread::Builder::new()
        .name("delete-purge-retry".into())
        .spawn(move || {
            let report = retry_delete_purge_journal_at(&data_dir);
            let _ = tx.send(report);
        });
    if let Err(error) = result {
        let _ = fallback_tx.send(DeletePurgeRetryReport {
            remaining: 1,
            errors: vec![format!("worker spawn failed: {error}")],
            ..Default::default()
        });
    }
    DeletePurgeRetryPending { rx }
}

#[allow(dead_code)] // lib target では bin-only App poller が無い
fn retry_delete_purge_journal_at(data_dir: &Path) -> DeletePurgeRetryReport {
    let snapshot = {
        let _guard = DELETE_PURGE_JOURNAL_LOCK.lock().unwrap();
        match load_delete_purge_journal_unlocked(data_dir) {
            Ok(entries) => entries,
            Err(error) => {
                return DeletePurgeRetryReport {
                    remaining: 1,
                    errors: vec![format!("journal load failed: {error}")],
                    ..Default::default()
                };
            }
        }
    };
    let mut report = DeletePurgeRetryReport {
        attempted: snapshot.len(),
        ..Default::default()
    };
    let mut completed = Vec::new();
    for entry in &snapshot {
        if classify_path(&entry.path) != PathClassification::Orphan {
            report.errors.push(format!(
                "{}: path is present or its parent is unreachable; deferred",
                entry.path.display()
            ));
            continue;
        }
        let purge = crate::rename_key_migration::purge_removed_paths_at(
            data_dir,
            std::slice::from_ref(&entry.path),
            &entry.pdf_paths,
        );
        report.rows += purge.rows;
        if purge.errors.is_empty() {
            report.purged += 1;
            completed.push(entry.clone());
        } else {
            report.errors.extend(purge.errors);
        }
    }

    let _guard = DELETE_PURGE_JOURNAL_LOCK.lock().unwrap();
    let mut current = match load_delete_purge_journal_unlocked(data_dir) {
        Ok(entries) => entries,
        Err(error) => {
            report
                .errors
                .push(format!("journal reload failed: {error}"));
            report.remaining = snapshot.len();
            return report;
        }
    };
    current.retain(|entry| !completed.contains(entry));
    report.remaining = current.len();
    if let Err(error) = save_delete_purge_journal_unlocked(data_dir, &current) {
        report
            .errors
            .push(format!("journal update failed: {error}"));
        report.remaining = current.len().max(1);
    }
    report
}

fn delete_root_contains(root: &Path, candidate: &Path) -> bool {
    let root = crate::adjustment_db::normalize_path(root);
    let candidate = crate::adjustment_db::normalize_path(candidate);
    candidate == root
        || candidate.starts_with(&format!("{root}/"))
        || candidate.starts_with(&format!("{root}::"))
}

fn load_delete_purge_journal_unlocked(
    data_dir: &Path,
) -> std::io::Result<Vec<DeletePurgeJournalEntry>> {
    let path = data_dir.join(DELETE_PURGE_JOURNAL_FILE);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn save_delete_purge_journal_unlocked(
    data_dir: &Path,
    entries: &[DeletePurgeJournalEntry],
) -> std::io::Result<()> {
    let path = data_dir.join(DELETE_PURGE_JOURNAL_FILE);
    if entries.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
    }
    std::fs::create_dir_all(data_dir)?;
    let bytes = serde_json::to_vec(entries)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path).or_else(|error| {
        let _ = std::fs::remove_file(&tmp);
        Err(error)
    })
}

pub fn spawn_scan(data_dir: PathBuf, books_root: PathBuf) -> CleanupPending {
    spawn_scan_impl(data_dir, books_root)
}

fn spawn_scan_impl(data_dir: PathBuf, books_root: PathBuf) -> CleanupPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(CleanupProgress::new(PHASE_SCANNING));
    let (tx, rx) = mpsc::channel();
    let worker_cancel = Arc::clone(&cancel);
    let worker_progress = Arc::clone(&progress);
    spawn_scan_thread(data_dir, books_root, worker_cancel, worker_progress, tx);
    CleanupPending {
        task: CleanupTask::Scan,
        cancel,
        progress,
        rx,
    }
}
fn spawn_scan_thread(
    data_dir: PathBuf,
    books_root: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: Arc<CleanupProgress>,
    tx: mpsc::Sender<WorkerResult>,
) {
    let fallback_tx = tx.clone();
    let result = std::thread::Builder::new()
        .name("metadata-cleanup-scan".into())
        .spawn(move || scan_worker(data_dir, books_root, cancel, progress, tx));
    finish_scan_spawn(result, fallback_tx);
}

fn finish_scan_spawn(
    result: std::io::Result<std::thread::JoinHandle<()>>,
    tx: mpsc::Sender<WorkerResult>,
) {
    finish_scan_spawn_result(result.err(), tx);
}

fn finish_scan_spawn_result(error: Option<std::io::Error>, tx: mpsc::Sender<WorkerResult>) {
    if let Some(error) = error {
        let _ = tx.send(WorkerResult::Scan(ScanReport {
            errors: vec![format!("worker spawn failed: {error}")],
            ..Default::default()
        }));
    }
}

fn scan_worker(
    data_dir: PathBuf,
    books_root: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: Arc<CleanupProgress>,
    tx: mpsc::Sender<WorkerResult>,
) {
    let started = std::time::Instant::now();
    let result = run_scan_at(&data_dir, &books_root, &cancel, &progress);
    crate::perf::emit_ms("metadata_cleanup", "scan", 0, started);
    let message = result.map_or(WorkerResult::ScanCanceled, WorkerResult::Scan);
    let _ = tx.send(message);
}

pub fn spawn_delete(data_dir: PathBuf, books_root: PathBuf, scan: ScanReport) -> CleanupPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(CleanupProgress::new(PHASE_DELETING));
    progress.total.store(scan.orphan_total(), Ordering::Relaxed);
    spawn_delete_impl(data_dir, books_root, scan, cancel, progress)
}

fn spawn_delete_impl(
    data_dir: PathBuf,
    books_root: PathBuf,
    scan: ScanReport,
    cancel: Arc<AtomicBool>,
    progress: Arc<CleanupProgress>,
) -> CleanupPending {
    let (tx, rx) = mpsc::channel();
    let worker_cancel = Arc::clone(&cancel);
    let worker_progress = Arc::clone(&progress);
    let fallback_tx = tx.clone();
    spawn_delete_thread(
        data_dir,
        books_root,
        scan,
        worker_cancel,
        worker_progress,
        tx,
        fallback_tx,
    );
    CleanupPending {
        task: CleanupTask::Delete,
        cancel,
        progress,
        rx,
    }
}

fn spawn_delete_thread(
    data_dir: PathBuf,
    books_root: PathBuf,
    scan: ScanReport,
    cancel: Arc<AtomicBool>,
    progress: Arc<CleanupProgress>,
    tx: mpsc::Sender<WorkerResult>,
    fallback_tx: mpsc::Sender<WorkerResult>,
) {
    let result = std::thread::Builder::new()
        .name("metadata-cleanup-delete".into())
        .spawn(move || delete_worker(data_dir, books_root, scan, cancel, progress, tx));
    if let Err(error) = result {
        let _ = fallback_tx.send(WorkerResult::Delete(DeleteReport {
            errors: vec![format!("worker spawn failed: {error}")],
            ..Default::default()
        }));
    }
}

fn delete_worker(
    data_dir: PathBuf,
    books_root: PathBuf,
    scan: ScanReport,
    cancel: Arc<AtomicBool>,
    progress: Arc<CleanupProgress>,
    tx: mpsc::Sender<WorkerResult>,
) {
    let started = std::time::Instant::now();
    let report = run_delete_at(&data_dir, &books_root, &scan, &cancel, &progress);
    crate::perf::emit_ms("metadata_cleanup", "delete", 0, started);
    let _ = tx.send(WorkerResult::Delete(report));
}

fn run_scan_at(
    data_dir: &Path,
    books_root: &Path,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
) -> Option<ScanReport> {
    let mut state = ScanState::default();
    for (index, descriptor) in STORES.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        progress.store_index.store(index, Ordering::Relaxed);
        scan_descriptor(
            data_dir, books_root, index, descriptor, cancel, progress, &mut state,
        );
    }
    state.finish(data_dir)
}

#[derive(Default)]
struct ScanState {
    report: ScanReport,
    orphan: BTreeMap<String, usize>,
    protected: BTreeMap<String, usize>,
    excluded: BTreeMap<(String, String), usize>,
}

impl ScanState {
    fn exclude(&mut self, store: &str, reason: &str, rows: usize) {
        *self
            .excluded
            .entry((store.into(), reason.into()))
            .or_default() += rows;
    }

    fn finish(mut self, data_dir: &Path) -> Option<ScanReport> {
        let password_rows = pdf_password_entry_count(&data_dir.join("pdf_passwords.json"));
        self.exclude(
            "PDF パスワード",
            "SHA-256 キーから元 PDF パスを逆引きできないため",
            password_rows,
        );
        self.report.orphan_by_store = counts_to_vec(self.orphan);
        self.report.protected_by_store = counts_to_vec(self.protected);
        self.report.excluded = excluded_to_vec(self.excluded);
        Some(self.report)
    }
}

fn scan_descriptor(
    data_dir: &Path,
    books_root: &Path,
    index: usize,
    descriptor: &StoreDescriptor,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
    state: &mut ScanState,
) {
    let path = data_dir.join(descriptor.file);
    if !path.exists() {
        return;
    }
    scan_db_path(
        &path, books_root, index, descriptor, cancel, progress, state,
    );
}

fn scan_db_path(
    path: &Path,
    books_root: &Path,
    index: usize,
    descriptor: &StoreDescriptor,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
    state: &mut ScanState,
) {
    let Some(connection) = open_for_scan(path, descriptor, state) else {
        return;
    };
    scan_open_table(
        &connection,
        books_root,
        index,
        descriptor,
        cancel,
        progress,
        state,
    );
}

fn open_for_scan(
    path: &Path,
    descriptor: &StoreDescriptor,
    state: &mut ScanState,
) -> Option<rusqlite::Connection> {
    match open_readonly(path) {
        Ok(connection) => Some(connection),
        Err(error) => {
            state
                .report
                .errors
                .push(format!("{}: {error}", descriptor.file));
            None
        }
    }
}

fn scan_open_table(
    connection: &rusqlite::Connection,
    books_root: &Path,
    index: usize,
    descriptor: &StoreDescriptor,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
    state: &mut ScanState,
) {
    if !table_exists(connection, descriptor.table).unwrap_or(false) {
        return;
    }
    let rows = table_row_count(connection, descriptor.table).unwrap_or(0);
    state.report.scanned_rows += rows;
    progress.total.fetch_add(rows, Ordering::Relaxed);
    scan_table_rows(
        connection, books_root, index, descriptor, cancel, progress, state, rows,
    );
}

fn scan_table_rows(
    connection: &rusqlite::Connection,
    books_root: &Path,
    index: usize,
    descriptor: &StoreDescriptor,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
    state: &mut ScanState,
    rows: usize,
) {
    if let Some(reason) = descriptor_exclusion(descriptor) {
        state.exclude(store_label(descriptor), &reason, rows);
        progress.processed.fetch_add(rows, Ordering::Relaxed);
        return;
    }
    scan_supported_rows(
        connection, books_root, index, descriptor, cancel, progress, state, rows,
    );
}

fn descriptor_exclusion(descriptor: &StoreDescriptor) -> Option<String> {
    descriptor_exclusion_inner(descriptor)
}

fn descriptor_exclusion_inner(descriptor: &StoreDescriptor) -> Option<String> {
    if descriptor.normalization == StoreKeyNormalization::DriveStripped {
        return Some("ドライブ文字を保存しないキーから実体を安全に逆引きできないため".into());
    }
    (descriptor.file == "reading_history.db")
        .then(|| "閲覧履歴は削除済みでも閲覧記録として残す仕様のため".into())
}

fn scan_supported_rows(
    connection: &rusqlite::Connection,
    books_root: &Path,
    index: usize,
    descriptor: &StoreDescriptor,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
    state: &mut ScanState,
    rows: usize,
) {
    let query = scan_query(descriptor);
    let mut statement = match connection.prepare(&query) {
        Ok(statement) => statement,
        Err(error) => {
            state
                .report
                .errors
                .push(format!("{}.{}: {error}", descriptor.file, descriptor.table));
            progress.processed.fetch_add(rows, Ordering::Relaxed);
            return;
        }
    };
    let mapped = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?.max(0) as usize,
            row.get::<_, Option<String>>(2)?,
        ))
    });
    let Ok(mapped) = mapped else {
        progress.processed.fetch_add(rows, Ordering::Relaxed);
        return;
    };
    for row in mapped {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match row {
            Ok((key, count, source)) => scan_key(
                books_root,
                index,
                descriptor,
                key,
                count,
                source.as_deref(),
                progress,
                state,
            ),
            Err(error) => state
                .report
                .errors
                .push(format!("{}.{}: {error}", descriptor.file, descriptor.table)),
        }
    }
}

fn scan_query(descriptor: &StoreDescriptor) -> String {
    if descriptor.file == "rating.db" && descriptor.table == "ratings" {
        format!(
            "SELECT {}, COUNT(*), MAX(source_path) FROM {} GROUP BY {}",
            descriptor.column, descriptor.table, descriptor.column
        )
    } else {
        format!(
            "SELECT {}, COUNT(*), NULL FROM {} GROUP BY {}",
            descriptor.column, descriptor.table, descriptor.column
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_key(
    books_root: &Path,
    index: usize,
    descriptor: &StoreDescriptor,
    key: String,
    rows: usize,
    source_path: Option<&str>,
    progress: &CleanupProgress,
    state: &mut ScanState,
) {
    progress.processed.fetch_add(rows, Ordering::Relaxed);
    let store = store_label(descriptor).to_string();
    let Some(path) = physical_path_for_key(&key, source_path) else {
        state.exclude(&store, "キーから絶対パスを安全に復元できないため", rows);
        return;
    };
    if is_under_bookshelf(&path, books_root) {
        state.exclude(&store, "製本用の本棚配下は誤削除防止のため対象外", rows);
        return;
    }
    match classify_path(&path) {
        PathClassification::Exists => {}
        PathClassification::Protected => {
            *state.protected.entry(store).or_default() += rows;
        }
        PathClassification::Orphan => {
            *state.orphan.entry(store.clone()).or_default() += rows;
            state.report.candidates.push(CleanupCandidate {
                descriptor_index: index,
                key,
                physical_path: path,
                rows,
                store,
            });
        }
    }
}

fn run_delete_at(
    data_dir: &Path,
    books_root: &Path,
    scan: &ScanReport,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
) -> DeleteReport {
    run_delete_with_hook(data_dir, books_root, scan, cancel, progress, || {})
}

fn run_delete_with_hook<F>(
    data_dir: &Path,
    books_root: &Path,
    scan: &ScanReport,
    cancel: &AtomicBool,
    progress: &CleanupProgress,
    mut after_candidate: F,
) -> DeleteReport
where
    F: FnMut(),
{
    let mut report = DeleteReport::default();
    let mut deleted = BTreeMap::<String, usize>::new();
    let mut protected = BTreeMap::<String, usize>::new();
    for index in 0..STORES.len() {
        let candidates = scan
            .candidates
            .iter()
            .filter(|candidate| candidate.descriptor_index == index)
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            report.canceled = true;
            break;
        }
        progress.store_index.store(index, Ordering::Relaxed);
        delete_descriptor(
            data_dir,
            books_root,
            &STORES[index],
            &candidates,
            cancel,
            progress,
            &mut after_candidate,
            &mut report,
            &mut deleted,
            &mut protected,
        );
        if report.canceled {
            break;
        }
    }
    report.deleted_by_store = counts_to_vec(deleted);
    report.protected_after_scan = counts_to_vec(protected);
    report
}

#[allow(clippy::too_many_arguments)]
fn delete_descriptor<F>(
    data_dir: &Path,
    books_root: &Path,
    descriptor: &StoreDescriptor,
    candidates: &[&CleanupCandidate],
    cancel: &AtomicBool,
    progress: &CleanupProgress,
    after_candidate: &mut F,
    report: &mut DeleteReport,
    deleted: &mut BTreeMap<String, usize>,
    protected: &mut BTreeMap<String, usize>,
) where
    F: FnMut(),
{
    let path = data_dir.join(descriptor.file);
    let mut connection = match open_for_write(&path) {
        Ok(connection) => connection,
        Err(error) => {
            report.errors.push(format!("{}: {error}", descriptor.file));
            return;
        }
    };
    let transaction =
        match connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
            Ok(transaction) => transaction,
            Err(error) => {
                report
                    .errors
                    .push(format!("{}.{}: {error}", descriptor.file, descriptor.table));
                return;
            }
        };
    let sql = format!(
        "DELETE FROM {} WHERE {} = ?1",
        descriptor.table, descriptor.column
    );
    let mut committed = Vec::<(String, usize, String)>::new();
    let mut protected_rows = 0usize;
    let mut failed = false;
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            report.canceled = true;
            break;
        }
        if is_under_bookshelf(&candidate.physical_path, books_root)
            || classify_path(&candidate.physical_path) != PathClassification::Orphan
        {
            protected_rows += candidate.rows;
        } else {
            match transaction.execute(&sql, [&candidate.key]) {
                Ok(rows) if rows > 0 => {
                    committed.push((candidate.key.clone(), rows, candidate.store.clone()))
                }
                Ok(_) => {}
                Err(error) => {
                    report
                        .errors
                        .push(format!("{}.{}: {error}", descriptor.file, descriptor.table));
                    failed = true;
                    break;
                }
            }
        }
        progress
            .processed
            .fetch_add(candidate.rows, Ordering::Relaxed);
        after_candidate();
    }
    if report.canceled || failed {
        drop(transaction);
        return;
    }
    if let Err(error) = transaction.commit() {
        report.errors.push(format!(
            "{}.{} commit: {error}",
            descriptor.file, descriptor.table
        ));
        return;
    }
    for (key, rows, store) in committed {
        *deleted.entry(store).or_default() += rows;
        report.deleted_keys.push(key);
    }
    if protected_rows > 0 {
        *protected.entry(store_label(descriptor).into()).or_default() += protected_rows;
    }
}

fn open_readonly(path: &Path) -> Result<rusqlite::Connection, rusqlite::Error> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(connection)
}

fn open_for_write(path: &Path) -> Result<rusqlite::Connection, rusqlite::Error> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(connection)
}

fn table_exists(connection: &rusqlite::Connection, table: &str) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
}

fn table_row_count(
    connection: &rusqlite::Connection,
    table: &str,
) -> Result<usize, rusqlite::Error> {
    connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        Ok(row.get::<_, i64>(0)?.max(0) as usize)
    })
}

fn pdf_password_entry_count(path: &Path) -> usize {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<BTreeMap<String, serde_json::Value>>(&bytes).ok()
        })
        .map(|entries| entries.len())
        .unwrap_or(0)
}

fn counts_to_vec(counts: BTreeMap<String, usize>) -> Vec<StoreCount> {
    counts
        .into_iter()
        .map(|(store, rows)| StoreCount { store, rows })
        .collect()
}

fn excluded_to_vec(counts: BTreeMap<(String, String), usize>) -> Vec<ExcludedStore> {
    counts
        .into_iter()
        .map(|((store, reason), rows)| ExcludedStore {
            store,
            rows,
            reason,
        })
        .collect()
}

fn store_label(descriptor: &StoreDescriptor) -> &'static str {
    match descriptor.file {
        "rating.db" => "レーティング",
        "adjustment.db" => "補正",
        "mask.db" => "消しゴム",
        "conceal.db" => "隠蔽加工",
        "local_adjust.db" => "部分補正",
        "comic.db" => "テキスト注釈",
        "export_crop.db" => "書き出し範囲",
        "tags.db" => "タグ",
        "rotation.db" => "回転",
        "view_trim.db" => "表示トリム",
        "video_pins.db" => "動画ピン",
        "video_bookmarks.db" => "動画ブックマーク",
        "folder_thumb_pins.db" => "代表サムネピン",
        "book_resume.db" => "読書位置",
        "spread.db" => "ページ表示モード",
        "reading_history.db" => "閲覧履歴",
        _ => descriptor.file,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathClassification {
    Exists,
    Orphan,
    Protected,
}

fn classify_path(path: &Path) -> PathClassification {
    if !path.is_absolute() {
        return PathClassification::Protected;
    }
    match path.try_exists() {
        Ok(true) => PathClassification::Exists,
        Ok(false) if path.parent().is_some_and(Path::is_dir) => PathClassification::Orphan,
        Ok(false) | Err(_) => PathClassification::Protected,
    }
}

fn physical_path_for_key(key: &str, source_path: Option<&str>) -> Option<PathBuf> {
    let raw = source_path
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| key.split_once("::").map(|(root, _)| root).unwrap_or(key));
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return None;
    }
    if matches!(path.try_exists(), Ok(true)) {
        return Some(path);
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        let Some(extension) = prefix.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        if crate::archive_converter::ArchiveFormat::nested_from_extension(extension).is_none() {
            continue;
        }
        if !prefix.is_dir() {
            return Some(prefix);
        }
    }
    Some(path)
}

fn is_under_bookshelf(path: &Path, books_root: &Path) -> bool {
    !books_root.as_os_str().is_empty() && crate::books::path_is_under_or_equal(path, books_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(phase: u8) -> CleanupProgress {
        CleanupProgress::new(phase)
    }

    fn descriptor_index(file: &str, table: &str) -> usize {
        STORES
            .iter()
            .position(|descriptor| descriptor.file == file && descriptor.table == table)
            .unwrap()
    }

    fn create_table(data_dir: &Path, descriptor: &StoreDescriptor) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(data_dir.join(descriptor.file)).unwrap();
        connection
            .execute(
                &format!(
                    "CREATE TABLE IF NOT EXISTS {} ({} TEXT)",
                    descriptor.table, descriptor.column
                ),
                [],
            )
            .unwrap();
        if descriptor.file == "rating.db" {
            let _ = connection.execute("ALTER TABLE ratings ADD COLUMN source_path TEXT", []);
        }
        connection
    }

    fn insert_key(data_dir: &Path, index: usize, key: &str) {
        let descriptor = &STORES[index];
        let connection = create_table(data_dir, descriptor);
        connection
            .execute(
                &format!(
                    "INSERT INTO {} ({}) VALUES (?1)",
                    descriptor.table, descriptor.column
                ),
                [key],
            )
            .unwrap();
    }

    #[test]
    fn only_missing_child_of_reachable_parent_is_orphan() {
        let temp = tempfile::tempdir().unwrap();
        let existing = temp.path().join("existing.jpg");
        std::fs::write(&existing, b"x").unwrap();
        let missing = temp.path().join("missing.jpg");
        let offline = temp.path().join("offline").join("missing.jpg");

        assert_eq!(classify_path(&existing), PathClassification::Exists);
        assert_eq!(classify_path(&missing), PathClassification::Orphan);
        assert_eq!(classify_path(&offline), PathClassification::Protected);
    }

    #[test]
    fn scan_counts_orphans_and_protects_offline_and_existing_rows() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let files = temp.path().join("files");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&files).unwrap();
        let existing = files.join("existing.jpg");
        std::fs::write(&existing, b"x").unwrap();
        let missing = files.join("missing.jpg");
        let offline = files.join("offline").join("missing.jpg");
        let rating = descriptor_index("rating.db", "ratings");
        for path in [&existing, &missing, &offline] {
            insert_key(
                &data_dir,
                rating,
                &crate::adjustment_db::normalize_path(path),
            );
        }

        let report = run_scan_at(
            &data_dir,
            Path::new(""),
            &AtomicBool::new(false),
            &progress(PHASE_SCANNING),
        )
        .unwrap();
        assert_eq!(report.orphan_total(), 1);
        assert_eq!(report.protected_total(), 1);
        assert_eq!(report.scanned_rows, 3);
    }

    #[test]
    fn folder_key_and_hash_key_are_handled_safely() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let parent = temp.path().join("folders");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let missing_folder = parent.join("gone");
        let sidecar = descriptor_index("adjustment.db", "sidecar_sync");
        insert_key(
            &data_dir,
            sidecar,
            &crate::adjustment_db::normalize_path(&missing_folder),
        );
        std::fs::write(
            data_dir.join("pdf_passwords.json"),
            br#"{"hash1":"secret","hash2":"secret"}"#,
        )
        .unwrap();

        let report = run_scan_at(
            &data_dir,
            Path::new(""),
            &AtomicBool::new(false),
            &progress(PHASE_SCANNING),
        )
        .unwrap();
        assert_eq!(report.orphan_total(), 1);
        assert!(report.excluded.iter().any(|entry| {
            entry.store == "PDF パスワード" && entry.rows == 2 && entry.reason.contains("SHA-256")
        }));
    }

    #[test]
    fn bookshelf_and_drive_stripped_rows_are_excluded() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let books = temp.path().join("books");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&books).unwrap();
        let missing_page = books.join("book-a").join("gone.jpg");
        let rating = descriptor_index("rating.db", "ratings");
        insert_key(
            &data_dir,
            rating,
            &crate::adjustment_db::normalize_path(&missing_page),
        );
        let spread = descriptor_index("spread.db", "spreads");
        insert_key(&data_dir, spread, "/some/book");

        let report = run_scan_at(
            &data_dir,
            &books,
            &AtomicBool::new(false),
            &progress(PHASE_SCANNING),
        )
        .unwrap();
        assert_eq!(report.orphan_total(), 0);
        assert!(
            report
                .excluded
                .iter()
                .any(|entry| entry.reason.contains("本棚配下"))
        );
        assert!(
            report
                .excluded
                .iter()
                .any(|entry| entry.store == "ページ表示モード" && entry.rows == 1)
        );
    }

    #[test]
    fn delete_removes_confirmed_orphan() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let files = temp.path().join("files");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&files).unwrap();
        let rating = descriptor_index("rating.db", "ratings");
        insert_key(
            &data_dir,
            rating,
            &crate::adjustment_db::normalize_path(&files.join("missing.jpg")),
        );
        let scan = run_scan_at(
            &data_dir,
            Path::new(""),
            &AtomicBool::new(false),
            &progress(PHASE_SCANNING),
        )
        .unwrap();
        let report = run_delete_at(
            &data_dir,
            Path::new(""),
            &scan,
            &AtomicBool::new(false),
            &progress(PHASE_DELETING),
        );
        assert_eq!(report.deleted_total(), 1);
        let connection = create_table(&data_dir, &STORES[rating]);
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM ratings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn cancellation_mid_descriptor_rolls_back_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let files = temp.path().join("files");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&files).unwrap();
        let rating = descriptor_index("rating.db", "ratings");
        for name in ["a.jpg", "b.jpg"] {
            insert_key(
                &data_dir,
                rating,
                &crate::adjustment_db::normalize_path(&files.join(name)),
            );
        }
        let scan = run_scan_at(
            &data_dir,
            Path::new(""),
            &AtomicBool::new(false),
            &progress(PHASE_SCANNING),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut seen = 0usize;
        let report = run_delete_with_hook(
            &data_dir,
            Path::new(""),
            &scan,
            &cancel,
            &progress(PHASE_DELETING),
            || {
                seen += 1;
                if seen == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert!(report.canceled);
        let connection = create_table(&data_dir, &STORES[rating]);
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM ratings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn delete_purge_journal_retries_confirmed_orphan_and_clears_itself() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let files = temp.path().join("files");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&files).unwrap();
        let removed = files.join("gone.jpg");
        let key = crate::adjustment_db::normalize_path(&removed);
        let rating = crate::rating_db::RatingDb::open_at(data_dir.join("rating.db")).unwrap();
        rating.set(&key, 5).unwrap();

        assert!(journal_failed_delete_purge(
            &data_dir,
            std::slice::from_ref(&removed),
            &[],
        ));
        assert!(data_dir.join(DELETE_PURGE_JOURNAL_FILE).exists());
        assert_eq!(
            rating.get(&key),
            5,
            "journal enqueue itself must not mutate DB"
        );

        let report = retry_delete_purge_journal_at(&data_dir);
        assert_eq!(report.attempted, 1);
        assert_eq!(report.purged, 1);
        assert_eq!(report.remaining, 0);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(rating.get(&key), 0);
        assert!(!data_dir.join(DELETE_PURGE_JOURNAL_FILE).exists());
    }
}
