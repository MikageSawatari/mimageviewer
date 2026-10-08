//! 正規化 root ごとの名前索引 owner。UI は snapshot を提出し、join / DB I/O は worker が行う。
//! 再構成は固定 snapshot の stop → join → clear → start。clear 失敗は次回起動の
//! 作り直し印へ記録しログを残す。Shutdown は UI から全 monitor に即時通知する。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::activity_gate::ActivityGate;
use crate::name_index_supervisor::{NameIndexMonitor, NameIndexStats};
use crate::search_index_db::{SearchIndexDb, normalize_path};
use crate::settings::FavoriteEntry;

#[derive(Clone, Debug)]
struct RootConfig {
    id: Uuid,
    path: PathBuf,
}

#[derive(Clone, Debug, Default)]
struct Configuration {
    active: BTreeMap<String, RootConfig>,
    known: BTreeMap<String, PathBuf>,
    excluded: Vec<PathBuf>,
    excluded_keys: BTreeSet<String>,
}

impl Configuration {
    fn from_favorites(favorites: &[FavoriteEntry], excluded: Vec<PathBuf>) -> Self {
        let mut config = Self {
            excluded_keys: excluded.iter().map(|p| normalize_path(p)).collect(),
            excluded,
            ..Default::default()
        };
        for favorite in favorites {
            let key = normalize_path(&favorite.path);
            config
                .known
                .entry(key.clone())
                .or_insert_with(|| favorite.path.clone());
            if favorite.auto_index_structure {
                let entry = config.active.entry(key).or_insert_with(|| RootConfig {
                    id: favorite.id,
                    path: favorite.path.clone(),
                });
                if favorite.id.to_string() < entry.id.to_string() {
                    *entry = RootConfig {
                        id: favorite.id,
                        path: favorite.path.clone(),
                    };
                }
            }
        }
        config
    }
}

#[derive(Clone, Copy)]
enum Lifecycle {
    Running,
    Shutdown { deadline: Instant },
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Applying,
}

struct Mailbox {
    lifecycle: Lifecycle,
    pending: Option<Configuration>,
    phase: Phase,
    monitors: BTreeMap<String, NameIndexMonitor>,
    requested_roots: BTreeSet<String>,
    full_check_requested: bool,
}

struct Shared {
    state: Mutex<Mailbox>,
    wake: Condvar,
    #[cfg(test)]
    full_check_requests: std::sync::atomic::AtomicUsize,
}

impl Shared {
    fn shutdown_deadline(&self) -> Option<Instant> {
        match self.state.lock().unwrap().lifecycle {
            Lifecycle::Running => None,
            Lifecycle::Shutdown { deadline } => Some(deadline),
        }
    }
}

struct ManagedSupervisor {
    monitor: NameIndexMonitor,
    thread: JoinHandle<()>,
}

/// テストも production と同じ worker / join の所有境界を使う。
trait Backend: Send + Sync + 'static {
    fn spawn(
        &self,
        root: &RootConfig,
        excluded: &[PathBuf],
        shared: &Arc<Shared>,
        startup: bool,
    ) -> Result<ManagedSupervisor, String>;
    fn clear(&self, root: &std::path::Path) -> Result<(), String>;
}

struct DatabaseBackend {
    db: Arc<SearchIndexDb>,
    gate: Option<Arc<ActivityGate>>,
    skip_offline_change_scan: bool,
}

impl Backend for DatabaseBackend {
    fn spawn(
        &self,
        root: &RootConfig,
        excluded: &[PathBuf],
        _shared: &Arc<Shared>,
        startup: bool,
    ) -> Result<ManagedSupervisor, String> {
        let handle = crate::name_index_supervisor::try_spawn_with_startup_policy(
            root.id,
            root.path.clone(),
            Arc::clone(&self.db),
            excluded.to_vec(),
            self.gate.clone(),
            startup && self.skip_offline_change_scan,
        )
        .map_err(|e| e.to_string())?;
        let (monitor, thread) = handle.into_worker_parts();
        Ok(ManagedSupervisor { monitor, thread })
    }

    fn clear(&self, root: &std::path::Path) -> Result<(), String> {
        if let Err(error) = self.db.clear_for_favorite(root) {
            if let Err(marker_error) = self.db.request_rebuild_on_next_start() {
                crate::logger::log(format!(
                    "name-index-manager: rebuild marker failed: {marker_error}"
                ));
            }
            return Err(error.to_string());
        }
        Ok(())
    }
}

pub struct NameIndexManager {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl NameIndexManager {
    pub fn new(db: Arc<SearchIndexDb>, gate: Option<Arc<ActivityGate>>) -> std::io::Result<Self> {
        Self::new_with_startup_policy(db, gate, false)
    }

    /// 起動時の設定 snapshot。実行中の設定変更では既存 supervisor を再構成しない。
    pub fn new_with_startup_policy(
        db: Arc<SearchIndexDb>,
        gate: Option<Arc<ActivityGate>>,
        skip_offline_change_scan: bool,
    ) -> std::io::Result<Self> {
        Self::with_backend(Arc::new(DatabaseBackend {
            db,
            gate,
            skip_offline_change_scan,
        }))
    }

    fn with_backend(backend: Arc<dyn Backend>) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(Mailbox {
                lifecycle: Lifecycle::Running,
                pending: None,
                phase: Phase::Idle,
                monitors: BTreeMap::new(),
                requested_roots: BTreeSet::new(),
                full_check_requested: false,
            }),
            wake: Condvar::new(),
            #[cfg(test)]
            full_check_requests: std::sync::atomic::AtomicUsize::new(0),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("name-index-manager".into())
            .spawn(move || run_worker(worker_shared, backend))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub fn sync_with_favorites(&self, favorites: &[FavoriteEntry], excluded: Vec<PathBuf>) {
        let config = Configuration::from_favorites(favorites, excluded);
        let mut state = self.shared.state.lock().unwrap();
        if matches!(state.lifecycle, Lifecycle::Shutdown { .. }) {
            return;
        }
        state.requested_roots = config.active.keys().cloned().collect();
        state.pending = Some(config);
        self.shared.wake.notify_all();
    }

    pub fn stats_by_id(&self, favorites: &[FavoriteEntry]) -> HashMap<Uuid, NameIndexStats> {
        let monitors = self.shared.state.lock().unwrap().monitors.clone();
        let snapshots: BTreeMap<_, _> = monitors
            .into_iter()
            .map(|(root, monitor)| (root, monitor.snapshot_stats()))
            .collect();
        favorites
            .iter()
            .filter(|f| f.auto_index_structure)
            .filter_map(|favorite| {
                snapshots
                    .get(&normalize_path(&favorite.path))
                    .map(|stats| (favorite.id, stats.clone()))
            })
            .collect()
    }

    pub fn all_initial_scans_done(&self) -> bool {
        let state = self.shared.state.lock().unwrap();
        if !matches!(state.phase, Phase::Idle) || state.pending.is_some() {
            return false;
        }
        state.requested_roots.iter().all(|key| {
            state
                .monitors
                .get(key)
                .is_some_and(|m| m.snapshot_stats().initial_scan_done)
        })
    }

    pub fn any_in_full_scan(&self) -> bool {
        let state = self.shared.state.lock().unwrap();
        state.full_check_requested
            || state.pending.is_some()
            || !matches!(state.phase, Phase::Idle)
            || state
                .monitors
                .values()
                .any(|m| m.snapshot_stats().in_full_scan)
    }

    pub fn request_full_rescan(&self) {
        let mut state = self.shared.state.lock().unwrap();
        if matches!(state.lifecycle, Lifecycle::Running) {
            #[cfg(test)]
            self.shared
                .full_check_requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // stop/join/clear/start 中も受け付け、採用された構成の supervisor へ1回送る。
            state.full_check_requested = true;
            self.shared.wake.notify_all();
        }
    }

    #[cfg(test)]
    pub(crate) fn request_full_check_count_for_test(&self) -> usize {
        self.shared
            .full_check_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn signal_shutdown(&self) {
        let mut state = self.shared.state.lock().unwrap();
        if matches!(state.lifecycle, Lifecycle::Running) {
            state.lifecycle = Lifecycle::Shutdown {
                deadline: Instant::now() + Duration::from_secs(4),
            };
        }
        state.pending = None;
        state.full_check_requested = false;
        for monitor in state.monitors.values() {
            monitor.signal_stop();
        }
        self.shared.wake.notify_all();
    }
}

impl Drop for NameIndexManager {
    fn drop(&mut self) {
        self.signal_shutdown();
        // worker は停止中 handle も含む期限を所有する。UI は join/SQLite を待たない。
        self.worker.take();
    }
}

struct Worker {
    current: Configuration,
    known: BTreeMap<String, PathBuf>,
    running: BTreeMap<String, ManagedSupervisor>,
    cleared: BTreeSet<String>,
    startup: bool,
}

enum Action {
    Configure(Configuration),
    FullCheck,
    Shutdown,
}

fn run_worker(shared: Arc<Shared>, backend: Arc<dyn Backend>) {
    let mut worker = Worker {
        current: Configuration::default(),
        known: BTreeMap::new(),
        running: BTreeMap::new(),
        cleared: BTreeSet::new(),
        startup: true,
    };
    loop {
        let action = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if matches!(state.lifecycle, Lifecycle::Shutdown { .. }) {
                    break Action::Shutdown;
                }
                if let Some(config) = state.pending.take() {
                    state.phase = Phase::Applying;
                    break Action::Configure(config);
                }
                if state.full_check_requested {
                    state.full_check_requested = false;
                    state.phase = Phase::Applying;
                    break Action::FullCheck;
                }
                state.phase = Phase::Idle;
                shared.wake.notify_all();
                state = shared.wake.wait(state).unwrap();
            }
        };
        if matches!(action, Action::Shutdown) {
            break;
        }
        let configured = matches!(action, Action::Configure(_));
        if let Action::Configure(config) = action {
            worker.configure(config, &shared);
        } else if matches!(action, Action::FullCheck) {
            for supervisor in worker.running.values() {
                supervisor.monitor.request_full_rescan();
            }
        }
        if shared.shutdown_deadline().is_some() {
            break;
        }
        worker.clear_and_start(&shared, backend.as_ref());
        if configured {
            worker.startup = false;
        }
    }
    for supervisor in worker.running.values() {
        supervisor.monitor.signal_stop();
    }
    for (key, supervisor) in std::mem::take(&mut worker.running) {
        finish_supervisor(&shared, &key, supervisor);
    }
    let mut state = shared.state.lock().unwrap();
    state.monitors.clear();
    state.phase = Phase::Idle;
    shared.wake.notify_all();
}

fn finish_supervisor(shared: &Shared, key: &str, supervisor: ManagedSupervisor) {
    supervisor.monitor.signal_stop();
    while !supervisor.thread.is_finished() {
        if shared
            .shutdown_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            crate::logger::log(format!(
                "name-index-manager: shutdown deadline, detaching {key}"
            ));
            shared.state.lock().unwrap().monitors.remove(key);
            return;
        }
        // join 待ちだけを worker で行う。UI mutex/DB lock を保持しない。
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = supervisor.thread.join();
    shared.state.lock().unwrap().monitors.remove(key);
}

impl Worker {
    fn stop_roots(&mut self, keys: &[String], shared: &Shared) {
        for key in keys {
            if let Some(supervisor) = self.running.get(key) {
                supervisor.monitor.signal_stop();
            }
        }
        for key in keys {
            if let Some(supervisor) = self.running.remove(key) {
                finish_supervisor(shared, key, supervisor);
            }
        }
    }

    fn configure(&mut self, config: Configuration, shared: &Shared) {
        let exclusions_changed = self.current.excluded_keys != config.excluded_keys;
        let stop: Vec<_> = self
            .running
            .keys()
            .filter(|key| exclusions_changed || !config.active.contains_key(*key))
            .cloned()
            .collect();
        self.stop_roots(&stop, shared);
        self.known.extend(config.known.clone());
        self.current = config;
    }

    fn spawn_root(
        &mut self,
        key: &str,
        root: &RootConfig,
        shared: &Arc<Shared>,
        backend: &dyn Backend,
    ) -> Result<bool, String> {
        // Shutdown と spawn 採用は同じ短時間 lock で順序を確定する。DB/scan/join はこの lock 外。
        let mut state = shared.state.lock().unwrap();
        if matches!(state.lifecycle, Lifecycle::Shutdown { .. }) {
            return Ok(false);
        }
        let supervisor = backend.spawn(root, &self.current.excluded, shared, self.startup)?;
        state
            .monitors
            .insert(key.to_owned(), supervisor.monitor.clone());
        self.running.insert(key.to_owned(), supervisor);
        self.cleared.remove(key);
        Ok(true)
    }

    fn clear_and_start(&mut self, shared: &Arc<Shared>, backend: &dyn Backend) {
        let clear: Vec<_> = self
            .known
            .iter()
            .filter(|(key, _)| {
                !self.current.active.contains_key(*key) && !self.cleared.contains(*key)
            })
            .map(|(key, path)| (key.clone(), path.clone()))
            .collect();
        for (key, path) in clear {
            if shared.shutdown_deadline().is_some() {
                return;
            }
            if let Err(error) = backend.clear(&path) {
                crate::logger::log(format!(
                    "name-index-manager: clear for {} failed: {error}",
                    path.display()
                ));
            }
            // v7: 回復待ち状態を持たず、失敗は次回起動の作り直しへ委ねる。
            self.cleared.insert(key);
        }
        let start: Vec<_> = self
            .current
            .active
            .iter()
            .filter(|(key, _)| !self.running.contains_key(*key))
            .map(|(key, root)| (key.clone(), root.clone()))
            .collect();
        for (key, root) in start {
            match self.spawn_root(&key, &root, shared, backend) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => crate::logger::log(format!(
                    "name-index-manager: spawn for {} failed: {error}",
                    root.path.display()
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{Receiver, Sender, unbounded};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        Spawn(String),
        Stop(String),
        Joined(String),
        Clear(String),
        FullCheck(String),
    }

    struct FakeBackend {
        events: Sender<Event>,
        stop_gate: Mutex<Option<Receiver<()>>>,
        fail_clear: AtomicBool,
        startup_policies: Mutex<Vec<bool>>,
    }

    impl Backend for FakeBackend {
        fn spawn(
            &self,
            root: &RootConfig,
            _excluded: &[PathBuf],
            _shared: &Arc<Shared>,
            startup: bool,
        ) -> Result<ManagedSupervisor, String> {
            self.startup_policies.lock().unwrap().push(startup);
            let key = normalize_path(&root.path);
            self.events.send(Event::Spawn(key.clone())).unwrap();
            let events = self.events.clone();
            let gate = self.stop_gate.lock().unwrap().take();
            let (monitor, commands) = NameIndexMonitor::for_test();
            let thread = std::thread::spawn(move || {
                while let Ok(command) = commands.recv() {
                    if matches!(
                        command,
                        crate::name_index_supervisor::NameIndexCommand::FullRescan
                    ) {
                        let _ = events.send(Event::FullCheck(key.clone()));
                    }
                    if matches!(
                        command,
                        crate::name_index_supervisor::NameIndexCommand::Stop
                    ) {
                        break;
                    }
                }
                let _ = events.send(Event::Stop(key.clone()));
                if let Some(gate) = gate {
                    let _ = gate.recv();
                }
                let _ = events.send(Event::Joined(key));
            });
            Ok(ManagedSupervisor { monitor, thread })
        }

        fn clear(&self, root: &std::path::Path) -> Result<(), String> {
            self.events
                .send(Event::Clear(normalize_path(root)))
                .unwrap();
            if self.fail_clear.load(Ordering::SeqCst) {
                Err("injected clear failure".into())
            } else {
                Ok(())
            }
        }
    }

    fn favorite(id: u128, path: &str, on: bool) -> FavoriteEntry {
        FavoriteEntry {
            id: Uuid::from_u128(id),
            name: format!("favorite-{id}"),
            path: path.into(),
            auto_index_structure: on,
            auto_index_metadata: false,
            auto_index_thumbs: false,
            auto_index_similar: false,
        }
    }

    fn setup(gate: Option<Receiver<()>>) -> (NameIndexManager, Arc<FakeBackend>, Receiver<Event>) {
        let (events, rx) = unbounded();
        let backend = Arc::new(FakeBackend {
            events,
            stop_gate: Mutex::new(gate),
            fail_clear: AtomicBool::new(false),
            startup_policies: Mutex::new(Vec::new()),
        });
        let manager = NameIndexManager::with_backend(backend.clone()).unwrap();
        (manager, backend, rx)
    }

    fn event(rx: &Receiver<Event>) -> Event {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("worker event")
    }

    fn idle(manager: &NameIndexManager) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = manager.shared.state.lock().unwrap();
        while state.pending.is_some()
            || state.full_check_requested
            || !matches!(state.phase, Phase::Idle)
        {
            assert!(Instant::now() < deadline, "manager did not settle");
            state = manager
                .shared
                .wake
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap()
                .0;
        }
    }

    fn shutdown(mut manager: NameIndexManager) {
        manager.signal_shutdown();
        manager.worker.take().unwrap().join().unwrap();
    }

    #[test]
    fn startup_reuse_policy_is_not_reused_after_runtime_exclusion_roundtrip() {
        let (manager, backend, events) = setup(None);
        let favorites = vec![favorite(1, r"C:\Books", true)];
        for excluded in [vec![], vec![PathBuf::from(r"C:\Books\Excluded")], vec![]] {
            manager.sync_with_favorites(&favorites, excluded);
            idle(&manager);
        }
        assert_eq!(
            *backend.startup_policies.lock().unwrap(),
            [true, false, false]
        );
        assert_eq!(
            events
                .try_iter()
                .filter(|event| matches!(event, Event::Spawn(_)))
                .count(),
            3
        );
        shutdown(manager);
    }

    #[test]
    fn off_then_on_cannot_clear_after_new_scan_starts() {
        let (release, gate) = unbounded();
        let (manager, _, events) = setup(Some(gate));
        let mut favorites = vec![favorite(1, r"C:\Books", true)];
        manager.sync_with_favorites(&favorites, vec![]);
        assert_eq!(event(&events), Event::Spawn("c:/books".into()));
        idle(&manager);
        favorites[0].auto_index_structure = false;
        manager.sync_with_favorites(&favorites, vec![]);
        assert_eq!(event(&events), Event::Stop("c:/books".into()));
        favorites[0].auto_index_structure = true;
        manager.sync_with_favorites(&favorites, vec![]);
        assert!(events.try_recv().is_err());
        release.send(()).unwrap();
        assert_eq!(event(&events), Event::Joined("c:/books".into()));
        assert_eq!(event(&events), Event::Clear("c:/books".into()));
        assert_eq!(event(&events), Event::Spawn("c:/books".into()));
        idle(&manager);
        assert!(events.try_recv().is_err());
        shutdown(manager);
    }

    #[test]
    fn shared_root_uuid_off_preserves_single_supervisor_and_maps_progress() {
        let (manager, _, events) = setup(None);
        let mut favorites = vec![
            favorite(2, r"C:\Books", true),
            favorite(1, "c:/BOOKS", true),
        ];
        manager.sync_with_favorites(&favorites, vec![]);
        assert_eq!(event(&events), Event::Spawn("c:/books".into()));
        idle(&manager);
        let stats = manager.stats_by_id(&favorites);
        assert_eq!(stats.len(), 2);
        assert!(stats.values().all(|s| s.initial_scan_done));
        favorites[1].auto_index_structure = false;
        favorites.reverse();
        manager.sync_with_favorites(&favorites, vec![]);
        idle(&manager);
        assert!(events.try_recv().is_err());
        assert_eq!(manager.stats_by_id(&favorites).len(), 1);
        shutdown(manager);
    }

    #[test]
    fn startup_and_edit_snapshots_share_root_owner_and_cleanup_path_changes() {
        let (manager, _, events) = setup(None);
        let mut favorites = vec![
            favorite(1, "a", true),
            favorite(2, "b", true),
            favorite(3, "off", false),
        ];
        manager.sync_with_favorites(&favorites, vec![]);
        assert_eq!(event(&events), Event::Clear("off".into()));
        assert_eq!(event(&events), Event::Spawn("a".into()));
        assert_eq!(event(&events), Event::Spawn("b".into()));
        idle(&manager);
        manager.sync_with_favorites(&favorites, vec![]);
        idle(&manager);
        assert!(events.try_recv().is_err());
        favorites[0].path = "c".into();
        manager.sync_with_favorites(&favorites, vec![]);
        assert_eq!(event(&events), Event::Stop("a".into()));
        assert_eq!(event(&events), Event::Joined("a".into()));
        assert_eq!(event(&events), Event::Clear("a".into()));
        assert_eq!(event(&events), Event::Spawn("c".into()));
        idle(&manager);
        assert!(
            events.try_recv().is_err(),
            "unrelated b root must stay running"
        );
        shutdown(manager);
    }

    #[test]
    fn shutdown_covers_handle_owned_by_joining_worker_and_never_respawns() {
        let (release, gate) = unbounded();
        let (mut manager, _, events) = setup(Some(gate));
        let mut favorites = vec![favorite(1, "a", true)];
        manager.sync_with_favorites(&favorites, vec![]);
        assert_eq!(event(&events), Event::Spawn("a".into()));
        idle(&manager);
        favorites[0].auto_index_structure = false;
        manager.sync_with_favorites(&favorites, vec![]);
        assert_eq!(event(&events), Event::Stop("a".into()));
        favorites[0].auto_index_structure = true;
        manager.sync_with_favorites(&favorites, vec![]);
        manager.signal_shutdown();
        // 時計を期限へ進め、4 秒を実時間で待たずに同じ期限分岐を検証する。
        manager.shared.state.lock().unwrap().lifecycle = Lifecycle::Shutdown {
            deadline: Instant::now(),
        };
        manager.shared.wake.notify_all();
        manager.worker.take().unwrap().join().unwrap();
        assert!(
            events.try_recv().is_err(),
            "shutdown must skip clear and new spawn"
        );
        release.send(()).unwrap();
        assert_eq!(event(&events), Event::Joined("a".into()));
        assert!(manager.shared.state.lock().unwrap().monitors.is_empty());
    }

    #[test]
    fn pending_configurations_coalesce_while_old_supervisor_joins() {
        let (release, gate) = unbounded();
        let (manager, _, events) = setup(Some(gate));
        manager.sync_with_favorites(&[favorite(1, "a", true)], vec![]);
        assert_eq!(event(&events), Event::Spawn("a".into()));
        idle(&manager);
        manager.sync_with_favorites(&[favorite(1, "a", false)], vec![]);
        assert_eq!(event(&events), Event::Stop("a".into()));
        manager.sync_with_favorites(&[favorite(1, "b", true)], vec![]);
        manager.sync_with_favorites(&[favorite(1, "c", true)], vec![]);
        release.send(()).unwrap();
        assert_eq!(event(&events), Event::Joined("a".into()));
        assert_eq!(event(&events), Event::Clear("a".into()));
        assert_eq!(event(&events), Event::Spawn("c".into()));
        idle(&manager);
        assert!(events.try_recv().is_err());
        shutdown(manager);
    }

    #[test]
    fn clear_failure_does_not_delay_latest_on_snapshot_or_create_retry_worker() {
        let (manager, backend, events) = setup(None);
        backend.fail_clear.store(true, Ordering::SeqCst);
        manager.sync_with_favorites(&[favorite(1, "a", false)], vec![]);
        assert_eq!(event(&events), Event::Clear("a".into()));
        idle(&manager);
        manager.sync_with_favorites(&[favorite(1, "a", false)], vec![]);
        idle(&manager);
        assert!(events.try_recv().is_err());
        manager.sync_with_favorites(&[favorite(1, "a", true)], vec![]);
        assert_eq!(event(&events), Event::Spawn("a".into()));
        idle(&manager);
        shutdown(manager);
    }

    #[test]
    fn production_clear_failure_records_rebuild_without_changing_ctrl_s_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("names.db");
        let db = Arc::new(SearchIndexDb::open_at(&path).unwrap());
        let root = std::path::Path::new("off-root");
        db.upsert_children(
            root,
            root,
            &[crate::search_index_db::IndexEntry {
                path: root.join("stale.zip"),
                display_name: "stale.zip".into(),
                kind: crate::search_index_db::IndexKind::ZipFile,
                mtime: 0,
            }],
        )
        .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_clear BEFORE DELETE ON entries BEGIN SELECT RAISE(FAIL, 'injected clear failure'); END;").unwrap();
        let manager = NameIndexManager::new(Arc::clone(&db), None).unwrap();
        manager.sync_with_favorites(&[favorite(1, "off-root", false)], vec![]);
        idle(&manager);
        // 既存の Ctrl+S『すべて』scope は OFF favorite も含む。この前提を変更しない。
        assert_eq!(db.count_for_favorite(root).unwrap(), 1);
        let pending: i64 = conn
            .query_row(
                "SELECT pending FROM name_index_rebuild_state WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);
        shutdown(manager);
        conn.execute_batch("DROP TRIGGER fail_clear").unwrap();
        drop(conn);
        drop(db);
        assert_eq!(
            SearchIndexDb::open_at(&path)
                .unwrap()
                .count_for_favorite(root)
                .unwrap(),
            0
        );
    }

    #[test]
    fn manual_check_reaches_every_normalized_root_once() {
        let (manager, _, events) = setup(None);
        manager.sync_with_favorites(
            &[
                favorite(1, "a", true),
                favorite(2, "A", true),
                favorite(3, "b", true),
            ],
            vec![],
        );
        assert_eq!(event(&events), Event::Spawn("a".into()));
        assert_eq!(event(&events), Event::Spawn("b".into()));
        idle(&manager);
        manager.request_full_rescan();
        let mut checked = vec![event(&events), event(&events)];
        checked.sort_by_key(|event| format!("{event:?}"));
        assert_eq!(
            checked,
            vec![Event::FullCheck("a".into()), Event::FullCheck("b".into())]
        );
        idle(&manager);
        assert!(events.try_recv().is_err());
        shutdown(manager);
    }

    #[test]
    fn manual_check_while_joining_is_reserved_for_new_supervisor() {
        let (release, gate) = unbounded();
        let (manager, _, events) = setup(Some(gate));
        manager.sync_with_favorites(&[favorite(1, "a", true)], vec![]);
        assert_eq!(event(&events), Event::Spawn("a".into()));
        idle(&manager);
        manager.sync_with_favorites(&[favorite(1, "b", true)], vec![]);
        assert_eq!(event(&events), Event::Stop("a".into()));
        manager.request_full_rescan();
        manager.request_full_rescan();
        assert!(events.try_recv().is_err());
        release.send(()).unwrap();
        assert_eq!(event(&events), Event::Joined("a".into()));
        assert_eq!(event(&events), Event::Clear("a".into()));
        assert_eq!(event(&events), Event::Spawn("b".into()));
        assert_eq!(event(&events), Event::FullCheck("b".into()));
        idle(&manager);
        assert!(events.try_recv().is_err());
        shutdown(manager);
    }
}
