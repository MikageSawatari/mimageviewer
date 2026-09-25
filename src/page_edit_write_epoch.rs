//! Process-wide publication stamp for page-edit DB writers and virtual-list readers.
//!
//! Two overlapping writers must never make a shared odd/even bit look quiescent.
#[cfg(test)]
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const MAX_NOTICES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WriteScope {
    Keys(Vec<String>),
    Full,
}

#[derive(Clone, Debug)]
pub(crate) struct WriteNotice {
    pub completed_writes: u64,
    pub scope: WriteScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WriteStamp {
    pub active_writers: usize,
    pub completed_writes: u64,
}

pub(crate) struct EditWriteEpoch {
    active_writers: AtomicUsize,
    completed_writes: AtomicU64,
    notices: Mutex<VecDeque<WriteNotice>>,
    repaint_context: Mutex<Option<egui::Context>>,
}

pub(crate) static PAGE_EDIT_WRITES: EditWriteEpoch = EditWriteEpoch::new();

#[cfg(test)]
struct TestEpochs {
    page: EditWriteEpoch,
    rating: EditWriteEpoch,
    tag: EditWriteEpoch,
}

/// One test's write publications, shared with its reader workers but not with other tests.
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct TestEpochHandle(&'static TestEpochs);

#[cfg(test)]
thread_local! {
    static TEST_EPOCHS: Cell<Option<TestEpochHandle>> = const { Cell::new(None) };
    static TEST_PROCESS_GLOBAL_READ_REASON: Cell<Option<&'static str>> = const { Cell::new(None) };
}

#[cfg(test)]
pub(crate) struct TestEpochScope {
    previous: Option<TestEpochHandle>,
}

#[cfg(test)]
impl TestEpochScope {
    pub(crate) fn fresh() -> Self {
        // The static API returns guards borrowing their epoch. Keep the small test instance
        // alive for the test process so worker threads and guards may finish in either order.
        let epochs = Box::leak(Box::new(TestEpochs {
            page: EditWriteEpoch::new(),
            rating: EditWriteEpoch::new(),
            tag: EditWriteEpoch::new(),
        }));
        Self::enter(TestEpochHandle(epochs))
    }

    pub(crate) fn capture() -> Option<TestEpochHandle> {
        TEST_EPOCHS.with(Cell::get)
    }

    pub(crate) fn enter(handle: TestEpochHandle) -> Self {
        let previous = TEST_EPOCHS.with(|scope| scope.replace(Some(handle)));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for TestEpochScope {
    fn drop(&mut self) {
        TEST_EPOCHS.with(|scope| scope.set(self.previous));
    }
}

/// Explicit exception for a test that intentionally reads the process-wide publication state.
#[cfg(test)]
pub(crate) struct TestProcessGlobalEpochRead {
    previous: Option<&'static str>,
}

#[cfg(test)]
impl TestProcessGlobalEpochRead {
    pub(crate) fn for_reason(reason: &'static str) -> Self {
        assert!(
            !reason.is_empty(),
            "a process-global epoch read needs a reason"
        );
        assert!(
            TestEpochScope::capture().is_none(),
            "a process-global epoch read cannot also use a private test scope"
        );
        let previous =
            TEST_PROCESS_GLOBAL_READ_REASON.with(|current| current.replace(Some(reason)));
        Self { previous }
    }

    fn is_active() -> bool {
        TEST_PROCESS_GLOBAL_READ_REASON.with(|current| current.get().is_some())
    }
}

#[cfg(test)]
impl Drop for TestProcessGlobalEpochRead {
    fn drop(&mut self) {
        TEST_PROCESS_GLOBAL_READ_REASON.with(|current| current.set(self.previous));
    }
}

#[cfg(test)]
pub(crate) fn with_foreign_scoped_writers(body: impl FnOnce()) {
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let writer = std::thread::spawn(move || {
        let _writer_scope = TestEpochScope::fresh();
        let _guards = [
            &PAGE_EDIT_WRITES,
            &crate::rating_db::RATING_WRITES,
            &crate::tags_db::TAG_WRITES,
        ]
        .map(EditWriteEpoch::begin);
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    entered_rx.recv().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    release_tx.send(()).unwrap();
    writer.join().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

impl EditWriteEpoch {
    pub const fn new() -> Self {
        Self {
            active_writers: AtomicUsize::new(0),
            completed_writes: AtomicU64::new(0),
            notices: Mutex::new(VecDeque::new()),
            repaint_context: Mutex::new(None),
        }
    }

    fn owner(&self) -> &Self {
        #[cfg(test)]
        if let Some(scope) = TestEpochScope::capture() {
            if std::ptr::eq(self, &PAGE_EDIT_WRITES) {
                return &scope.0.page;
            }
            if std::ptr::eq(self, &crate::rating_db::RATING_WRITES) {
                return &scope.0.rating;
            }
            if std::ptr::eq(self, &crate::tags_db::TAG_WRITES) {
                return &scope.0.tag;
            }
        }
        self
    }

    #[track_caller]
    fn owner_for_read(&self) -> &Self {
        #[cfg(test)]
        if (std::ptr::eq(self, &PAGE_EDIT_WRITES)
            || std::ptr::eq(self, &crate::rating_db::RATING_WRITES)
            || std::ptr::eq(self, &crate::tags_db::TAG_WRITES))
            && TestEpochScope::capture().is_none()
            && !TestProcessGlobalEpochRead::is_active()
        {
            panic!(
                "unscoped process-global write epoch read at {}; enter TestEpochScope or explicitly use TestProcessGlobalEpochRead::for_reason",
                std::panic::Location::caller()
            );
        }
        self.owner()
    }

    pub fn begin(&self) -> EditWriteGuard<'_> {
        let owner = self.owner();
        owner.active_writers.fetch_add(1, Ordering::SeqCst);
        EditWriteGuard {
            owner,
            scope: WriteScope::Full,
        }
    }

    pub fn begin_for_key(&self, key: &str) -> EditWriteGuard<'_> {
        let scope = WriteScope::Keys(vec![key.to_owned()]);
        let owner = self.owner();
        owner.active_writers.fetch_add(1, Ordering::SeqCst);
        EditWriteGuard { owner, scope }
    }

    pub fn begin_for_keys(&self, keys: &[String]) -> EditWriteGuard<'_> {
        let scope = WriteScope::Keys(keys.to_vec());
        let owner = self.owner();
        owner.active_writers.fetch_add(1, Ordering::SeqCst);
        EditWriteGuard { owner, scope }
    }

    pub fn register_repaint_context(&self, ctx: &egui::Context) {
        *self
            .owner()
            .repaint_context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ctx.clone());
    }

    #[track_caller]
    pub fn repaint_context(&self) -> Option<egui::Context> {
        self.owner_for_read()
            .repaint_context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// A missing notice means overflow, injection, or a panicking publisher. The reader must
    /// re-prepare the entire installed order rather than guessing which key changed.
    #[track_caller]
    pub fn notices_since(&self, completed: u64) -> Option<Vec<WriteNotice>> {
        let owner = self.owner_for_read();
        let notices = owner
            .notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = owner.completed_writes.load(Ordering::SeqCst);
        if completed == current {
            return Some(Vec::new());
        }
        let mut expected = completed.wrapping_add(1);
        let mut result = Vec::new();
        for notice in notices
            .iter()
            .filter(|notice| notice.completed_writes > completed)
        {
            if notice.completed_writes != expected {
                return None;
            }
            result.push(notice.clone());
            expected = expected.wrapping_add(1);
        }
        (expected == current.wrapping_add(1)).then_some(result)
    }

    /// `None` requests a full reread (unknown scope, missing notification, or too many keys).
    /// A bounded key set keeps the UI drain independent of the size of the write history.
    #[track_caller]
    pub fn changed_keys_since(
        &self,
        completed: u64,
        expected_completed: u64,
    ) -> Option<Vec<String>> {
        let owner = self.owner_for_read();
        let notices = owner
            .notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = owner.completed_writes.load(Ordering::SeqCst);
        if current != expected_completed {
            return None;
        }
        if completed == current {
            return Some(Vec::new());
        }
        let mut expected = completed.wrapping_add(1);
        let mut keys = std::collections::HashSet::new();
        for notice in notices
            .iter()
            .filter(|notice| notice.completed_writes > completed)
        {
            if notice.completed_writes != expected {
                return None;
            }
            expected = expected.wrapping_add(1);
            match &notice.scope {
                WriteScope::Full => return None,
                WriteScope::Keys(changed) => {
                    keys.extend(changed.iter().cloned());
                    if keys.len() > 128 {
                        return None;
                    }
                }
            }
        }
        (expected == current.wrapping_add(1)).then(|| keys.into_iter().collect())
    }

    #[track_caller]
    pub fn sample(&self) -> WriteStamp {
        let owner = self.owner_for_read();
        // Bracket the completion load so a writer that starts or finishes during
        // this sample cannot look quiescent at the acceptance boundary.
        let active_before = owner.active_writers.load(Ordering::SeqCst);
        let completed_writes = owner.completed_writes.load(Ordering::SeqCst);
        let active_after = owner.active_writers.load(Ordering::SeqCst);
        WriteStamp {
            active_writers: active_before.max(active_after),
            completed_writes,
        }
    }

    pub fn read_is_stable(before: WriteStamp, after: WriteStamp) -> bool {
        before.active_writers == 0
            && after.active_writers == 0
            && before.completed_writes == after.completed_writes
    }

    #[track_caller]
    pub fn accepts(&self, prepared: WriteStamp) -> bool {
        let now = self.sample();
        prepared.active_writers == 0
            && now.active_writers == 0
            && prepared.completed_writes == now.completed_writes
    }
}

pub(crate) struct EditWriteGuard<'a> {
    owner: &'a EditWriteEpoch,
    scope: WriteScope,
}

impl Drop for EditWriteGuard<'_> {
    fn drop(&mut self) {
        {
            let mut notices = self
                .owner
                .notices
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let completed_writes = self.owner.completed_writes.fetch_add(1, Ordering::SeqCst) + 1;
            if notices.len() == MAX_NOTICES {
                notices.pop_front();
            }
            notices.push_back(WriteNotice {
                completed_writes,
                scope: std::mem::replace(&mut self.scope, WriteScope::Full),
            });
        }
        self.owner.active_writers.fetch_sub(1, Ordering::SeqCst);
        if let Some(ctx) = self
            .owner
            .repaint_context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unscoped_process_global_read_reports_its_call_site() {
        let reads: [fn(&EditWriteEpoch); 5] = [
            |epoch| {
                let _ = epoch.sample();
            },
            |epoch| {
                let _ = epoch.accepts(WriteStamp {
                    active_writers: 0,
                    completed_writes: 0,
                });
            },
            |epoch| {
                let _ = epoch.notices_since(0);
            },
            |epoch| {
                let _ = epoch.changed_keys_since(0, 0);
            },
            |epoch| {
                let _ = epoch.repaint_context();
            },
        ];
        for epoch in [
            &PAGE_EDIT_WRITES,
            &crate::rating_db::RATING_WRITES,
            &crate::tags_db::TAG_WRITES,
        ] {
            for read in reads {
                let panic = std::panic::catch_unwind(|| read(epoch)).unwrap_err();
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap();
                assert!(message.contains("unscoped process-global write epoch read"));
                assert!(message.contains(file!()), "{message}");
            }
        }
    }

    #[test]
    fn explicit_process_global_read_opt_in_requires_a_reason() {
        assert!(std::panic::catch_unwind(|| TestProcessGlobalEpochRead::for_reason("")).is_err());
        let _global = TestProcessGlobalEpochRead::for_reason(
            "exercise the intentional process-global read exception itself",
        );
        let _ = PAGE_EDIT_WRITES.sample();
    }

    #[test]
    fn test_scope_keeps_worker_writes_visible_to_its_reader() {
        let _scope = TestEpochScope::fresh();
        let epochs = [
            &PAGE_EDIT_WRITES,
            &crate::rating_db::RATING_WRITES,
            &crate::tags_db::TAG_WRITES,
        ];
        let before = epochs.map(EditWriteEpoch::sample);
        let worker_epochs = epochs;
        let handle = TestEpochScope::capture().unwrap();
        std::thread::spawn(move || {
            let _scope = TestEpochScope::enter(handle);
            for epoch in worker_epochs {
                drop(epoch.begin());
            }
        })
        .join()
        .unwrap();
        for (epoch, stamp) in epochs.into_iter().zip(before) {
            assert!(!epoch.accepts(stamp));
            assert_eq!(
                epoch.notices_since(stamp.completed_writes).unwrap().len(),
                1
            );
        }
    }

    #[test]
    fn test_scope_ignores_another_tests_active_writers() {
        let _reader_scope = TestEpochScope::fresh();
        let epochs = [
            &PAGE_EDIT_WRITES,
            &crate::rating_db::RATING_WRITES,
            &crate::tags_db::TAG_WRITES,
        ];
        let before = epochs.map(EditWriteEpoch::sample);
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let writer = std::thread::spawn(move || {
            let _writer_scope = TestEpochScope::fresh();
            let _guards = epochs.map(EditWriteEpoch::begin);
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.recv().unwrap();
        for (epoch, stamp) in epochs.into_iter().zip(before) {
            assert!(epoch.accepts(stamp));
        }
        release_tx.send(()).unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn writer_before_and_after_commit_rejects_stale_read() {
        let epoch = EditWriteEpoch::new();
        let before = epoch.sample();
        let writer = epoch.begin();
        assert!(!EditWriteEpoch::read_is_stable(before, epoch.sample()));
        assert!(!epoch.accepts(before));
        // The writer is paused just after its commit while its guard remains live.
        assert!(!epoch.accepts(before));
        drop(writer);
        assert!(!epoch.accepts(before));
        let next = epoch.sample();
        assert!(EditWriteEpoch::read_is_stable(next, epoch.sample()));
        assert!(epoch.accepts(next));
    }

    #[test]
    fn overlapping_writers_cannot_expose_a_quiescent_stamp() {
        let epoch = EditWriteEpoch::new();
        let before = epoch.sample();
        let first = epoch.begin();
        let second = epoch.begin();
        drop(first);
        assert_eq!(epoch.sample().active_writers, 1);
        assert!(!epoch.accepts(before));
        drop(second);
        assert_eq!(epoch.sample().completed_writes, 2);
        assert!(!epoch.accepts(before));
    }

    #[test]
    fn paused_writer_before_and_after_sql_commit_blocks_prepare() {
        use std::sync::{Arc, mpsc};
        let epoch = Arc::new(EditWriteEpoch::new());
        let (stage_tx, stage_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let writer_epoch = Arc::clone(&epoch);
        let writer = std::thread::spawn(move || {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            conn.execute_batch("CREATE TABLE edits (key TEXT PRIMARY KEY)")
                .unwrap();
            let guard = writer_epoch.begin();
            stage_tx.send(0).unwrap(); // guard acquired, before DB write
            resume_rx.recv().unwrap();
            conn.execute("INSERT INTO edits (key) VALUES ('page')", [])
                .unwrap();
            stage_tx.send(1).unwrap(); // committed, guard still active
            resume_rx.recv().unwrap();
            drop(guard);
        });
        let old = epoch.sample();
        assert_eq!(stage_rx.recv().unwrap(), 0);
        assert!(!epoch.accepts(old));
        resume_tx.send(()).unwrap();
        assert_eq!(stage_rx.recv().unwrap(), 1);
        assert!(!epoch.accepts(old));
        resume_tx.send(()).unwrap();
        writer.join().unwrap();
        assert!(!epoch.accepts(old));
    }

    #[test]
    fn nested_and_multi_store_guards_publish_each_completion() {
        let epoch = EditWriteEpoch::new();
        let outer = epoch.begin_for_keys(&["page-a".into(), "page-b".into()]);
        let inner = epoch.begin_for_key("page-a");
        drop(inner);
        assert_eq!(epoch.sample().active_writers, 1);
        drop(outer);
        let notices = epoch.notices_since(0).unwrap();
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0].scope, WriteScope::Keys(vec!["page-a".into()]));
        assert_eq!(
            notices[1].scope,
            WriteScope::Keys(vec!["page-a".into(), "page-b".into()])
        );
        let keys = epoch
            .changed_keys_since(0, epoch.sample().completed_writes)
            .unwrap();
        assert_eq!(
            keys.into_iter().collect::<std::collections::HashSet<_>>(),
            ["page-a".to_owned(), "page-b".to_owned()].into()
        );
    }

    #[test]
    fn missing_or_overflowed_notice_is_a_detectable_gap() {
        let epoch = EditWriteEpoch::new();
        drop(epoch.begin_for_key("first"));
        drop(epoch.begin_for_key("second"));
        epoch.notices.lock().unwrap().pop_front();
        assert!(epoch.notices_since(0).is_none());
        assert!(
            epoch
                .changed_keys_since(0, epoch.sample().completed_writes)
                .is_none()
        );
        for _ in 0..=MAX_NOTICES {
            drop(epoch.begin());
        }
        assert!(epoch.notices_since(2).is_none());
        assert!(
            epoch
                .changed_keys_since(2, epoch.sample().completed_writes)
                .is_none()
        );
    }

    #[test]
    fn panic_drops_guard_and_publishes_completion() {
        let epoch = EditWriteEpoch::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _writer = epoch.begin_for_key("panic-page");
            panic!("injected writer panic");
        }));
        assert!(result.is_err());
        assert_eq!(epoch.sample().active_writers, 0);
        assert_eq!(epoch.notices_since(0).unwrap().len(), 1);
        epoch.notices.lock().unwrap().pop_front();
        assert!(
            epoch
                .changed_keys_since(0, epoch.sample().completed_writes)
                .is_none(),
            "a lost panic completion must force a full reread"
        );
    }

    #[test]
    fn idle_view_gets_a_repaint_when_a_writer_finishes() {
        let epoch = EditWriteEpoch::new();
        let ctx = egui::Context::default();
        let wakes = std::sync::Arc::new(AtomicUsize::new(0));
        let received = std::sync::Arc::clone(&wakes);
        ctx.set_request_repaint_callback(move |_| {
            received.fetch_add(1, Ordering::SeqCst);
        });
        epoch.register_repaint_context(&ctx);
        drop(epoch.begin_for_key("idle-page"));
        assert!(wakes.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn key_delta_cannot_consume_a_later_write_outside_its_batch() {
        let epoch = EditWriteEpoch::new();
        drop(epoch.begin_for_key("a"));
        let observed = epoch.sample();
        drop(epoch.begin_for_key("b"));
        assert!(
            epoch
                .changed_keys_since(0, observed.completed_writes)
                .is_none()
        );
        let current = epoch.sample();
        let keys = epoch
            .changed_keys_since(0, current.completed_writes)
            .unwrap();
        assert_eq!(
            keys.into_iter().collect::<std::collections::HashSet<_>>(),
            ["a".to_owned(), "b".to_owned()].into()
        );
    }
}
