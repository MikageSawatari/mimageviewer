//! One worker owns supervisor handles. UI submits snapshots and reads short-lived views.
use crate::indexer_supervisor::{self, SupervisorControl, SupervisorHandle, SupervisorParams};
use crate::metadata_ownership::{MetadataOwnership, metadata_ownership, root_key};
use crate::settings::FavoriteEntry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Configuration {
    pub favorites: Vec<FavoriteEntry>,
    pub excluded: Vec<PathBuf>,
    pub ownership: MetadataOwnership,
}
impl Configuration {
    pub fn new(favorites: &[FavoriteEntry], excluded: Vec<PathBuf>) -> Self {
        Self {
            favorites: favorites.to_vec(),
            ownership: metadata_ownership(favorites, &excluded),
            excluded,
        }
    }
    fn signature(&self, id: Uuid, similar: bool) -> Option<(String, bool, bool)> {
        let f = self.favorites.iter().find(|f| f.id == id)?;
        Some((
            self.ownership.favorites[&id].root.clone(),
            self.ownership.favorites[&id].effective_metadata,
            similar && f.auto_index_similar,
        ))
    }
    fn exclusions(&self) -> HashSet<String> {
        self.excluded.iter().map(|p| root_key(p)).collect()
    }
}

pub(crate) struct View {
    pub controls: HashMap<Uuid, SupervisorControl>,
    pub favorites: Vec<FavoriteEntry>,
    pub pending: Option<Configuration>,
    pub busy: bool,
    pub shutdown: Option<Instant>,
    pub must_scan_roots: HashSet<String>,
    pub reconciliation_ms: u64,
    pub failed_cleaned: usize,
    pub notifications: Vec<&'static str>,
}
pub(crate) struct Shared {
    pub state: Mutex<View>,
    pub changed: Condvar,
    #[cfg(test)]
    gate: Mutex<Option<Arc<dyn Fn(&Configuration, &'static str) + Send + Sync>>>,
}
impl Shared {
    fn checkpoint(&self, config: &Configuration, phase: &'static str) {
        #[cfg(test)]
        if let Some(gate) = self.gate.lock().unwrap().clone() {
            gate(config, phase);
        }
        #[cfg(not(test))]
        let _ = (config, phase);
    }
}

pub(crate) struct Runtime {
    pub shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct Stores {
    pub meta: Arc<crate::fts_meta::FtsMetaDb>,
    pub fts: Arc<crate::fts_index::FtsIndex>,
    pub writer: Arc<crate::fts_writer_dispatcher::FtsWriterDispatcher>,
    pub io: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    pub gate: Arc<crate::activity_gate::ActivityGate>,
    pub similar: Option<crate::similar_index::SimilarIndexNotifier>,
}

impl Runtime {
    pub fn start(stores: Stores, initial: Configuration) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(View {
                controls: HashMap::new(),
                favorites: Vec::new(),
                pending: Some(initial),
                busy: true,
                shutdown: None,
                must_scan_roots: HashSet::new(),
                reconciliation_ms: 0,
                failed_cleaned: 0,
                notifications: Vec::new(),
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            gate: Mutex::new(None),
        });
        let shared_worker = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("metadata-reconfigure".into())
            .spawn(move || run(stores, shared_worker))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    pub fn submit(&self, config: Configuration) {
        let mut view = self.shared.state.lock().unwrap();
        if view.shutdown.is_none() {
            view.pending = Some(config);
            self.shared.changed.notify_one();
        }
    }
    fn signal_shutdown(&self) -> Instant {
        let mut view = self.shared.state.lock().unwrap();
        let deadline = *view
            .shutdown
            .get_or_insert_with(|| Instant::now() + Duration::from_secs(4));
        for control in view.controls.values() {
            control.signal_stop();
        }
        self.shared.changed.notify_all();
        deadline
    }
    pub fn shutdown(&mut self) {
        let deadline = self.signal_shutdown();
        if let Some(worker) = self.worker.take() {
            while !worker.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
            // Dropping an unfinished JoinHandle detaches it, including handles owned there.
        }
    }
}

pub(crate) fn cleanup_failure(stores: &Stores, shared: &Shared, error: impl std::fmt::Display) {
    crate::logger::log(format!("metadata cleanup failed: {error}"));
    if let Err(e) = stores.meta.request_item_index_rebuild() {
        crate::logger::log(format!("metadata rebuild marker failed: {e}"));
    }
    shared
        .state
        .lock()
        .unwrap()
        .notifications
        .push("索引の更新に失敗しました。mIV を再起動すると索引を作り直します。");
}

/// Background FIFO puts cleanup behind batches whose cancelled caller abandoned the reply.
fn delete_paths(stores: &Stores, paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    stores
        .writer
        .batch(
            Vec::new(),
            paths.clone(),
            true,
            true,
            crate::fts_writer_dispatcher::WriterPriority::Background,
        )
        .map_err(|e| e.to_string())?;
    stores
        .meta
        .delete_paths(&paths)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Reconciliation must run before any supervisor; orphan IDs are included.
pub(crate) fn startup_cleanup(
    stores: &Stores,
    config: &Configuration,
) -> Result<HashSet<String>, String> {
    let started = Instant::now();
    let rows = stores.meta.list_path_owners().map_err(|e| e.to_string())?;
    let mut roots = HashSet::new();
    let mut paths = Vec::new();
    for (path, id, _) in rows {
        let valid = Uuid::parse_str(&id)
            .ok()
            .and_then(|id| config.ownership.favorites.get(&id))
            .and_then(|f| f.owned_range.as_ref())
            .is_some_and(|r| r.contains(&path));
        if !valid {
            if let Some(owner) = config.ownership.owner(&path) {
                roots.insert(owner.root.clone());
            }
            paths.push(path);
        }
    }
    let count = paths.len();
    let result = delete_paths(stores, paths);
    crate::perf::event(
        "startup",
        "metadata_out_of_range_cleanup",
        None,
        0,
        &[
            ("rows", count.into()),
            (
                "ms",
                started.elapsed().as_secs_f64().mul_add(1000.0, 0.0).into(),
            ),
            ("success", result.is_ok().into()),
        ],
    );
    result?;
    Ok(roots)
}

fn run(stores: Stores, shared: Arc<Shared>) {
    let mut handles: HashMap<Uuid, SupervisorHandle> = HashMap::new();
    let mut old = Configuration::new(&[], Vec::new());
    let mut startup = true;
    loop {
        let next = {
            let mut view = shared.state.lock().unwrap();
            while view.pending.is_none() && view.shutdown.is_none() {
                view = shared.changed.wait(view).unwrap();
            }
            if view.shutdown.is_some() {
                break;
            }
            view.busy = true;
            let config = view.pending.take().unwrap();
            view.favorites = config.favorites.clone();
            config
        };
        shared.checkpoint(&next, "snapshot");
        if startup {
            let t = Instant::now();
            match startup_cleanup(&stores, &next) {
                Ok(roots) => shared.state.lock().unwrap().must_scan_roots.extend(roots),
                Err(e) => cleanup_failure(&stores, &shared, e),
            }
            match crate::indexer_manager::run_reconciliation_via_dispatcher(
                &stores.meta,
                &stores.fts,
                &stores.writer,
                &next.favorites,
            ) {
                Ok(report) => shared.state.lock().unwrap().failed_cleaned = report.failed_cleaned,
                Err(e) => cleanup_failure(&stores, &shared, e),
            }
            shared.state.lock().unwrap().reconciliation_ms = t.elapsed().as_millis() as u64;
            crate::perf::emit_ms("startup", "fts_reconciliation", 0, t);
            startup = false;
        }
        let ids: HashSet<_> = old
            .ownership
            .favorites
            .keys()
            .chain(next.ownership.favorites.keys())
            .copied()
            .collect();
        let changed = ids
            .into_iter()
            .filter(|id| {
                old.signature(*id, stores.similar.is_some())
                    != next.signature(*id, stores.similar.is_some())
            })
            .collect();
        let group = old.ownership.overlap_group(
            &next.ownership,
            &changed,
            old.exclusions() != next.exclusions(),
        );
        for id in &group {
            if let Some(h) = handles.get(id) {
                h.signal_stop();
            }
        }
        for id in &group {
            if let Some(h) = handles.remove(id) {
                shared.checkpoint(&next, "joining");
                h.join();
            }
            shared.state.lock().unwrap().controls.remove(id);
        }
        shared.checkpoint(&next, "joined");
        if shared.state.lock().unwrap().shutdown.is_some() {
            break;
        }
        for id in &group {
            let previous = old.ownership.favorites.get(id);
            let current = next.ownership.favorites.get(id);
            let purge = previous.is_some_and(|p| {
                current.is_none_or(|n| {
                    p.root != n.root || (p.effective_metadata && !n.effective_metadata)
                })
            });
            let result = if purge {
                stores
                    .writer
                    .purge_favorite(
                        *id,
                        crate::fts_writer_dispatcher::WriterPriority::Background,
                    )
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        stores
                            .meta
                            .delete_all_for_favorite(*id)
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    })
            } else {
                stores
                    .meta
                    .list_paths_outside_range(*id, current.and_then(|c| c.owned_range.as_ref()))
                    .map_err(|e| e.to_string())
                    .and_then(|paths| delete_paths(&stores, paths))
            };
            if let Err(e) = result {
                cleanup_failure(&stores, &shared, e);
            }
            if shared.state.lock().unwrap().shutdown.is_some() {
                break;
            }
        }
        for favorite in &next.favorites {
            if !group.contains(&favorite.id) || handles.contains_key(&favorite.id) {
                continue;
            }
            let owned = &next.ownership.favorites[&favorite.id];
            let similar_enabled = stores.similar.is_some() && favorite.auto_index_similar;
            if !owned.effective_metadata && !similar_enabled {
                continue;
            }
            // Shutdown and spawn adoption are serialized by this short state lock.
            let mut view = shared.state.lock().unwrap();
            if view.shutdown.is_some() {
                break;
            }
            let h = indexer_supervisor::spawn(
                SupervisorParams {
                    favorite_id: favorite.id,
                    favorite_root: favorite.path.clone(),
                    excluded_roots: owned.excluded_roots.clone(),
                    enable_metadata_index: owned.effective_metadata,
                    similar_notifier: if similar_enabled {
                        stores.similar.clone()
                    } else {
                        None
                    },
                },
                Arc::clone(&stores.meta),
                Arc::clone(&stores.fts),
                Arc::clone(&stores.writer),
                Arc::clone(&stores.io),
                Arc::clone(&stores.gate),
            );
            view.controls.insert(favorite.id, h.control());
            handles.insert(favorite.id, h);
        }
        old = next;
        let mut view = shared.state.lock().unwrap();
        view.busy = false;
        shared.changed.notify_all();
    }
    let deadline = shared
        .state
        .lock()
        .unwrap()
        .shutdown
        .unwrap_or_else(Instant::now);
    for h in handles.values() {
        h.signal_stop();
    }
    for (_, h) in handles {
        h.join_until(deadline);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    fn favorite(id: u128, path: PathBuf, on: bool) -> FavoriteEntry {
        FavoriteEntry {
            id: Uuid::from_u128(id),
            name: id.to_string(),
            path,
            auto_index_metadata: on,
            auto_index_similar: false,
            auto_index_structure: false,
            auto_index_thumbs: false,
        }
    }
    fn stores(tmp: &std::path::Path) -> Stores {
        let meta = Arc::new(crate::fts_meta::FtsMetaDb::open_at(&tmp.join("meta.db")).unwrap());
        let fts = Arc::new(crate::fts_index::FtsIndex::open_at(&tmp.join("fts")).unwrap());
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            Arc::clone(&fts),
        );
        Stores {
            meta,
            fts,
            writer,
            io: Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1)),
            gate: Arc::new(crate::activity_gate::ActivityGate::new(0)),
            similar: None,
        }
    }
    fn settled(runtime: &Runtime) {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut v = runtime.shared.state.lock().unwrap();
        while v.busy || v.pending.is_some() {
            let (next, _) = runtime
                .shared
                .changed
                .wait_timeout(v, Duration::from_secs(1))
                .unwrap();
            v = next;
            assert!(Instant::now() < deadline, "reconfiguration stuck");
        }
    }
    fn initial_done(runtime: &Runtime) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let ready = runtime
                .shared
                .state
                .lock()
                .unwrap()
                .controls
                .values()
                .all(|c| c.snapshot_stats().initial_scan_done);
            if ready {
                return;
            }
            assert!(Instant::now() < deadline, "initial scan stuck");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn has_doc(stores: &Stores, path: &str) -> bool {
        crate::fts_index::find_doc_by_path(&stores.fts.searcher(), stores.fts.fields(), path)
            .unwrap()
            .is_some()
    }
    #[test]
    fn fixed_snapshot_coalesces_later_requests_and_join_precedes_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let mut runtime = Runtime::start(stores, Configuration::new(&[], Vec::new())).unwrap();
        settled(&runtime);
        let (entered, rx) = mpsc::channel();
        let (release, wait) = crossbeam_channel::bounded(0);
        *runtime.shared.gate.lock().unwrap() = Some(Arc::new(move |c, phase| {
            if phase == "joined" {
                entered
                    .send(c.favorites.first().map(|f| f.name.clone()))
                    .unwrap();
                wait.recv().unwrap();
            }
        }));
        let first = favorite(1, tmp.path().join("first"), true);
        std::fs::create_dir(&first.path).unwrap();
        runtime.submit(Configuration::new(&[first.clone()], Vec::new()));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(60)).unwrap(),
            Some("1".into())
        );
        assert!(runtime.shared.state.lock().unwrap().controls.is_empty());
        let middle = favorite(2, tmp.path().join("middle"), true);
        let last = favorite(3, tmp.path().join("last"), true);
        std::fs::create_dir(&last.path).unwrap();
        runtime.submit(Configuration::new(&[middle], Vec::new()));
        runtime.submit(Configuration::new(&[last.clone()], Vec::new()));
        release.send(()).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(60)).unwrap(),
            Some("3".into())
        );
        assert!(runtime.shared.state.lock().unwrap().controls.is_empty());
        release.send(()).unwrap();
        settled(&runtime);
        assert!(
            runtime
                .shared
                .state
                .lock()
                .unwrap()
                .controls
                .contains_key(&last.id)
        );
        *runtime.shared.gate.lock().unwrap() = None;
        runtime.shutdown();
    }
    #[test]
    fn shutdown_during_worker_owned_join_cannot_respawn() {
        let tmp = tempfile::tempdir().unwrap();
        let old = favorite(1, tmp.path().join("old"), true);
        std::fs::create_dir(&old.path).unwrap();
        let mut runtime = Runtime::start(
            stores(tmp.path()),
            Configuration::new(&[old.clone()], Vec::new()),
        )
        .unwrap();
        settled(&runtime);
        let control = runtime.shared.state.lock().unwrap().controls[&old.id].clone();
        let (entered, rx) = mpsc::channel();
        let (release, wait) = crossbeam_channel::bounded(0);
        *runtime.shared.gate.lock().unwrap() = Some(Arc::new(move |_, phase| {
            if phase == "joining" {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            }
        }));
        runtime.submit(Configuration::new(
            &[favorite(1, tmp.path().join("x"), true)],
            Vec::new(),
        ));
        rx.recv_timeout(Duration::from_secs(60)).unwrap();
        assert!(control.same_instance(&runtime.shared.state.lock().unwrap().controls[&old.id]));
        runtime.signal_shutdown();
        assert!(control.is_cancelled());
        release.send(()).unwrap();
        runtime.shutdown();
        assert!(runtime.shared.state.lock().unwrap().controls.is_empty());
    }
    #[test]
    fn startup_cleanup_returns_current_owner_roots_and_removes_orphans() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let outer = favorite(1, PathBuf::from("c:/photos"), true);
        let inner = favorite(2, PathBuf::from("c:/photos/inner"), true);
        for (p, id) in [
            ("c:/photos/inner/x.jpg", outer.id),
            ("d:/orphan.jpg", Uuid::new_v4()),
        ] {
            stores
                .meta
                .upsert_meta_ok(
                    p,
                    id,
                    std::path::Path::new("c:/photos"),
                    crate::fts_index::IndexKind::Image,
                    1,
                    1,
                )
                .unwrap();
        }
        let config = Configuration::new(&[outer, inner], Vec::new());
        assert_eq!(
            startup_cleanup(&stores, &config).unwrap(),
            HashSet::from(["c:/photos/inner".into()])
        );
        assert!(stores.meta.list_path_owners().unwrap().is_empty());
    }
    #[test]
    fn cleanup_failure_sets_rebuild_pending_and_notifies_without_runtime_wipe() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        stores
            .meta
            .upsert_meta_ok(
                "c:/x.jpg",
                Uuid::new_v4(),
                std::path::Path::new("c:/"),
                crate::fts_index::IndexKind::Image,
                1,
                1,
            )
            .unwrap();
        let mut runtime =
            Runtime::start(stores.clone(), Configuration::new(&[], Vec::new())).unwrap();
        settled(&runtime);
        stores
            .meta
            .upsert_meta_ok(
                "c:/retained.jpg",
                Uuid::new_v4(),
                std::path::Path::new("c:/"),
                crate::fts_index::IndexKind::Image,
                1,
                1,
            )
            .unwrap();
        cleanup_failure(&stores, &runtime.shared, "injected cleanup failure");
        assert!(stores.meta.tantivy_rebuild_pending().unwrap());
        assert!(stores.meta.get("c:/retained.jpg").unwrap().is_some());
        assert_eq!(runtime.shared.state.lock().unwrap().notifications.len(), 1);
        runtime.shutdown();
    }
    #[test]
    fn overlap_edits_restart_only_the_group_and_common_exclusions_restart_all() {
        let tmp = tempfile::tempdir().unwrap();
        let outer = favorite(1, tmp.path().join("outer"), true);
        let unrelated = favorite(2, tmp.path().join("other"), true);
        let inner = favorite(3, outer.path.join("inner"), true);
        for f in [&outer, &unrelated, &inner] {
            std::fs::create_dir_all(&f.path).unwrap();
        }
        let mut runtime = Runtime::start(
            stores(tmp.path()),
            Configuration::new(&[outer.clone(), unrelated.clone()], Vec::new()),
        )
        .unwrap();
        settled(&runtime);
        let before = runtime.shared.state.lock().unwrap().controls.clone();
        runtime.submit(Configuration::new(
            &[outer.clone(), unrelated.clone(), inner.clone()],
            Vec::new(),
        ));
        settled(&runtime);
        let added = runtime.shared.state.lock().unwrap().controls.clone();
        assert!(!before[&outer.id].same_instance(&added[&outer.id]));
        assert!(before[&unrelated.id].same_instance(&added[&unrelated.id]));
        let mut off = inner.clone();
        off.auto_index_metadata = false;
        runtime.submit(Configuration::new(
            &[outer.clone(), unrelated.clone(), off],
            Vec::new(),
        ));
        settled(&runtime);
        let disabled = runtime.shared.state.lock().unwrap().controls.clone();
        assert!(!added[&outer.id].same_instance(&disabled[&outer.id]));
        assert!(added[&unrelated.id].same_instance(&disabled[&unrelated.id]));
        runtime.submit(Configuration::new(
            &[outer.clone(), unrelated.clone()],
            Vec::new(),
        ));
        settled(&runtime);
        let removed = runtime.shared.state.lock().unwrap().controls.clone();
        assert!(!disabled[&outer.id].same_instance(&removed[&outer.id]));
        assert!(disabled[&unrelated.id].same_instance(&removed[&unrelated.id]));
        runtime.submit(Configuration::new(
            &[outer.clone(), unrelated.clone()],
            vec![outer.path.join("private")],
        ));
        settled(&runtime);
        let excluded = runtime.shared.state.lock().unwrap().controls.clone();
        assert!(!removed[&unrelated.id].same_instance(&excluded[&unrelated.id]));
        runtime.shutdown();
    }
    #[test]
    fn smaller_uuid_takes_duplicate_root_and_reorder_rename_preserve_controls() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("photos");
        std::fs::create_dir(&root).unwrap();
        let loser = favorite(9, root.clone(), true);
        let winner = favorite(1, root, true);
        let mut runtime = Runtime::start(
            stores(tmp.path()),
            Configuration::new(&[loser.clone()], Vec::new()),
        )
        .unwrap();
        settled(&runtime);
        runtime.submit(Configuration::new(
            &[loser.clone(), winner.clone()],
            Vec::new(),
        ));
        settled(&runtime);
        let before = runtime.shared.state.lock().unwrap().controls.clone();
        assert!(!before.contains_key(&loser.id));
        assert!(before.contains_key(&winner.id));
        let mut renamed = winner.clone();
        renamed.name = "renamed".into();
        runtime.submit(Configuration::new(&[renamed, loser], Vec::new()));
        settled(&runtime);
        assert!(
            before[&winner.id]
                .same_instance(&runtime.shared.state.lock().unwrap().controls[&winner.id])
        );
        runtime.shutdown();
    }
    #[test]
    fn sqlite_cleanup_failure_still_respawns_and_marks_next_start_rebuild() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let root = tmp.path().join("photos");
        std::fs::create_dir(&root).unwrap();
        let on = favorite(1, root.clone(), true);
        let mut runtime = Runtime::start(
            stores.clone(),
            Configuration::new(&[on.clone()], Vec::new()),
        )
        .unwrap();
        settled(&runtime);
        let excluded = root.join("private");
        let key = crate::search_index_db::normalize_path(&excluded.join("old.jpg"));
        stores
            .meta
            .upsert_meta_ok(&key, on.id, &root, crate::fts_index::IndexKind::Image, 1, 1)
            .unwrap();
        let sql = rusqlite::Connection::open(tmp.path().join("meta.db")).unwrap();
        sql.execute_batch("CREATE TRIGGER fail_cleanup BEFORE DELETE ON files BEGIN SELECT RAISE(FAIL, 'injected cleanup failure'); END;").unwrap();
        runtime.submit(Configuration::new(&[on.clone()], vec![excluded]));
        settled(&runtime);
        assert!(
            runtime
                .shared
                .state
                .lock()
                .unwrap()
                .controls
                .contains_key(&on.id)
        );
        assert!(
            !runtime
                .shared
                .state
                .lock()
                .unwrap()
                .notifications
                .is_empty()
        );
        assert!(stores.meta.tantivy_rebuild_pending().unwrap());
        runtime.shutdown();
    }
    #[test]
    fn root_change_purges_tantivy_only_old_root_document_through_manager() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let mut f = favorite(1, tmp.path().join("old"), true);
        std::fs::create_dir(&f.path).unwrap();
        let mut runtime =
            Runtime::start(stores.clone(), Configuration::new(&[f.clone()], Vec::new())).unwrap();
        settled(&runtime);
        initial_done(&runtime);
        let key = crate::search_index_db::normalize_path(&f.path.join("ghost.jpg"));
        let doc = crate::fts_index::IndexDoc {
            path: key.clone(),
            container: crate::fts_index::Container::Fs,
            zip_entry: String::new(),
            favorite_id: f.id,
            kind: crate::fts_index::IndexKind::Image,
            mtime: 1,
            file_size: 1,
            norms: Default::default(),
        };
        stores
            .writer
            .batch(
                vec![doc],
                Vec::new(),
                true,
                true,
                crate::fts_writer_dispatcher::WriterPriority::Background,
            )
            .unwrap();
        assert!(has_doc(&stores, &key));
        assert!(stores.meta.get(&key).unwrap().is_none());
        f.path = tmp.path().join("new");
        std::fs::create_dir(&f.path).unwrap();
        runtime.submit(Configuration::new(&[f], Vec::new()));
        settled(&runtime);
        assert!(!has_doc(&stores, &key));
        runtime.shutdown();
    }
    #[test]
    fn broadened_common_exclusion_removes_sqlite_and_tantivy_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let f = favorite(1, tmp.path().join("photos"), true);
        let excluded = f.path.join("private");
        std::fs::create_dir_all(&excluded).unwrap();
        let path = excluded.join("old.png");
        image::RgbImage::new(2, 2).save(&path).unwrap();
        let key = crate::search_index_db::normalize_path(&path);
        let mut runtime =
            Runtime::start(stores.clone(), Configuration::new(&[f.clone()], Vec::new())).unwrap();
        settled(&runtime);
        initial_done(&runtime);
        assert!(stores.meta.get(&key).unwrap().is_some());
        assert!(has_doc(&stores, &key));
        runtime.submit(Configuration::new(&[f], vec![excluded]));
        settled(&runtime);
        initial_done(&runtime);
        assert!(path.exists());
        assert!(stores.meta.get(&key).unwrap().is_none());
        assert!(!has_doc(&stores, &key));
        runtime.shutdown();
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}
