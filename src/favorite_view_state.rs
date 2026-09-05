use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use uuid::Uuid;

use crate::settings::FavoriteViewState;

pub(crate) const FAVORITE_VIEW_WRITE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub(crate) enum FavoriteViewStoreCommand {
    Set { id: Uuid, state: FavoriteViewState },
    Remove { id: Uuid },
    Clear,
}

#[derive(Debug)]
pub(crate) struct FavoriteViewStoreResult {
    pub(crate) command: FavoriteViewStoreCommand,
    pub(crate) result: Result<usize, String>,
}

enum WorkerMessage {
    Store(FavoriteViewStoreCommand),
    Shutdown,
}

/// `favorite_view_states` だけを書き込む直列 worker。
///
/// UI thread は debounce 後の command を送るだけで、SQLite open / JSON serialize /
/// commit はすべてここで行う。単一 queue なので Set → Remove / Clear の順序も保たれる。
pub(crate) struct FavoriteViewStoreWriter {
    tx: Sender<WorkerMessage>,
    result_rx: Receiver<FavoriteViewStoreResult>,
    submitted: Arc<AtomicUsize>,
    done: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FavoriteViewStoreWriter {
    pub(crate) fn spawn(db_path: PathBuf) -> std::io::Result<Self> {
        let (tx, rx) = unbounded();
        let (result_tx, result_rx) = unbounded();
        let submitted = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let worker_done = Arc::clone(&done);
        let thread = std::thread::Builder::new()
            .name("favorite-view-store".to_owned())
            .spawn(move || run_store_worker(db_path, rx, result_tx, worker_done))?;
        Ok(Self {
            tx,
            result_rx,
            submitted,
            done,
            thread: Some(thread),
        })
    }

    pub(crate) fn submit(&self, command: FavoriteViewStoreCommand) -> Result<(), String> {
        self.tx
            .send(WorkerMessage::Store(command))
            .map_err(|_| "favorite view store worker stopped".to_owned())?;
        self.submitted.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub(crate) fn try_recv(&self) -> Option<FavoriteViewStoreResult> {
        self.result_rx.try_recv().ok()
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.submitted.load(Ordering::Acquire) != self.done.load(Ordering::Acquire)
            || !self.result_rx.is_empty()
    }
}

impl Drop for FavoriteViewStoreWriter {
    fn drop(&mut self) {
        // Shutdown は同じ FIFO の末尾に入る。先行する Set / Remove / Clear を捨てずに
        // drain してから終了し、アプリ終了直前の debounce flush も確実に commit する。
        let _ = self.tx.send(WorkerMessage::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_store_worker(
    db_path: PathBuf,
    rx: Receiver<WorkerMessage>,
    result_tx: Sender<FavoriteViewStoreResult>,
    done: Arc<AtomicUsize>,
) {
    let mut db: Option<crate::adjustment_db::AdjustmentDb> = None;
    while let Ok(message) = rx.recv() {
        let WorkerMessage::Store(command) = message else {
            break;
        };
        let result = (|| {
            if db.is_none() {
                db = Some(
                    crate::adjustment_db::AdjustmentDb::open_at(&db_path)
                        .map_err(|error| error.to_string())?,
                );
            }
            let db = db.as_ref().expect("favorite view DB initialized");
            match &command {
                FavoriteViewStoreCommand::Set { id, state } => db
                    .set_favorite_view_state(*id, state)
                    .map(|()| 1)
                    .map_err(|error| error.to_string()),
                FavoriteViewStoreCommand::Remove { id } => db
                    .remove_favorite_view_state(*id)
                    .map(|()| 1)
                    .map_err(|error| error.to_string()),
                FavoriteViewStoreCommand::Clear => db
                    .clear_favorite_view_states()
                    .map_err(|error| error.to_string()),
            }
        })();
        // result を publish してから done を進める。UI 側が drain と busy 判定の間で
        // completion を取り逃がして repaint が止まる race を避ける。
        let _ = result_tx.send(FavoriteViewStoreResult { command, result });
        done.fetch_add(1, Ordering::Release);
    }
}

#[derive(Clone)]
struct PendingWrite {
    state: FavoriteViewState,
    due_at: Instant,
}

/// お気に入り表示状態の DB 書き込みを UUID ごとにまとめる。
///
/// メモリ上の正本は即時更新し、永続化だけを最後の変更から 500ms 後へ遅延する。
#[derive(Default)]
pub(crate) struct FavoriteViewWriteDebounce {
    pending: HashMap<Uuid, PendingWrite>,
}

impl FavoriteViewWriteDebounce {
    pub(crate) fn note_at(&mut self, id: Uuid, state: FavoriteViewState, now: Instant) {
        self.pending.insert(
            id,
            PendingWrite {
                state,
                due_at: now + FAVORITE_VIEW_WRITE_DEBOUNCE,
            },
        );
    }

    pub(crate) fn take_due_at(&mut self, now: Instant) -> Vec<(Uuid, FavoriteViewState)> {
        let due: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, write)| (write.due_at <= now).then_some(*id))
            .collect();
        due.into_iter()
            .filter_map(|id| self.pending.remove(&id).map(|write| (id, write.state)))
            .collect()
    }

    pub(crate) fn take_all(&mut self) -> Vec<(Uuid, FavoriteViewState)> {
        self.pending
            .drain()
            .map(|(id, write)| (id, write.state))
            .collect()
    }

    pub(crate) fn next_due_in_at(&self, now: Instant) -> Option<Duration> {
        self.pending
            .values()
            .map(|write| write.due_at.saturating_duration_since(now))
            .min()
    }

    pub(crate) fn remove(&mut self, id: Uuid) {
        self.pending.remove(&id);
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn continuous_changes_collapse_to_one_write() {
        let id = Uuid::new_v4();
        let start = Instant::now();
        let mut debounce = FavoriteViewWriteDebounce::default();
        let mut state = FavoriteViewState::from_settings(&Settings::default());

        for offset_ms in [0, 100, 200] {
            state.thumb_px += 1;
            debounce.note_at(id, state.clone(), start + Duration::from_millis(offset_ms));
        }

        assert!(
            debounce
                .take_due_at(start + Duration::from_millis(699))
                .is_empty()
        );
        let writes = debounce.take_due_at(start + Duration::from_millis(700));
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, id);
        assert_eq!(writes[0].1.thumb_px, state.thumb_px);
        let temp = tempfile::tempdir().unwrap();
        let db = crate::adjustment_db::AdjustmentDb::open_at(&temp.path().join("adjustment.db"))
            .unwrap();
        let before = db.total_changes_for_test();
        for (write_id, write_state) in writes {
            db.set_favorite_view_state(write_id, &write_state).unwrap();
        }
        assert_eq!(db.total_changes_for_test() - before, 1);
        assert!(debounce.take_all().is_empty());
    }

    #[test]
    fn store_writer_preserves_set_remove_clear_fifo_and_drains_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("adjustment.db");
        let id = Uuid::new_v4();
        let mut state = FavoriteViewState::from_settings(&Settings::default());
        state.thumb_px = 123;

        let writer = FavoriteViewStoreWriter::spawn(path.clone()).unwrap();
        writer
            .submit(FavoriteViewStoreCommand::Set {
                id,
                state: state.clone(),
            })
            .unwrap();
        writer
            .submit(FavoriteViewStoreCommand::Remove { id })
            .unwrap();
        writer
            .submit(FavoriteViewStoreCommand::Set {
                id,
                state: state.clone(),
            })
            .unwrap();
        writer.submit(FavoriteViewStoreCommand::Clear).unwrap();
        drop(writer);

        let db = crate::adjustment_db::AdjustmentDb::open_at(&path).unwrap();
        assert!(db.load_all_favorite_view_states().is_empty());
    }
}
