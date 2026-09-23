//! Process-wide publication stamp for page-edit DB writers and virtual-list readers.
//!
//! Two overlapping writers must never make a shared odd/even bit look quiescent.
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WriteStamp {
    pub active_writers: usize,
    pub completed_writes: u64,
}

pub(crate) struct EditWriteEpoch {
    active_writers: AtomicUsize,
    completed_writes: AtomicU64,
}

pub(crate) static PAGE_EDIT_WRITES: EditWriteEpoch = EditWriteEpoch::new();

impl EditWriteEpoch {
    pub const fn new() -> Self {
        Self {
            active_writers: AtomicUsize::new(0),
            completed_writes: AtomicU64::new(0),
        }
    }

    pub fn begin(&self) -> EditWriteGuard<'_> {
        self.active_writers.fetch_add(1, Ordering::SeqCst);
        EditWriteGuard { owner: self }
    }

    pub fn sample(&self) -> WriteStamp {
        // Bracket the completion load so a writer that starts or finishes during
        // this sample cannot look quiescent at the acceptance boundary.
        let active_before = self.active_writers.load(Ordering::SeqCst);
        let completed_writes = self.completed_writes.load(Ordering::SeqCst);
        let active_after = self.active_writers.load(Ordering::SeqCst);
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

    pub fn accepts(&self, prepared: WriteStamp) -> bool {
        let now = self.sample();
        prepared.active_writers == 0
            && now.active_writers == 0
            && prepared.completed_writes == now.completed_writes
    }
}

pub(crate) struct EditWriteGuard<'a> {
    owner: &'a EditWriteEpoch,
}

impl Drop for EditWriteGuard<'_> {
    fn drop(&mut self) {
        self.owner.completed_writes.fetch_add(1, Ordering::SeqCst);
        self.owner.active_writers.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
