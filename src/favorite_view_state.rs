use std::collections::{HashMap, VecDeque};
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
pub(crate) enum FavoriteViewStoreResult {
    Persisted {
        command: FavoriteViewStoreCommand,
        result: Result<usize, String>,
    },
    /// A copy publishes a new UUID's saved state for adoption by the App owner.
    /// Ordinary writes already changed that owner's memory before submission.
    Copied { id: Uuid, state: FavoriteViewState },
}

enum WorkerMessage {
    FolderZip(
        Vec<(Uuid, Uuid)>,
        crate::image_folder_zip_metadata::CopyPhase,
        std::sync::mpsc::Sender<crate::image_folder_zip_metadata::CopyReply>,
    ),
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
    pub(crate) fn folder_zip_portal(
        &self,
        pairs: Vec<(Uuid, Uuid)>,
    ) -> crate::image_folder_zip_metadata::CopyPortal {
        let tx = self.tx.clone();
        let submitted = Arc::clone(&self.submitted);
        Arc::new(move |_, phase| {
            let (reply, rx) = std::sync::mpsc::channel();
            submitted.fetch_add(1, Ordering::Release);
            if tx
                .send(WorkerMessage::FolderZip(pairs.clone(), phase, reply))
                .is_err()
            {
                submitted.fetch_sub(1, Ordering::Release);
            }
            rx
        })
    }
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
        if let WorkerMessage::FolderZip(pairs, phase, reply) = message {
            let result = (|| {
                if db.is_none() {
                    db = Some(
                        crate::adjustment_db::AdjustmentDb::open_at(&db_path)
                            .map_err(|e| e.to_string())?,
                    );
                }
                let db = db.as_ref().unwrap();
                let mut count = 0;
                for (old, new) in pairs {
                    count += db.copy_favorite_view_for_zip(old, new, phase)?;
                    if phase == crate::image_folder_zip_metadata::CopyPhase::Apply
                        && let Some(state) = db.favorite_view_for_copy(new)?
                    {
                        let _ = result_tx.send(FavoriteViewStoreResult::Copied { id: new, state });
                    }
                }
                Ok(count)
            })();
            done.fetch_add(1, Ordering::Release);
            let _ = reply.send(result);
            continue;
        }
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
        let _ = result_tx.send(FavoriteViewStoreResult::Persisted { command, result });
        done.fetch_add(1, Ordering::Release);
    }
}

#[derive(Clone)]
struct PendingWrite {
    state: FavoriteViewState,
    due_at: Instant,
}

#[derive(Clone)]
struct DeferredCommand {
    command: FavoriteViewStoreCommand,
    due_at: Instant,
}

/// お気に入り表示状態の DB 書き込みを UUID ごとにまとめる。
///
/// メモリ上の正本は即時更新し、永続化だけを最後の変更から 500ms 後へ遅延する。
#[derive(Default)]
pub(crate) struct FavoriteViewWriteDebounce {
    pending: HashMap<Uuid, PendingWrite>,
    deferred: VecDeque<DeferredCommand>,
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

    /// Return a sidecar-restore write intent to the normal owner without
    /// resubmitting it inside the restore modal. Non-coalescible Remove/Clear
    /// commands stay FIFO ordered.
    pub(crate) fn defer_command_at(&mut self, command: FavoriteViewStoreCommand, now: Instant) {
        self.deferred.push_back(DeferredCommand {
            command,
            due_at: now + FAVORITE_VIEW_WRITE_DEBOUNCE,
        });
    }

    pub(crate) fn take_due_deferred_at(&mut self, now: Instant) -> Vec<FavoriteViewStoreCommand> {
        let mut due = Vec::new();
        while self
            .deferred
            .front()
            .is_some_and(|command| command.due_at <= now)
        {
            due.push(
                self.deferred
                    .pop_front()
                    .expect("front was checked")
                    .command,
            );
        }
        due
    }

    pub(crate) fn take_all_deferred(&mut self) -> Vec<FavoriteViewStoreCommand> {
        self.deferred
            .drain(..)
            .map(|deferred| deferred.command)
            .collect()
    }

    pub(crate) fn restore_deferred_front(
        &mut self,
        commands: Vec<FavoriteViewStoreCommand>,
        now: Instant,
    ) {
        let due_at = now + FAVORITE_VIEW_WRITE_DEBOUNCE;
        for command in commands.into_iter().rev() {
            self.deferred
                .push_front(DeferredCommand { command, due_at });
        }
    }

    pub(crate) fn next_due_in_at(&self, now: Instant) -> Option<Duration> {
        self.pending
            .values()
            .map(|write| write.due_at.saturating_duration_since(now))
            .chain(
                self.deferred
                    .front()
                    .map(|command| command.due_at.saturating_duration_since(now)),
            )
            .min()
    }

    pub(crate) fn remove(&mut self, id: Uuid) {
        self.pending.remove(&id);
        self.deferred.retain(|deferred| {
            !matches!(
                deferred.command,
                FavoriteViewStoreCommand::Set { id: command_id, .. }
                    | FavoriteViewStoreCommand::Remove { id: command_id }
                    if command_id == id
            )
        });
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.deferred.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn folder_zip_manifest(
        temp: &tempfile::TempDir,
    ) -> crate::image_folder_zip::ImageFolderManifest {
        let source = temp.path().join("book");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("p1.jpg"), b"image bytes").unwrap();
        crate::image_folder_zip::prepare_manifest(
            &source,
            None,
            &crate::image_folder_zip::ConversionControl::default(),
        )
        .unwrap()
    }

    #[test]
    fn folder_zip_favorite_flush_copies_all_fields_and_inheritance_to_fresh_uuids() {
        use crate::image_folder_zip_metadata::CopyPhase;
        use crate::settings::{
            FullscreenSpecifiedScale, GridItemDisplayKind, GridViewMode, ReadingFlow, SortOrder,
            SpecifiedScaleAxis, SpecifiedScaleAxisSetting, SpreadMode, ThumbAspect,
        };
        let temp = tempfile::tempdir().unwrap();
        let manifest = folder_zip_manifest(&temp);
        let path = temp.path().join("adjustment.db");
        let writer = FavoriteViewStoreWriter::spawn(path.clone()).unwrap();
        let mut debounce = FavoriteViewWriteDebounce::default();
        let now = Instant::now();
        let enabled = FullscreenSpecifiedScale {
            width: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::PercentTenths(1250)),
            height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::LogicalPixels(720)),
        };
        let disabled = FullscreenSpecifiedScale {
            width: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(1750)),
            height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::LogicalPixels(900)),
        };
        let mut states = Vec::new();
        let mut pairs = Vec::new();
        for (index, (mode, scale)) in [
            (SpreadMode::Single, None),
            (SpreadMode::RtlCover, Some(disabled)),
            (SpreadMode::SplitRtl, Some(enabled)),
        ]
        .into_iter()
        .enumerate()
        {
            let old = Uuid::new_v4();
            let new = Uuid::new_v4();
            let mut state = FavoriteViewState::from_settings(&Settings::default());
            state.grid_view_mode = GridViewMode::Details;
            state.grid_cols = 3 + index;
            state.thumb_aspect = ThumbAspect::Portrait2x3;
            state.thumb_aspect_auto = false;
            state
                .grid_display_order
                .assign(GridItemDisplayKind::Image, 3);
            state.sort_order = SortOrder::NumericDesc;
            state.default_spread_mode = mode;
            state.default_reading_flow = ReadingFlow::Horizontal;
            state.fullscreen_specified_scale = scale;
            debounce.note_at(old, state.clone(), now);
            states.push((old, new, state));
            pairs.push((old, new));
        }
        let missing = (Uuid::new_v4(), Uuid::new_v4());
        pairs.push(missing);
        assert!(
            debounce.take_due_at(now).is_empty(),
            "debounce has not reached 500ms"
        );
        for (id, state) in debounce.take_all() {
            writer
                .submit(FavoriteViewStoreCommand::Set { id, state })
                .unwrap();
        }
        let sibling = Uuid::new_v4();
        let sibling_state = FavoriteViewState::from_settings(&Settings::default());
        writer
            .submit(FavoriteViewStoreCommand::Set {
                id: sibling,
                state: sibling_state.clone(),
            })
            .unwrap();
        let portal = writer.folder_zip_portal(pairs);
        assert_eq!(
            portal(&manifest, CopyPhase::Check)
                .recv_timeout(Duration::from_secs(10))
                .unwrap(),
            Ok(3)
        );
        let db = crate::adjustment_db::AdjustmentDb::open_at(&path).unwrap();
        let checked = db.load_all_favorite_view_states();
        assert!(states.iter().all(|(_, new, _)| !checked.contains_key(new)));
        assert_eq!(
            portal(&manifest, CopyPhase::Apply)
                .recv_timeout(Duration::from_secs(10))
                .unwrap(),
            Ok(3)
        );
        // The regular result consumer receives new UUID states before copy completion.
        let mut adopted = HashMap::new();
        let mut copies = 0;
        for _ in 0..7 {
            let result = writer
                .result_rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            match result {
                FavoriteViewStoreResult::Persisted {
                    command: FavoriteViewStoreCommand::Set { id, state },
                    result,
                } => {
                    assert_eq!(result, Ok(1));
                    adopted.insert(id, state);
                }
                FavoriteViewStoreResult::Copied { id, state } => {
                    assert!(states.iter().any(|(_, new, _)| *new == id));
                    copies += 1;
                    adopted.insert(id, state);
                }
                result => panic!("unexpected {result:?}"),
            }
        }
        assert_eq!(copies, 3);
        assert!(!writer.is_busy());
        drop(portal);
        drop(writer);
        let persisted = db.load_all_favorite_view_states();
        for (old, new, state) in states {
            assert_eq!(persisted[&old], state);
            assert_eq!(persisted[&new], state);
            assert_eq!(adopted[&new], state);
            let mut settings = Settings::default();
            let inherited_scale = settings.common_fullscreen_specified_scale();
            persisted[&new].apply_to_settings(&mut settings);
            assert_eq!(settings.grid_view_mode, state.grid_view_mode);
            assert_eq!(settings.grid_cols, state.grid_cols);
            assert_eq!(settings.thumb_aspect, state.thumb_aspect);
            assert_eq!(settings.thumb_aspect_auto, state.thumb_aspect_auto);
            assert_eq!(settings.grid_display_order, state.grid_display_order);
            assert_eq!(settings.sort_order, state.sort_order);
            assert_eq!(settings.default_spread_mode, state.default_spread_mode);
            assert_eq!(settings.default_reading_flow, state.default_reading_flow);
            assert_eq!(
                settings.fullscreen_specified_scale,
                state.fullscreen_specified_scale.unwrap_or(inherited_scale)
            );
        }
        assert_eq!(persisted[&sibling], sibling_state);
        assert_eq!(adopted[&sibling], sibling_state);
        assert!(!persisted.contains_key(&missing.0));
        assert!(!persisted.contains_key(&missing.1));
        assert!(!adopted.contains_key(&missing.1));
    }

    #[test]
    fn folder_zip_favorite_fifo_remove_and_clear_do_not_resurrect_source_state() {
        use crate::image_folder_zip_metadata::CopyPhase;
        let temp = tempfile::tempdir().unwrap();
        let manifest = folder_zip_manifest(&temp);
        let path = temp.path().join("adjustment.db");
        let writer = FavoriteViewStoreWriter::spawn(path.clone()).unwrap();
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        let mut debounce = FavoriteViewWriteDebounce::default();
        let now = Instant::now();
        for command in [
            FavoriteViewStoreCommand::Set {
                id: old,
                state: FavoriteViewState::from_settings(&Settings::default()),
            },
            FavoriteViewStoreCommand::Remove { id: old },
            FavoriteViewStoreCommand::Set {
                id: old,
                state: FavoriteViewState::from_settings(&Settings::default()),
            },
            FavoriteViewStoreCommand::Clear,
        ] {
            debounce.defer_command_at(command, now);
        }
        assert!(debounce.take_due_deferred_at(now).is_empty());
        for command in debounce.take_all_deferred() {
            writer.submit(command).unwrap();
        }
        let portal = writer.folder_zip_portal(vec![(old, new)]);
        assert_eq!(
            portal(&manifest, CopyPhase::Check)
                .recv_timeout(Duration::from_secs(10))
                .unwrap(),
            Ok(0)
        );
        assert_eq!(
            portal(&manifest, CopyPhase::Apply)
                .recv_timeout(Duration::from_secs(10))
                .unwrap(),
            Ok(0)
        );
        for _ in 0..4 {
            let result = writer
                .result_rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            assert!(matches!(
                result,
                FavoriteViewStoreResult::Persisted { result: Ok(_), .. }
            ));
        }
        assert!(
            writer.try_recv().is_none(),
            "absent source must not publish a copied state"
        );
        assert!(!writer.is_busy());
        drop(portal);
        drop(writer);
        let db = crate::adjustment_db::AdjustmentDb::open_at(&path).unwrap();
        assert!(db.load_all_favorite_view_states().is_empty());
    }

    #[test]
    fn continuous_changes_collapse_to_one_write() {
        let id = Uuid::new_v4();
        let start = Instant::now();
        let mut debounce = FavoriteViewWriteDebounce::default();
        let mut state = FavoriteViewState::from_settings(&Settings::default());

        for offset_ms in [0, 100, 200] {
            state.grid_cols += 1;
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
        assert_eq!(writes[0].1.grid_cols, state.grid_cols);
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
        state.grid_cols = 5;

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

    #[test]
    fn sidecar_restore_barrier_observes_published_result_after_done() {
        let temp = tempfile::tempdir().unwrap();
        let writer = FavoriteViewStoreWriter::spawn(temp.path().join("adjustment.db")).unwrap();
        writer.submit(FavoriteViewStoreCommand::Clear).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while writer.done.load(Ordering::Acquire) != writer.submitted.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "favorite worker did not finish");
            std::thread::yield_now();
        }

        // `is_busy` remains true after DB completion until the already-published result is
        // consumed.  Quiescence therefore samples busy before draining the result queue.
        assert!(writer.is_busy());
        let result = writer.try_recv().expect("done publishes a result first");
        assert!(matches!(
            result,
            FavoriteViewStoreResult::Persisted { result: Ok(_), .. }
        ));
        assert!(!writer.is_busy());
    }
}
