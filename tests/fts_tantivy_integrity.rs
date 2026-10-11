//! Exercises the patched dependency itself, using disposable on-disk indexes.
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use mimageviewer::fts_integrity::{FailureReporter, MaintenanceState, UPDATE_FAILURE_NOTICE};
use mimageviewer::fts_meta::FtsMetaDb;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use tantivy::collector::TopDocs;
use tantivy::directory::{Directory, META_LOCK, MmapDirectory};
use tantivy::merge_policy::{MergeCandidate, MergePolicy, NoMergePolicy};
use tantivy::query::AllQuery;
use tantivy::schema::{Field, STORED, STRING, Schema, TEXT, Value};
use tantivy::{
    Index, IndexSettings, IndexWriter, SegmentMeta, TantivyDocument, TantivyError, Term, doc,
};

const WAIT: Duration = Duration::from_secs(30);

struct DiagnosticLog;
static DIAGNOSTIC_LOG: DiagnosticLog = DiagnosticLog;
static DIAGNOSTIC_INIT: Once = Once::new();
static DIAGNOSTIC_RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl log::Log for DiagnosticLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("tantivy::") && metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            DIAGNOSTIC_RECORDS
                .lock()
                .unwrap()
                .push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

fn capture_diagnostics() {
    DIAGNOSTIC_INIT.call_once(|| {
        log::set_logger(&DIAGNOSTIC_LOG).unwrap();
        log::set_max_level(log::LevelFilter::Warn);
    });
}

fn diagnostics_for(path: &Path) -> Vec<String> {
    let folder = path.file_name().unwrap().to_str().unwrap();
    DIAGNOSTIC_RECORDS
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.contains(folder))
        .cloned()
        .collect()
}

struct Fixture {
    dir: tempfile::TempDir,
    mmap: MmapDirectory,
    index: Index,
    writer: IndexWriter,
    path: Field,
    body: Field,
}

fn fixture() -> Fixture {
    capture_diagnostics();
    let dir = tempfile::tempdir().unwrap();
    let mmap = MmapDirectory::open(dir.path()).unwrap();
    let mut builder = Schema::builder();
    let path = builder.add_text_field("path", STRING | STORED);
    let body = builder.add_text_field("body", TEXT | STORED);
    let index = Index::create(mmap.clone(), builder.build(), IndexSettings::default()).unwrap();
    let writer = index.writer_with_num_threads(1, 50_000_000).unwrap();
    writer.set_merge_policy(Box::new(NoMergePolicy));
    Fixture {
        dir,
        mmap,
        index,
        writer,
        path,
        body,
    }
}

fn all_docs(index: &Index, path: Field, body: Field) -> BTreeMap<String, String> {
    let reader = index.reader().unwrap();
    reader.reload().unwrap();
    let searcher = reader.searcher();
    let addresses = searcher
        .search(&AllQuery, &TopDocs::with_limit(100).order_by_score())
        .unwrap();
    let mut actual = BTreeMap::new();
    for (_, address) in addresses {
        let document: TantivyDocument = searcher.doc(address).unwrap();
        let path = document
            .get_first(path)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let body = document
            .get_first(body)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        assert!(actual.insert(path, body).is_none(), "duplicate live path");
    }
    assert_eq!(actual.len() as u64, searcher.num_docs());
    actual
}

#[cfg(windows)]
fn block_replacement(path: &Path) -> File {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE, deliberately no FILE_SHARE_DELETE.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(path)
        .unwrap()
}

#[cfg(windows)]
#[test]
fn patched_persist_recovery_preserves_mixed_commit_after_reopen() {
    let mut f = fixture();
    for (path, body) in [
        ("updated", "old"),
        ("deleted", "gone"),
        ("unchanged", "same"),
    ] {
        f.writer
            .add_document(doc!(f.path => path, f.body => body))
            .unwrap();
    }
    f.writer.commit().unwrap();
    let stages = Arc::new(Mutex::new(Vec::new()));
    let handle = Arc::new(Mutex::new(None));
    let observed = stages.clone();
    let blocker = handle.clone();
    f.mmap
        .set_atomic_write_test_hook(move |path, stage, attempt| {
            if path.file_name().unwrap() == "meta.json" {
                observed.lock().unwrap().push((stage, attempt));
                if stage == "persist" {
                    if attempt == 1 {
                        *blocker.lock().unwrap() = Some(block_replacement(path));
                    } else if attempt == 3 {
                        blocker.lock().unwrap().take();
                    }
                }
            }
            Ok(())
        });
    f.writer
        .delete_term(Term::from_field_text(f.path, "updated"));
    f.writer
        .add_document(doc!(f.path => "updated", f.body => "new"))
        .unwrap();
    f.writer
        .delete_term(Term::from_field_text(f.path, "deleted"));
    f.writer
        .add_document(doc!(f.path => "added", f.body => "fresh"))
        .unwrap();
    let start = Instant::now();
    // Exactly one upper-level commit; only the completed TempPath is re-persisted.
    f.writer.commit().unwrap();
    assert!(start.elapsed() >= Duration::from_millis(35));
    let records = diagnostics_for(f.dir.path());
    assert_eq!(records.len(), 3);
    assert_eq!(
        records
            .iter()
            .filter(|s| s.contains("atomic_write failed"))
            .count(),
        2
    );
    assert!(records.iter().any(|s| {
        s.contains("atomic_write recovered origin=foreground stage=persist")
            && s.contains("attempts=3")
    }));
    assert_eq!(
        *stages.lock().unwrap(),
        vec![
            ("temp-create", 1),
            ("write", 1),
            ("flush", 1),
            ("sync", 1),
            ("persist", 1),
            ("persist", 2),
            ("persist", 3),
        ]
    );
    let Fixture {
        dir,
        writer,
        index,
        mmap,
        path,
        body,
    } = f;
    drop(writer);
    drop(index);
    drop(mmap);
    drop(handle);
    let reopened = Index::open_in_dir(dir.path()).unwrap();
    let expected = BTreeMap::from([
        ("added".to_owned(), "fresh".to_owned()),
        ("updated".to_owned(), "new".to_owned()),
        ("unchanged".to_owned(), "same".to_owned()),
    ]);
    assert_eq!(all_docs(&reopened, path, body), expected);
}

#[cfg(windows)]
#[test]
fn permanent_persist_denial_preserves_destination_and_raw_error() {
    capture_diagnostics();
    for filename in ["meta.json", ".managed.json"] {
        let tmp = tempfile::tempdir().unwrap();
        let mmap = MmapDirectory::open(tmp.path()).unwrap();
        let path = Path::new(filename);
        mmap.atomic_write(path, b"old bytes").unwrap();
        let blocker = block_replacement(&tmp.path().join(path));
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        mmap.set_atomic_write_test_hook(move |_, stage, _| {
            if stage == "persist" {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        });
        let error = mmap.atomic_write(path, b"new bytes").unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32 | 33)));
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        let records = diagnostics_for(tmp.path());
        assert_eq!(records.len(), 4);
        for (attempt, record) in records.iter().enumerate() {
            assert!(record.contains(filename));
            assert!(record.contains("stage=persist"));
            assert!(record.contains("raw_os_error=Some("));
            assert!(record.contains(&format!("attempt={}", attempt + 1)));
        }
        assert_eq!(std::fs::read(tmp.path().join(path)).unwrap(), b"old bytes");
        drop(blocker);
        // The failed TempPath was cleaned up, without deleting the destination.
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}

#[test]
fn atomic_nonpersist_stages_are_never_retried() {
    capture_diagnostics();
    for failed_stage in ["temp-create", "write", "flush", "sync"] {
        let tmp = tempfile::tempdir().unwrap();
        let mmap = MmapDirectory::open(tmp.path()).unwrap();
        mmap.atomic_write(Path::new("meta.json"), b"old").unwrap();
        let failures = Arc::new(AtomicUsize::new(0));
        let observed = failures.clone();
        mmap.set_atomic_write_test_hook(move |_, stage, _| {
            if stage == failed_stage {
                observed.fetch_add(1, Ordering::SeqCst);
                return Err(io::Error::from_raw_os_error(5));
            }
            assert_ne!(stage, "persist");
            Ok(())
        });
        assert_eq!(
            mmap.atomic_write(Path::new("meta.json"), b"new")
                .unwrap_err()
                .raw_os_error(),
            Some(5)
        );
        assert_eq!(failures.load(Ordering::SeqCst), 1);
        let records = diagnostics_for(tmp.path());
        assert_eq!(records.len(), 1);
        assert!(records[0].contains(&format!("stage={failed_stage}")));
        assert!(records[0].contains("raw_os_error=Some(5) attempt=1"));
        assert_eq!(mmap.atomic_read(Path::new("meta.json")).unwrap(), b"old");
    }
}

#[test]
fn only_windows_persist_raw_codes_have_the_four_attempt_budget() {
    capture_diagnostics();
    for code in [5, 32, 33, 2, 13, 28] {
        let tmp = tempfile::tempdir().unwrap();
        let mmap = MmapDirectory::open(tmp.path()).unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        mmap.set_atomic_write_test_hook(move |_, stage, _| {
            if stage == "persist" {
                observed.fetch_add(1, Ordering::SeqCst);
                return Err(io::Error::from_raw_os_error(code));
            }
            Ok(())
        });
        assert_eq!(
            mmap.atomic_write(Path::new("meta.json"), b"data")
                .unwrap_err()
                .raw_os_error(),
            Some(code)
        );
        let expected = if cfg!(windows) && matches!(code, 5 | 32 | 33) {
            4
        } else {
            1
        };
        assert_eq!(attempts.load(Ordering::SeqCst), expected);
    }
    let tmp = tempfile::tempdir().unwrap();
    let mmap = MmapDirectory::open(tmp.path()).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    mmap.set_atomic_write_test_hook(move |_, stage, _| {
        if stage == "persist" {
            observed.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    });
    mmap.atomic_write(Path::new("meta.json"), b"normal")
        .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[derive(Debug, Default)]
struct MergeOnce(AtomicBool);

impl MergePolicy for MergeOnce {
    fn compute_merge_candidates(&self, segments: &[SegmentMeta]) -> Vec<MergeCandidate> {
        if segments.len() >= 2 && !self.0.swap(true, Ordering::SeqCst) {
            vec![MergeCandidate(segments.iter().map(|s| s.id()).collect())]
        } else {
            Vec::new()
        }
    }
}

#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn wait(&self) {
        let (lock, cv) = &*self.0;
        let opened = lock.lock().unwrap();
        let (opened, _) = cv
            .wait_timeout_while(opened, WAIT, |opened| !*opened)
            .unwrap();
        assert!(*opened, "test gate timed out");
    }

    fn open(&self) {
        let (lock, cv) = &*self.0;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
}

fn merge_fixture() -> Fixture {
    let mut f = fixture();
    for path in ["one", "two"] {
        f.writer
            .add_document(doc!(f.path => path, f.body => path))
            .unwrap();
        f.writer.commit().unwrap();
    }
    f
}

fn reporter_for(dir: &Path) -> (Arc<FailureReporter>, PathBuf) {
    let path = dir.join("fts_meta.db");
    let meta = Arc::new(FtsMetaDb::open_at(&path).unwrap());
    let reporter = FailureReporter::new(meta);
    assert_eq!(reporter.snapshot(), MaintenanceState::Ready);
    assert!(!durable_pending(&path));
    (reporter, path)
}

fn durable_pending(path: &Path) -> bool {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let value: Option<i64> = conn
        .query_row(
            "SELECT value FROM index_state WHERE key='tantivy_rebuild_pending'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    value.is_some_and(|value| value != 0)
}

fn observe_terminal(writer: &IndexWriter) -> mpsc::Receiver<bool> {
    let (tx, rx) = mpsc::channel();
    writer.set_merge_terminal_test_hook(move |killed_before_schedule| {
        tx.send(killed_before_schedule).unwrap();
    });
    rx
}

fn attach_reporter(
    writer: &IndexWriter,
    reporter: &Arc<FailureReporter>,
) -> mpsc::Receiver<TantivyError> {
    let reporter = reporter.clone();
    let (tx, rx) = mpsc::channel();
    writer.set_merge_failure_handler(move |error| {
        reporter.report("tantivy auto merge", error);
        tx.send(error.clone()).unwrap();
    });
    rx
}

fn arm_before_end(writer: &IndexWriter) -> (Gate, mpsc::Receiver<()>) {
    let gate = Gate::default();
    let blocked = gate.clone();
    let (tx, rx) = mpsc::channel();
    writer.set_merge_before_end_test_hook(move || {
        tx.send(()).unwrap();
        blocked.wait();
    });
    (gate, rx)
}

fn trigger_auto_merge(writer: &mut IndexWriter) {
    writer.set_merge_policy(Box::<MergeOnce>::default());
    // No explicit merge Future is constructed by this test. Tantivy discards its
    // automatic merge Future, and no further commit is issued after this trigger.
    writer.commit().unwrap();
}

#[cfg(windows)]
#[test]
fn auto_merge_terminal_persist_failure_reaches_reporter_without_a_consumer() {
    let mut f = merge_fixture();
    let (reporter, db_path) = reporter_for(f.dir.path());
    let failures = attach_reporter(&f.writer, &reporter);
    let terminal = observe_terminal(&f.writer);
    let (gate, started) = arm_before_end(&f.writer);
    trigger_auto_merge(&mut f.writer);
    started.recv_timeout(WAIT).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let blocker = Arc::new(Mutex::new(None));
    let held = blocker.clone();
    f.mmap
        .set_atomic_write_test_hook(move |path, stage, attempt| {
            if path.file_name().unwrap() == "meta.json" && stage == "persist" {
                observed.fetch_add(1, Ordering::SeqCst);
                if attempt == 1 {
                    *held.lock().unwrap() = Some(block_replacement(path));
                }
            }
            Ok(())
        });
    gate.open();
    let error = failures.recv_timeout(WAIT).unwrap();
    assert!(
        matches!(error, TantivyError::IoError(ref e) if matches!(e.raw_os_error(), Some(5 | 32 | 33)))
    );
    assert!(!terminal.recv_timeout(WAIT).unwrap());
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
    assert_eq!(reporter.snapshot(), MaintenanceState::RebuildOnRestart);
    assert_eq!(reporter.take_notifications(), vec![UPDATE_FAILURE_NOTICE]);
    assert!(durable_pending(&db_path));
    assert!(failures.try_recv().is_err());
    // Handler ran outside Directory/updater tasks: a subsequent standalone read
    // and META_LOCK acquisition succeed, with no extra commit or merge wait.
    f.mmap.atomic_read(Path::new("meta.json")).unwrap();
    drop(f.mmap.acquire_lock(&META_LOCK).unwrap());
    blocker.lock().unwrap().take();
}

#[test]
fn automatic_merge_shutdown_before_end_schedule_is_not_a_write_failure() {
    let mut f = merge_fixture();
    let (reporter, db_path) = reporter_for(f.dir.path());
    let failures = attach_reporter(&f.writer, &reporter);
    let terminal = observe_terminal(&f.writer);
    let (gate, started) = arm_before_end(&f.writer);
    trigger_auto_merge(&mut f.writer);
    started.recv_timeout(WAIT).unwrap();
    drop(f.writer); // normal Drop completes kill before scheduler admission
    gate.open();
    assert!(
        terminal.recv_timeout(WAIT).unwrap(),
        "expected typed admission refusal"
    );
    assert!(failures.try_recv().is_err());
    assert_eq!(reporter.snapshot(), MaintenanceState::Ready);
    assert!(reporter.take_notifications().is_empty());
    assert!(!durable_pending(&db_path));
}

#[test]
fn executed_same_message_system_error_during_shutdown_is_not_admission_refusal() {
    let mut f = merge_fixture();
    let (reporter, db_path) = reporter_for(f.dir.path());
    let failures = attach_reporter(&f.writer, &reporter);
    let terminal = observe_terminal(&f.writer);
    let gate = Gate::default();
    let held = gate.clone();
    let (started_tx, started) = mpsc::channel();
    f.writer.set_end_merge_test_hook(move || {
        started_tx.send(()).unwrap();
        held.wait();
        Err(TantivyError::SystemError(
            "Segment updater killed".to_owned(),
        ))
    });
    trigger_auto_merge(&mut f.writer);
    started.recv_timeout(WAIT).unwrap();
    drop(f.writer);
    gate.open();
    let error = failures.recv_timeout(WAIT).unwrap();
    assert!(
        matches!(error, TantivyError::SystemError(ref message) if message == "Segment updater killed")
    );
    assert!(!terminal.recv_timeout(WAIT).unwrap());
    assert_eq!(reporter.snapshot(), MaintenanceState::RebuildOnRestart);
    assert_eq!(reporter.take_notifications(), vec![UPDATE_FAILURE_NOTICE]);
    assert!(durable_pending(&db_path));
}

#[test]
fn rollback_preserves_the_writer_owned_failure_handler() {
    let mut f = merge_fixture();
    let (reporter, db_path) = reporter_for(f.dir.path());
    let failures = attach_reporter(&f.writer, &reporter);
    f.writer.rollback().unwrap();
    // Hooks are installed on the replacement updater; the production observer
    // was registered only before rollback and must follow the logical writer.
    let terminal = observe_terminal(&f.writer);
    f.writer
        .set_end_merge_test_hook(|| Err(TantivyError::SystemError("executed error".to_owned())));
    trigger_auto_merge(&mut f.writer);
    failures.recv_timeout(WAIT).unwrap();
    assert!(!terminal.recv_timeout(WAIT).unwrap());
    assert_eq!(reporter.take_notifications(), vec![UPDATE_FAILURE_NOTICE]);
    assert!(durable_pending(&db_path));
}

#[test]
fn invalid_candidate_before_merge_starts_is_not_a_write_failure() {
    let mut f = merge_fixture();
    let (reporter, db_path) = reporter_for(f.dir.path());
    let failures = attach_reporter(&f.writer, &reporter);
    assert!(
        f.writer
            .merge(&[tantivy::index::SegmentId::generate_random()])
            .wait()
            .is_err()
    );
    assert!(failures.try_recv().is_err());
    assert_eq!(reporter.snapshot(), MaintenanceState::Ready);
    assert!(reporter.take_notifications().is_empty());
    assert!(!durable_pending(&db_path));
}

#[cfg(windows)]
#[test]
fn accepted_end_merge_persist_failure_during_shutdown_still_reports() {
    let mut f = merge_fixture();
    let (reporter, db_path) = reporter_for(f.dir.path());
    let failures = attach_reporter(&f.writer, &reporter);
    let terminal = observe_terminal(&f.writer);
    let (before_end, merge_started) = arm_before_end(&f.writer);
    trigger_auto_merge(&mut f.writer);
    merge_started.recv_timeout(WAIT).unwrap();
    let io_gate = Gate::default();
    let held_gate = io_gate.clone();
    let (io_started_tx, io_started) = mpsc::channel();
    let blocker = Arc::new(Mutex::new(None));
    let held = blocker.clone();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    f.mmap
        .set_atomic_write_test_hook(move |path, stage, attempt| {
            if path.file_name().unwrap() == "meta.json" && stage == "persist" {
                observed.fetch_add(1, Ordering::SeqCst);
                if attempt == 1 {
                    *held.lock().unwrap() = Some(block_replacement(path));
                    io_started_tx.send(()).unwrap();
                    held_gate.wait();
                }
            }
            Ok(())
        });
    before_end.open();
    io_started.recv_timeout(WAIT).unwrap();
    drop(f.writer); // accepted task is already inside atomic I/O
    io_gate.open();
    failures.recv_timeout(WAIT).unwrap();
    assert!(!terminal.recv_timeout(WAIT).unwrap());
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
    assert_eq!(reporter.take_notifications(), vec![UPDATE_FAILURE_NOTICE]);
    assert_eq!(reporter.snapshot(), MaintenanceState::RebuildOnRestart);
    assert!(durable_pending(&db_path));
    blocker.lock().unwrap().take();
}

#[cfg(windows)]
#[test]
fn mapped_snapshot_gc_refusal_does_not_report_index_failure() {
    let mut f = merge_fixture();
    let (reporter, db_path) = reporter_for(f.dir.path());
    let failures = attach_reporter(&f.writer, &reporter);
    // A separately opened Index has a different meta inventory: the writer's GC
    // can see these old files as dead while their reader mmap remains in use.
    let old_index = Index::open_in_dir(f.dir.path()).unwrap();
    let old_reader = old_index.reader().unwrap();
    let old_searcher = old_reader.searcher();
    // Recent Windows/Rust combinations can delete a file while a mapped section
    // remains usable. Keep a no-delete-sharing read handle as well, so the GC
    // refusal itself is deterministic instead of assuming mmap always denies it.
    let mapped_store = std::fs::read_dir(f.dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "store"))
        .unwrap();
    let blocker = block_replacement(&mapped_store);
    let terminal = observe_terminal(&f.writer);
    trigger_auto_merge(&mut f.writer);
    assert!(!terminal.recv_timeout(WAIT).unwrap());
    let gc = f.writer.garbage_collect_files().wait().unwrap();
    assert!(
        !gc.failed_to_delete_files.is_empty(),
        "expected actual Windows mmap deletion refusal"
    );
    assert_eq!(old_searcher.num_docs(), 2);
    assert_eq!(reporter.snapshot(), MaintenanceState::Ready);
    assert!(reporter.take_notifications().is_empty());
    assert!(!durable_pending(&db_path));
    assert!(failures.try_recv().is_err());
    drop(blocker);
}
