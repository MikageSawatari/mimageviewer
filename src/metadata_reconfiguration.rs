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
    pub similar_passwords: Option<crate::pdf_passwords::PdfPasswordStore>,
    pub skip_offline_change_scan: bool,
}
impl Configuration {
    pub fn new(favorites: &[FavoriteEntry], excluded: Vec<PathBuf>) -> Self {
        Self {
            favorites: favorites.to_vec(),
            ownership: metadata_ownership(favorites, &excluded),
            excluded,
            similar_passwords: None,
            skip_offline_change_scan: false,
        }
    }
    fn signature(&self, id: Uuid, similar: bool) -> Option<(String, bool, Option<String>)> {
        let f = self.favorites.iter().find(|f| f.id == id)?;
        Some((
            self.ownership.favorites[&id].root.clone(),
            self.ownership.favorites[&id].effective_metadata,
            // Similar's persisted root key preserves a trailing separator. Re-register its
            // watcher whenever that key changes, even if metadata owns the same path range.
            (similar && f.auto_index_similar)
                .then(|| crate::search_index_db::normalize_path(&f.path)),
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
    full_check_pending: bool,
    #[cfg(test)]
    pub(crate) full_check_dispatched: usize,
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
        Self::start_inner(
            stores,
            initial,
            #[cfg(test)]
            None,
        )
    }
    fn start_inner(
        stores: Stores,
        initial: Configuration,
        #[cfg(test)] gate: Option<Arc<dyn Fn(&Configuration, &'static str) + Send + Sync>>,
    ) -> std::io::Result<Self> {
        let startup_skip_offline_change_scan = initial.skip_offline_change_scan;
        #[cfg(test)]
        let first_config = initial.clone();
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
                full_check_pending: false,
                #[cfg(test)]
                full_check_dispatched: 0,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            gate: Mutex::new(gate),
        });
        let shared_worker = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("metadata-reconfigure".into())
            .spawn(move || {
                #[cfg(test)]
                shared_worker.checkpoint(&first_config, "before_snapshot");
                run(stores, shared_worker, startup_skip_offline_change_scan)
            })?;
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
    /// The worker consumes this after supervisor adoption, including requests during init.
    pub fn request_full_check(&self) {
        let mut view = self.shared.state.lock().unwrap();
        if view.shutdown.is_none() {
            view.full_check_pending = true;
            self.shared.changed.notify_one();
        }
    }
    #[cfg(test)]
    pub(crate) fn full_check_requested_for_test(&self) -> bool {
        let view = self.shared.state.lock().unwrap();
        view.full_check_pending || view.full_check_dispatched != 0
    }
    #[cfg(test)]
    pub(crate) fn wait_full_check_dispatched_for_test(&self) {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut view = self.shared.state.lock().unwrap();
        while view.full_check_dispatched == 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "full check was not dispatched");
            view = self.shared.changed.wait_timeout(view, remaining).unwrap().0;
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
fn delete_paths(
    stores: &Stores,
    paths: Vec<String>,
    affected_roots: &[String],
) -> Result<(), String> {
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
        .delete_paths_and_invalidate_scans(&paths, affected_roots)
        .map_err(|e| e.to_string())?;
    Ok(())
}

struct StartupCleanupTargets {
    paths: Vec<String>,
    roots: HashSet<String>,
    owner_rows: usize,
}

/// Retain only invalid rows; the visitor must not re-enter the locked metadata DB.
fn collect_startup_cleanup_targets(
    meta: &crate::fts_meta::FtsMetaDb,
    config: &Configuration,
) -> Result<StartupCleanupTargets, String> {
    let mut roots = HashSet::new();
    let mut paths = Vec::new();
    let owner_rows = meta
        .for_each_path_owner(|path, id| {
            let valid = Uuid::parse_str(id)
                .ok()
                .and_then(|id| config.ownership.favorites.get(&id))
                .and_then(|f| f.owned_range.as_ref())
                .is_some_and(|r| r.contains(path));
            if !valid {
                if let Some(owner) = config.ownership.owner(path) {
                    roots.insert(owner.root.clone());
                }
                paths.push(path.to_owned());
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(StartupCleanupTargets {
        paths,
        roots,
        owner_rows,
    })
}

/// Reconciliation must run before any supervisor; orphan IDs are included.
pub(crate) fn startup_cleanup(
    stores: &Stores,
    config: &Configuration,
) -> Result<HashSet<String>, String> {
    let started = Instant::now();
    let query_started = Instant::now();
    let StartupCleanupTargets {
        paths,
        roots,
        owner_rows,
    } = collect_startup_cleanup_targets(&stores.meta, config)?;
    // Includes the ownership check. Rows and the DB lock are released before deleting.
    let query_ms = query_started.elapsed().as_secs_f64() * 1000.0;
    let count = paths.len();
    let result = delete_paths(stores, paths, &roots.iter().cloned().collect::<Vec<_>>());
    crate::perf::event(
        "startup",
        "metadata_out_of_range_cleanup",
        None,
        0,
        &[
            ("rows", count.into()),
            ("owner_rows", owner_rows.into()),
            ("query_ms", query_ms.into()),
            // The streaming collector only grows, so its final size is its peak.
            ("peak_collected_rows", count.into()),
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

fn dispatch_full_check(stores: &Stores, view: &mut View) {
    for control in view.controls.values() {
        control.request_metadata_full_rescan();
    }
    if let Some(similar) = stores.similar.as_ref() {
        similar.request_full_all();
    }
    #[cfg(test)]
    {
        view.full_check_dispatched += 1;
    }
    view.full_check_pending = false;
}

fn run(stores: Stores, shared: Arc<Shared>, startup_skip_offline_change_scan: bool) {
    let mut handles: HashMap<Uuid, SupervisorHandle> = HashMap::new();
    let mut old = Configuration::new(&[], Vec::new());
    let mut startup = true;
    loop {
        let next = {
            let mut view = shared.state.lock().unwrap();
            while view.pending.is_none() && !view.full_check_pending && view.shutdown.is_none() {
                view = shared.changed.wait(view).unwrap();
            }
            if view.shutdown.is_some() {
                break;
            }
            if view.pending.is_none() {
                dispatch_full_check(&stores, &mut view);
                shared.changed.notify_all();
                continue;
            }
            view.busy = true;
            let config = view.pending.take().unwrap();
            view.favorites = config.favorites.clone();
            config
        };
        let initial_configuration = startup;
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
        if let Some(similar) = stores.similar.as_ref() {
            if let Some(passwords) = next
                .similar_passwords
                .clone()
                .or_else(|| similar.password_snapshot())
            {
                similar.configure(
                    &next.favorites,
                    passwords,
                    Some(Arc::clone(&stores.gate)),
                    next.excluded.clone(),
                );
            }
        }

        let previous_exclusions = old.exclusions();
        let newly_excluded = next
            .exclusions()
            .into_iter()
            .filter(|root| {
                !previous_exclusions
                    .iter()
                    .any(|old| crate::metadata_ownership::contains(old, root))
            })
            .flat_map(|root| {
                crate::metadata_ownership::OwnedRange {
                    root,
                    exclusions: Vec::new(),
                }
                .sql_ranges()
            })
            .collect::<Vec<_>>();
        if !newly_excluded.is_empty() {
            let affected_roots = old
                .ownership
                .favorites
                .values()
                .chain(next.ownership.favorites.values())
                .map(|favorite| favorite.root.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let result = stores
                .writer
                .purge_path_ranges(
                    newly_excluded.clone(),
                    crate::fts_writer_dispatcher::WriterPriority::Background,
                )
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    stores
                        .meta
                        .delete_path_ranges_and_invalidate_scans(&newly_excluded, &affected_roots)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
            if let Err(e) = result {
                cleanup_failure(&stores, &shared, e);
            }
        }
        for id in &group {
            let previous = old.ownership.favorites.get(id);
            let current = next.ownership.favorites.get(id);
            let purge = previous.is_some_and(|p| {
                current.is_none_or(|n| {
                    p.root != n.root || (p.effective_metadata && !n.effective_metadata)
                })
            });
            let affected_roots = previous
                .into_iter()
                .chain(current)
                .map(|favorite| favorite.root.clone())
                .collect::<Vec<_>>();
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
                            .delete_all_for_favorite_and_invalidate_scans(*id, &affected_roots)
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    })
            } else {
                stores
                    .meta
                    .list_paths_outside_range(*id, current.and_then(|c| c.owned_range.as_ref()))
                    .map_err(|e| e.to_string())
                    .and_then(|paths| delete_paths(&stores, paths, &affected_roots))
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
            let must_scan = shared
                .state
                .lock()
                .unwrap()
                .must_scan_roots
                .contains(&owned.root);
            let skip_initial_scan = initial_configuration
                && owned.effective_metadata
                && indexer_supervisor::can_skip_initial_scan(
                    &stores.meta,
                    startup_skip_offline_change_scan,
                    must_scan,
                    &favorite.path,
                    favorite.id,
                    &owned.excluded_roots,
                );
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
                    skip_initial_scan,
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
        if view.full_check_pending && view.pending.is_none() {
            dispatch_full_check(&stores, &mut view);
        }
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
    fn startup_skip_policy_survives_latest_configuration_coalescing() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let f = favorite(1, tmp.path().join("images"), true);
        std::fs::create_dir(&f.path).unwrap();
        let fingerprint = indexer_supervisor::fts_scan_fingerprint(
            &f.path,
            f.id,
            &[],
            &indexer_supervisor::fts_scan_extensions(),
        );
        stores
            .meta
            .mark_scanned_once(&crate::metadata_ownership::root_key(&f.path), &fingerprint)
            .unwrap();
        let mut initial = Configuration::new(&[f.clone()], Vec::new());
        initial.skip_offline_change_scan = true;
        let (entered, ready) = mpsc::channel();
        let (release, wait) = crossbeam_channel::bounded(0);
        let checkpoint = Arc::new(move |_: &Configuration, phase| {
            if phase == "before_snapshot" {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            }
        });
        let mut runtime = Runtime::start_inner(stores, initial, Some(checkpoint)).unwrap();
        ready.recv_timeout(Duration::from_secs(60)).unwrap();
        // App adopts the manager and submits current favorites before the worker takes initial.
        runtime.submit(Configuration::new(&[f.clone()], Vec::new()));
        release.send(()).unwrap();
        settled(&runtime);
        initial_done(&runtime);
        assert!(
            runtime.shared.state.lock().unwrap().controls[&f.id]
                .snapshot_stats()
                .initial_scan_skipped
        );
        runtime.shutdown();
    }

    #[test]
    fn configuration_roundtrip_then_normal_stop_cannot_reuse_deleted_fts_scope() {
        for change in ["off", "root", "exclusions"] {
            let tmp = tempfile::tempdir().unwrap();
            let mut stores = stores(tmp.path());
            let f = favorite(1, tmp.path().join("images"), true);
            let nested = f.path.join("nested");
            std::fs::create_dir_all(&nested).unwrap();
            image::RgbImage::new(1, 1)
                .save(f.path.join("a.png"))
                .unwrap();
            image::RgbImage::new(1, 1)
                .save(nested.join("b.png"))
                .unwrap();
            let mut initial = Configuration::new(&[f.clone()], Vec::new());
            initial.skip_offline_change_scan = true;
            let mut runtime = Runtime::start(stores.clone(), initial.clone()).unwrap();
            settled(&runtime);
            initial_done(&runtime);
            let root = root_key(&f.path);
            assert!(stores.meta.scanned_once(&root).unwrap().is_some());
            // Full を ActivityGate で止め、構成変更の purge だけを確定させる。
            stores.gate.set_paused(true);
            let mut away = f.clone();
            let exclusions = match change {
                "off" => {
                    away.auto_index_metadata = false;
                    vec![]
                }
                "root" => {
                    away.path = tmp.path().join("other");
                    std::fs::create_dir(&away.path).unwrap();
                    vec![]
                }
                _ => vec![nested],
            };
            runtime.submit(Configuration::new(&[away], exclusions));
            settled(&runtime);
            assert_eq!(stores.meta.scanned_once(&root).unwrap(), None, "{change}");
            runtime.submit(initial.clone());
            settled(&runtime);
            assert!(
                !runtime.shared.state.lock().unwrap().controls[&f.id]
                    .snapshot_stats()
                    .initial_scan_done
            );
            runtime.shutdown();
            drop(runtime);
            stores.meta =
                Arc::new(crate::fts_meta::FtsMetaDb::open_at(&tmp.path().join("meta.db")).unwrap());
            assert_eq!(stores.meta.scanned_once(&root).unwrap(), None);
            stores.gate.set_paused(false);
            let mut restarted = Runtime::start(stores.clone(), initial).unwrap();
            settled(&restarted);
            initial_done(&restarted);
            let stats = restarted.shared.state.lock().unwrap().controls[&f.id].snapshot_stats();
            assert!(!stats.initial_scan_skipped, "{change}");
            assert_eq!(
                stats.last_full_outcome,
                Some(indexer_supervisor::FullScanOutcome::Complete)
            );
            assert_eq!(stores.meta.list_path_owners().unwrap().len(), 2);
            restarted.shutdown();
        }
    }

    #[test]
    fn startup_repair_marker_loss_survives_normal_stop_before_full_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let mut stores = stores(tmp.path());
        let f = favorite(1, tmp.path().join("images"), true);
        std::fs::create_dir(&f.path).unwrap();
        let root = root_key(&f.path);
        let old_root = root_key(&tmp.path().join("old"));
        let fingerprint = indexer_supervisor::fts_scan_fingerprint(
            &f.path,
            f.id,
            &[],
            &indexer_supervisor::fts_scan_extensions(),
        );
        stores.meta.mark_scanned_once(&root, &fingerprint).unwrap();
        stores
            .meta
            .mark_scanned_once(&old_root, "old complete")
            .unwrap();
        let path = crate::search_index_db::normalize_path(&f.path.join("wrong-owner.png"));
        stores
            .meta
            .upsert_meta_ok(
                &path,
                Uuid::from_u128(99),
                &tmp.path().join("old"),
                crate::fts_index::IndexKind::Image,
                1,
                1,
            )
            .unwrap();
        stores.gate.set_paused(true);
        let mut initial = Configuration::new(&[f.clone()], vec![]);
        initial.skip_offline_change_scan = true;
        let mut runtime = Runtime::start(stores.clone(), initial.clone()).unwrap();
        settled(&runtime);
        assert!(
            runtime
                .shared
                .state
                .lock()
                .unwrap()
                .must_scan_roots
                .contains(&root)
        );
        assert_eq!(stores.meta.scanned_once(&root).unwrap(), None);
        assert_eq!(stores.meta.scanned_once(&old_root).unwrap(), None);
        runtime.shutdown();
        drop(runtime);
        stores.meta =
            Arc::new(crate::fts_meta::FtsMetaDb::open_at(&tmp.path().join("meta.db")).unwrap());
        stores.gate.set_paused(false);
        let mut restarted = Runtime::start(stores, initial).unwrap();
        settled(&restarted);
        initial_done(&restarted);
        assert!(
            restarted
                .shared
                .state
                .lock()
                .unwrap()
                .must_scan_roots
                .is_empty()
        );
        assert!(
            !restarted.shared.state.lock().unwrap().controls[&f.id]
                .snapshot_stats()
                .initial_scan_skipped
        );
        restarted.shutdown();
    }

    #[test]
    fn shared_full_check_waits_for_first_similar_configuration_adoption() {
        let tmp = tempfile::tempdir().unwrap();
        let similar = crate::similar_index::SimilarIndexManager::new(tmp.path().join("similar"));
        let mut stores = stores(tmp.path());
        stores.similar = Some(similar.notifier());
        stores.gate.set_paused(true);
        let mut f = favorite(1, tmp.path().join("images"), true);
        f.auto_index_similar = true;
        std::fs::create_dir(&f.path).unwrap();
        image::RgbImage::new(2, 2)
            .save(f.path.join("a.png"))
            .unwrap();
        let mut config = Configuration::new(&[f.clone()], Vec::new());
        config.similar_passwords = Some(crate::pdf_passwords::PdfPasswordStore::empty_for_test());
        let (entered, ready) = mpsc::channel();
        let (release, wait) = crossbeam_channel::bounded(0);
        let checkpoint = Arc::new(move |_: &Configuration, phase| {
            if phase == "joined" {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            }
        });
        let mut runtime = Runtime::start_inner(stores.clone(), config, Some(checkpoint)).unwrap();
        ready.recv_timeout(Duration::from_secs(60)).unwrap();
        assert!(similar.notifier().password_snapshot().is_none());
        runtime.request_full_check();
        runtime.request_full_check();
        assert_eq!(similar.full_check_request_count_for_test(), 0);
        release.send(()).unwrap();
        runtime.wait_full_check_dispatched_for_test();
        assert!(similar.notifier().password_snapshot().is_some());
        assert_eq!(similar.full_check_request_count_for_test(), 1);
        *runtime.shared.gate.lock().unwrap() = None;
        stores.gate.set_paused(false);
        settled(&runtime);
        initial_done(&runtime);
        let deadline = Instant::now() + Duration::from_secs(60);
        while !matches!(
            similar.progress(),
            crate::similar_index::IndexProgress::Complete(_)
        ) {
            assert!(
                Instant::now() < deadline,
                "manual similar full did not complete"
            );
            std::thread::yield_now();
        }
        assert!(similar.reconcile_job_counts_for_test().0 >= 1);
        runtime.shutdown();
    }

    #[test]
    fn full_check_before_adoption_coalesces_and_is_accepted_while_paused() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        stores.gate.set_paused(true);
        let mut runtime =
            Runtime::start(stores.clone(), Configuration::new(&[], Vec::new())).unwrap();
        settled(&runtime);
        let (entered, ready) = mpsc::channel();
        let (release, wait) = crossbeam_channel::bounded(0);
        *runtime.shared.gate.lock().unwrap() = Some(Arc::new(move |_, phase| {
            if phase == "joined" {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            }
        }));
        let f = favorite(1, tmp.path().join("images"), true);
        std::fs::create_dir(&f.path).unwrap();
        std::fs::write(f.path.join("a.jpg"), b"image").unwrap();
        runtime.submit(Configuration::new(&[f.clone()], Vec::new()));
        ready.recv_timeout(Duration::from_secs(60)).unwrap();
        runtime.request_full_check();
        runtime.request_full_check();
        assert!(runtime.shared.state.lock().unwrap().controls.is_empty());
        release.send(()).unwrap();
        settled(&runtime);
        {
            let view = runtime.shared.state.lock().unwrap();
            assert_eq!(view.full_check_dispatched, 1);
            assert!(view.controls.contains_key(&f.id));
        }
        assert!(stores.meta.list_path_owners().unwrap().is_empty());
        *runtime.shared.gate.lock().unwrap() = None;
        stores.gate.set_paused(false);
        initial_done(&runtime);
        runtime.shutdown();
        assert!(
            stores
                .meta
                .get(&crate::search_index_db::normalize_path(
                    &f.path.join("a.jpg")
                ))
                .unwrap()
                .is_some()
        );
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
    fn coalesced_similar_off_on_keeps_unrelated_watch_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = favorite(1, tmp.path().join("f"), false);
        f.auto_index_similar = true;
        let mut other = favorite(2, tmp.path().join("other"), true);
        for root in [&f.path, &other.path] {
            std::fs::create_dir_all(root).unwrap();
        }
        let similar = crate::similar_index::SimilarIndexManager::new(tmp.path().join("similar"));
        let notifier = similar.notifier();
        let mut stores = stores(tmp.path());
        stores.similar = Some(notifier.clone());
        let config = |favorites: &[FavoriteEntry]| {
            let mut config = Configuration::new(favorites, Vec::new());
            config.similar_passwords =
                Some(crate::pdf_passwords::PdfPasswordStore::empty_for_test());
            config
        };
        let mut runtime = Runtime::start(stores, config(&[f.clone(), other.clone()])).unwrap();
        settled(&runtime);
        initial_done(&runtime);
        let deadline = Instant::now() + Duration::from_secs(60);
        while !notifier.watch_is_ready_for_test(f.id) {
            assert!(
                Instant::now() < deadline,
                "initial similar watcher not Ready"
            );
            std::thread::yield_now();
        }
        let before = runtime.shared.state.lock().unwrap().controls[&f.id].clone();
        let (entered, rx) = mpsc::channel();
        let (release, wait) = crossbeam_channel::bounded(0);
        let hold_once = Arc::new(std::sync::atomic::AtomicBool::new(true));
        *runtime.shared.gate.lock().unwrap() = Some(Arc::new(move |_, phase| {
            if phase == "joined" && hold_once.swap(false, std::sync::atomic::Ordering::AcqRel) {
                entered.send(()).unwrap();
                wait.recv().unwrap();
            }
        }));
        other.auto_index_metadata = false;
        runtime.submit(config(&[f.clone(), other.clone()]));
        rx.recv_timeout(Duration::from_secs(60)).unwrap();
        let mut off = f.clone();
        off.auto_index_similar = false;
        runtime.submit(config(&[off, other.clone()]));
        runtime.submit(config(&[f.clone(), other]));
        assert!(
            notifier.watch_is_ready_for_test(f.id),
            "pending snapshots must not configure similar"
        );
        release.send(()).unwrap();
        settled(&runtime);
        assert!(before.same_instance(&runtime.shared.state.lock().unwrap().controls[&f.id]));
        assert!(notifier.watch_is_ready_for_test(f.id));
        let mut aliased = runtime.shared.state.lock().unwrap().favorites.clone();
        let changed = aliased
            .iter_mut()
            .find(|favorite| favorite.id == f.id)
            .unwrap();
        changed.path.push("");
        assert_ne!(
            crate::search_index_db::normalize_path(&changed.path),
            crate::search_index_db::normalize_path(&f.path)
        );
        runtime.submit(config(&aliased));
        settled(&runtime);
        assert!(!before.same_instance(&runtime.shared.state.lock().unwrap().controls[&f.id]));
        let deadline = Instant::now() + Duration::from_secs(60);
        while !notifier.watch_is_ready_for_test(f.id) {
            assert!(
                Instant::now() < deadline,
                "aliased similar watcher not Ready"
            );
            std::thread::yield_now();
        }
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
    fn startup_cleanup_streams_valid_rows_without_collecting_them() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let f = favorite(1, PathBuf::from("c:/photos"), true);
        for i in 0..1000 {
            stores
                .meta
                .upsert_meta_ok(
                    &format!("c:/photos/{i}.jpg"),
                    f.id,
                    &f.path,
                    crate::fts_index::IndexKind::Image,
                    1,
                    1,
                )
                .unwrap();
        }
        stores
            .meta
            .mark_scanned_once("c:/photos", "complete")
            .unwrap();
        let config = Configuration::new(&[f], Vec::new());
        let targets = collect_startup_cleanup_targets(&stores.meta, &config).unwrap();
        assert_eq!(targets.owner_rows, 1000);
        assert_eq!(
            targets.paths.capacity(),
            0,
            "valid rows must not be retained"
        );
        assert!(targets.roots.is_empty());
        assert!(startup_cleanup(&stores, &config).unwrap().is_empty());
        assert_eq!(
            stores
                .meta
                .count_ok_grouped_by_favorite()
                .unwrap()
                .values()
                .sum::<u64>(),
            1000
        );
        assert_eq!(
            stores.meta.scanned_once("c:/photos").unwrap().as_deref(),
            Some("complete")
        );
    }

    #[test]
    fn startup_cleanup_collects_only_invalid_paths_and_current_owners() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let outer = favorite(1, PathBuf::from("c:/photos"), true);
        let inner = favorite(2, PathBuf::from("c:/photos/inner"), true);
        let off = favorite(3, PathBuf::from("d:/off"), false);
        let valid = ["c:/photos/valid.jpg", "c:/photos/private2/valid.jpg"];
        let invalid = [
            "c:/photos/inner/wrong-owner.jpg",
            "c:/photos/invalid-id.jpg",
            "c:/photos/private/excluded.jpg",
            "d:/off/x.jpg",
            "e:/orphan.jpg",
        ];
        for (path, id, root) in valid
            .iter()
            .map(|p| (*p, outer.id, &outer.path))
            .chain(invalid.iter().map(|p| match *p {
                "d:/off/x.jpg" => (*p, off.id, &off.path),
                "e:/orphan.jpg" => (*p, Uuid::nil(), &outer.path),
                _ => (*p, outer.id, &outer.path),
            }))
            .chain(std::iter::once((
                "c:/photos/inner/valid.jpg",
                inner.id,
                &inner.path,
            )))
        {
            stores
                .meta
                .upsert_meta_ok(path, id, root, crate::fts_index::IndexKind::Image, 1, 1)
                .unwrap();
        }
        rusqlite::Connection::open(tmp.path().join("meta.db"))
            .unwrap()
            .execute(
                "UPDATE files SET favorite_id='invalid UUID' WHERE path='c:/photos/invalid-id.jpg'",
                [],
            )
            .unwrap();
        let config = Configuration::new(&[outer, inner, off], vec!["c:/photos/private".into()]);
        let targets = collect_startup_cleanup_targets(&stores.meta, &config).unwrap();
        assert_eq!(targets.owner_rows, 8);
        assert_eq!(
            targets
                .paths
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
            HashSet::from(invalid)
        );
        assert_eq!(
            targets.roots,
            HashSet::from(["c:/photos".into(), "c:/photos/inner".into()])
        );
        assert_eq!(startup_cleanup(&stores, &config).unwrap(), targets.roots);
        assert_eq!(stores.meta.list_path_owners().unwrap().len(), 3);
    }
    #[test]
    fn startup_cleanup_read_failure_does_not_delete_collected_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let id = Uuid::new_v4();
        for path in ["c:/x/orphan.jpg", "c:/x/invalid-text.jpg"] {
            stores
                .meta
                .upsert_meta_ok(
                    path,
                    id,
                    std::path::Path::new("c:/x"),
                    crate::fts_index::IndexKind::Image,
                    1,
                    1,
                )
                .unwrap();
        }
        let conn = rusqlite::Connection::open(tmp.path().join("meta.db")).unwrap();
        // TEXT sorts before BLOB in the covering index: an orphan is collected before failure.
        conn.execute(
            "UPDATE files SET favorite_id=x'ff' WHERE path='c:/x/invalid-text.jpg'",
            [],
        )
        .unwrap();
        stores.meta.mark_scanned_once("c:/x", "complete").unwrap();
        assert!(startup_cleanup(&stores, &Configuration::new(&[], Vec::new())).is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            stores.meta.scanned_once("c:/x").unwrap().as_deref(),
            Some("complete")
        );
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
    fn broadened_common_exclusion_removes_submitted_unapplied_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = stores(tmp.path());
        let f = favorite(1, tmp.path().join("photos"), true);
        let excluded = f.path.join("private");
        std::fs::create_dir_all(&excluded).unwrap();
        let mut runtime =
            Runtime::start(stores.clone(), Configuration::new(&[f.clone()], Vec::new())).unwrap();
        settled(&runtime);
        initial_done(&runtime);
        let key = crate::search_index_db::normalize_path(&excluded.join("ghost.jpg"));
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
        let (submitted, release) = stores.writer.test_gate_next_batch();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let writer = Arc::clone(&stores.writer);
        let pending = std::thread::spawn(move || {
            writer.batch_cancellable(
                vec![doc],
                Vec::new(),
                true,
                true,
                crate::fts_writer_dispatcher::WriterPriority::Background,
                &worker_cancel,
            )
        });
        submitted.recv_timeout(Duration::from_secs(60)).unwrap();
        cancel.store(true, std::sync::atomic::Ordering::Release);
        release.send(()).unwrap();
        assert!(!pending.join().unwrap().unwrap());
        assert!(stores.meta.get(&key).unwrap().is_none());
        runtime.submit(Configuration::new(&[f], vec![excluded]));
        settled(&runtime);
        initial_done(&runtime);
        assert!(!has_doc(&stores, &key));
        assert!(stores.meta.get(&key).unwrap().is_none());
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
