//! `IndexerManager` — App 統合のための Supervisor 群管理 (docs/archive/search-metadata/search-expansion-design.md §3)。
//!
//! 全お気に入りの `SupervisorHandle` を束ねて、以下を提供する:
//!
//! - 起動時: `auto_index_metadata` / `auto_index_similar` のどちらかが true の
//!   お気に入りに対して Supervisor を spawn
//! - お気に入り変更時: `sync_with_favorites` で追加/削除/フラグ変更を反映
//! - 起動時 reconciliation (§5.6.3): pending/failed/tombstone を supervisor 起動前に掃除
//! - Ctrl+G 検索: `spawn_search` で global_search::run を別スレッド実行
//! - 進捗 UI: `all_stats()` で各 supervisor の SupervisorStats を取得
//! - シャットダウン: App drop 時に全 Supervisor を停止
//!
//! ## ライフサイクル
//!
//! ```text
//!   App::new
//!     └── IndexerManager::new(settings.favorites)
//!           ├── FtsMetaDb + FtsIndex を開く
//!           ├── 起動時 reconciliation (status != ok を整理)
//!           └── 自動索引が有効なお気に入りに共有 Supervisor を spawn
//!   ...
//!   App::update ループ:
//!     - all_stats() で進捗取得 (軽量な lock)
//!     - Ctrl+G 入力 → spawn_search()
//!     - お気に入り編集保存時 → sync_with_favorites()
//!   ...
//!   App::drop → IndexerManager::drop → 全 Supervisor drop
//! ```
//!
//! UI calls submit snapshots and read views; all stops, joins and cleanup run on one worker.
//! Startup reconciliation completes there before supervisors are spawned.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crossbeam_channel::{Receiver, Sender};
use uuid::Uuid;

use crate::activity_gate::ActivityGate;
use crate::fts_index::FtsIndex;
use crate::fts_meta::FtsMetaDb;
use crate::global_search::SearchStreamEvent;
use crate::indexer_supervisor::SupervisorStats;
use crate::io_semaphore::GlobalIoSemaphore;
use crate::settings::FavoriteEntry;

// 注: IO permits は `IndexerSpeedProfile::io_permits()` から決まる (Low=1/Med=2/High=4)。
// IndexerManager::new の `speed` 引数経由で渡される。

/// 検索ハンドル。UI 側が `try_recv` で stream を受け取る。
///
/// **Drop 挙動** (Codex round-8 Should-fix #2): `Drop` 実装で cancel フラグを立てる。
/// これで呼び出し側が handle を単に drop するだけでワーカーが次のチェックポイントで
/// 自己終了する。明示的に `handle.cancel.store(true)` を呼ぶ必要はない。
pub struct SearchHandle {
    pub cancel: Arc<AtomicBool>,
    pub rx: Receiver<SearchStreamEvent>,
}

impl Drop for SearchHandle {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        // ワーカースレッドは自分で finish する。ここで join しない
        // (Ctrl+G の UI フレーム内で drop されるので、長時間ブロックしない契約)。
    }
}

/// IndexerManager のコア。App が保有する。
pub struct IndexerManager {
    meta_db: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    /// **全 supervisor で共有する** Tantivy IndexWriter のディスパッチャー。
    /// Tantivy は 1 Index につき IndexWriter を 1 本しか許さないため、所有権を専用スレッドに
    /// 集約し、優先度付きキュー (Interactive > Background) でジョブを直列処理する
    /// (`fts_writer_dispatcher` 参照)。旧設計の `Arc<Mutex<IndexWriter>>` 直接共有は
    /// indexer の長時間 lock 保持で interactive 操作 (タグ書き込み) が starve する問題があったため
    /// 廃止 (2026-04 commit 14037af + ユーザー報告)。
    writer: Arc<crate::fts_writer_dispatcher::FtsWriterDispatcher>,
    io_sem: Arc<GlobalIoSemaphore>,
    /// アプリ管理下で生成する派生コンテンツなど、検索索引から除外する root。
    excluded_roots: Vec<std::path::PathBuf>,
    /// 再構成 worker が supervisor ハンドルと構成 snapshot を所有する。
    runtime: crate::metadata_reconfiguration::Runtime,
    startup_diag: StartupDiag,
}

/// 起動時 reconciliation の統計スナップショット (UI 表示用)。
#[derive(Clone, Copy, Debug, Default)]
pub struct StartupDiag {
    /// reconciliation に要した時間 (ms)
    pub reconciliation_ms: u64,
    /// Tantivy から delete した残留 failed 行数
    pub failed_cleaned: usize,
    /// スピードプロファイル (io_permits 数) — 診断時に見たいので保存
    pub io_permits: usize,
}

/// `FtsMetaDb` と `FtsIndex` を `data_dir` 配下で開く。
///
/// fts_meta が INDEX_VERSION bump / 旧スキーマを検出して `files` テーブルを drop した場合、
/// Tantivy 側も一緒に wipe しないと旧 key 形式 (例: `!` separator) で書かれた orphan doc が
/// 残り続ける (post-filter で弾かれるが容量を食う、かつ将来リカバリ経路が増えたら顕在化し得る)。
/// このため fts_meta open → 旧 STORED tags を tags.db へ移行 → durable rebuild pending
/// チェック → 必要なら `fts_index` 削除 → fts open → pending clear の順で実行する。
///
/// 本番 `new()` とテスト用 `new_at()` の両方から呼ぶ (Codex P3 指摘: 旧コードでは
/// `new_at` が wipe を再現していなかったため、version bump の挙動を統合テストで検証できず
/// 本番と乖離していた)。
/// 起動進捗 (UI 側のオーバーレイに表示する短文) を更新するためのフック。
///
/// `IndexerManager::new` 内部で各 sub-step の前に呼ばれる。
/// `None` を渡せば従来の挙動。`Some` の場合は文字列を Mutex に書き込む。
pub type StartupProgressHook = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// App の既存 startup completion channel に載せる typed outcome。状態を App field に
/// 重複保持せず、migration failure の user message を poll owner が 1 回だけ通知する。
pub enum StartupInitOutcome {
    Ready(IndexerManager),
    Unavailable,
    Failed { user_message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoreOpenFailure {
    Unavailable,
    RebuildDeferred,
}

fn run_legacy_tantivy_tag_import(
    data_dir: &std::path::Path,
    fts_dir: &std::path::Path,
    log_tag: &str,
    progress: Option<&StartupProgressHook>,
) {
    let mut tags_db = match crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")) {
        Ok(db) => db,
        Err(e) => {
            crate::logger::log(format!(
                "{log_tag}: tags.db open for legacy import failed: {e}"
            ));
            return;
        }
    };
    if tags_db
        .meta(crate::tags_db::LEGACY_TANTIVY_IMPORTED_META)
        .as_deref()
        == Some("1")
    {
        return;
    }
    if let Some(p) = progress {
        p("旧タグをタグカタログへ移行しています…");
    }
    let t_import = std::time::Instant::now();
    let legacy_docs = match crate::fts_index::collect_legacy_tag_docs_at(fts_dir) {
        Ok(docs) => docs,
        Err(e) => {
            crate::logger::log(format!(
                "{log_tag}: collect legacy Tantivy tags failed: {e} (continuing)"
            ));
            return;
        }
    };
    let report = match tags_db.import_legacy_tantivy_tags(
        legacy_docs
            .into_iter()
            .map(|doc| (doc.item_key, doc.tags_column)),
    ) {
        Ok(report) => report,
        Err(e) => {
            crate::logger::log(format!(
                "{log_tag}: import legacy Tantivy tags failed: {e} (continuing)"
            ));
            return;
        }
    };
    crate::perf::emit_ms("startup", "legacy_tantivy_tag_import", 0, t_import);
    crate::logger::log(format!(
        "{log_tag}: legacy Tantivy tag import: scanned_docs={}, imported_items={}, \
         inserted_tags={}, skipped_decided_items={}, skipped_already_imported={}",
        report.scanned_docs,
        report.imported_items,
        report.inserted_tags,
        report.skipped_decided_items,
        report.skipped_already_imported
    ));
}

fn open_stores_with_rebuild_sync(
    data_dir: &std::path::Path,
    log_tag: &str,
    progress: Option<&StartupProgressHook>,
) -> Result<(Arc<FtsMetaDb>, Arc<FtsIndex>), StoreOpenFailure> {
    open_stores_with_rebuild_sync_using(data_dir, log_tag, progress, |path| {
        std::fs::remove_dir_all(path)
    })
}

fn open_stores_with_rebuild_sync_using(
    data_dir: &std::path::Path,
    log_tag: &str,
    progress: Option<&StartupProgressHook>,
    remove_fts_dir: impl Fn(&std::path::Path) -> std::io::Result<()>,
) -> Result<(Arc<FtsMetaDb>, Arc<FtsIndex>), StoreOpenFailure> {
    if let Some(p) = progress {
        p("アイテム索引データベースを開いています…");
    }
    let meta_path = data_dir.join("fts_meta.db");
    let fts_dir = data_dir.join("fts_index");
    // meta DB を失ったのに Tantivy だけ残った場合、files inventory が無いので旧 docs を
    // 公開できない。marker は DB 初期化 transaction 内で立て、crash gap を作らない。
    let force_tantivy_rebuild = !meta_path.exists() && fts_dir.exists();
    let t_meta = std::time::Instant::now();
    let meta_db = match FtsMetaDb::open_at_with_tantivy_rebuild_requirement(
        &meta_path,
        force_tantivy_rebuild,
    ) {
        Ok(db) => db,
        Err(e) => {
            crate::logger::log(format!("{log_tag}: FtsMetaDb open failed: {e}"));
            return Err(StoreOpenFailure::Unavailable);
        }
    };
    crate::perf::emit_ms("startup", "fts_meta_open", 0, t_meta);
    run_legacy_tantivy_tag_import(data_dir, &fts_dir, log_tag, progress);
    let rebuild_pending = match meta_db.tantivy_rebuild_pending() {
        Ok(pending) => pending,
        Err(e) => {
            crate::logger::log(format!(
                "{log_tag}: read Tantivy rebuild marker failed: {e}"
            ));
            return Err(StoreOpenFailure::Unavailable);
        }
    };
    if rebuild_pending {
        if let Some(p) = progress {
            p("古いインデックスを削除しています…");
        }
        crate::logger::log(format!(
            "{log_tag}: durable rebuild pending → wiping Tantivy index dir {}",
            fts_dir.display()
        ));
        let t_wipe = std::time::Instant::now();
        if let Err(e) = remove_fts_dir(&fts_dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                crate::logger::log(format!(
                    "{log_tag}: wipe fts_index failed for {}: {e}; old index will not be opened, rebuild remains pending",
                    fts_dir.display()
                ));
                return Err(StoreOpenFailure::RebuildDeferred);
            }
        }
        crate::perf::emit_ms("startup", "fts_index_wipe", 0, t_wipe);
    }
    let meta_db = Arc::new(meta_db);
    if let Some(p) = progress {
        p("全文検索インデックスを開いています…");
    }
    let t_fts = std::time::Instant::now();
    let fts = match FtsIndex::open_at(&fts_dir) {
        Ok(idx) => Arc::new(idx),
        Err(e) => {
            crate::logger::log(format!("{log_tag}: FtsIndex open failed: {e}"));
            return Err(if rebuild_pending {
                StoreOpenFailure::RebuildDeferred
            } else {
                StoreOpenFailure::Unavailable
            });
        }
    };
    crate::perf::emit_ms("startup", "fts_index_open", 0, t_fts);
    if fts.recreated_on_open() {
        meta_db
            .reset_inventory_for_recreated_tantivy()
            .map_err(|error| {
                crate::logger::log(format!(
                    "{log_tag}: clear scan markers after store recreation failed: {error}"
                ));
                if let Err(pending_error) = meta_db.request_item_index_rebuild() {
                    crate::logger::log(format!(
                        "{log_tag}: request rebuild failed: {pending_error}"
                    ));
                }
                StoreOpenFailure::RebuildDeferred
            })?;
    }
    if rebuild_pending && let Err(e) = meta_db.complete_tantivy_rebuild() {
        crate::logger::log(format!(
            "{log_tag}: clear Tantivy rebuild marker failed: {e}; rebuild remains pending"
        ));
        return Err(StoreOpenFailure::RebuildDeferred);
    }
    Ok((meta_db, fts))
}

impl IndexerManager {
    /// DB/index を開き、起動時 reconciliation → 自動索引が有効なお気に入りに
    /// 共有 Supervisor を spawn する。
    ///
    /// DB 初期化結果を startup completion channel 用の typed outcome で返す。
    ///
    /// 起動時 reconciliation は再構成 worker が supervisors spawn の前に実行する。
    /// IndexWriter は dispatcher が所有し、掃除と通常の取り込みを直列に処理する。
    /// `progress` を渡すと各 sub-step (FtsMetaDb open / FtsIndex open / writer init /
    /// reconciliation / supervisor spawn) の前に短い進捗文字列が書き込まれる。
    /// 起動オーバーレイで状態を見せたい場合に渡す。`None` なら従来通り無音。
    pub fn new(
        favorites: &[FavoriteEntry],
        speed: crate::settings::IndexerSpeedProfile,
        activity_gate: Arc<ActivityGate>,
        excluded_roots: Vec<std::path::PathBuf>,
        similar_notifier: Option<crate::similar_index::SimilarIndexNotifier>,
        similar_passwords: Option<crate::pdf_passwords::PdfPasswordStore>,
        progress: Option<StartupProgressHook>,
        skip_offline_change_scan: bool,
    ) -> StartupInitOutcome {
        let data_dir = crate::data_dir::get();
        let (meta_db, fts) =
            match open_stores_with_rebuild_sync(&data_dir, "IndexerManager", progress.as_ref()) {
                Ok(stores) => stores,
                Err(StoreOpenFailure::RebuildDeferred) => {
                    return StartupInitOutcome::Failed {
                        user_message:
                            "全文検索索引を再構築できませんでした。次回起動時に再試行します"
                                .to_string(),
                    };
                }
                Err(StoreOpenFailure::Unavailable) => return StartupInitOutcome::Unavailable,
            };
        match Self::new_with_stores(
            meta_db,
            fts,
            favorites,
            speed,
            activity_gate,
            excluded_roots,
            similar_notifier,
            similar_passwords,
            progress,
            skip_offline_change_scan,
        ) {
            Some(manager) => StartupInitOutcome::Ready(manager),
            None => StartupInitOutcome::Unavailable,
        }
    }

    /// テスト用コンストラクタ: `data_dir` 配下に `fts_meta.db` / `fts_index/` を作って初期化する。
    ///
    /// 本番の `new()` と同じ reconciliation → supervisor spawn のパスをたどるので、
    /// 統合テストで notify-rs 監視や spawn_search の end-to-end を検証できる。
    /// `data_dir` は呼び出し側が tempdir を用意する想定。
    pub fn new_at(
        data_dir: &std::path::Path,
        favorites: &[FavoriteEntry],
        speed: crate::settings::IndexerSpeedProfile,
        activity_gate: Arc<ActivityGate>,
        excluded_roots: Vec<std::path::PathBuf>,
    ) -> Option<Self> {
        std::fs::create_dir_all(data_dir).ok();
        let (meta_db, fts) =
            match open_stores_with_rebuild_sync(data_dir, "IndexerManager(test)", None) {
                Ok(stores) => stores,
                Err(_) => return None,
            };
        Self::new_with_stores(
            meta_db,
            fts,
            favorites,
            speed,
            activity_gate,
            excluded_roots,
            None,
            None,
            None,
            false,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_at_with_similar_for_test(
        data_dir: &std::path::Path,
        gate: Arc<ActivityGate>,
        similar: crate::similar_index::SimilarIndexNotifier,
        passwords: crate::pdf_passwords::PdfPasswordStore,
    ) -> Self {
        std::fs::create_dir_all(data_dir).unwrap();
        let (meta, fts) = open_stores_with_rebuild_sync(data_dir, "S3 fanout", None).unwrap();
        Self::new_with_stores(
            meta,
            fts,
            &[],
            crate::settings::IndexerSpeedProfile::default(),
            gate,
            Vec::new(),
            Some(similar),
            Some(passwords),
            None,
            false,
        )
        .unwrap()
    }

    /// `new` / `new_at` 共通の本体。stores を受け取って reconciliation + supervisor spawn を行う。
    fn new_with_stores(
        meta_db: Arc<FtsMetaDb>,
        fts: Arc<FtsIndex>,
        favorites: &[FavoriteEntry],
        speed: crate::settings::IndexerSpeedProfile,
        activity_gate: Arc<ActivityGate>,
        excluded_roots: Vec<std::path::PathBuf>,
        similar_notifier: Option<crate::similar_index::SimilarIndexNotifier>,
        similar_passwords: Option<crate::pdf_passwords::PdfPasswordStore>,
        progress: Option<StartupProgressHook>,
        skip_offline_change_scan: bool,
    ) -> Option<Self> {
        // IndexWriter は dispatcher に owner として渡す (Tantivy は 1 Index 1 writer 制約)。
        // dispatcher が常駐スレッドで処理するので、reconciliation も submit ベースで行う。
        if let Some(p) = progress.as_ref() {
            p("インデックスライターを初期化中…");
        }
        let t_writer = std::time::Instant::now();
        let raw_writer = match fts.writer() {
            Ok(w) => w,
            Err(e) => {
                crate::logger::log(format!("IndexerManager: writer init failed: {e}"));
                return None;
            }
        };
        crate::perf::emit_ms("startup", "fts_writer_init", 0, t_writer);
        let permits = speed.io_permits().max(1); // 0 は GlobalIoSemaphore で panic するので防御
        crate::logger::log(format!(
            "IndexerManager: speed profile = {:?} → io_permits = {permits}",
            speed
        ));
        let io_sem = Arc::new(GlobalIoSemaphore::new(permits));

        let writer =
            crate::fts_writer_dispatcher::FtsWriterDispatcher::start(raw_writer, Arc::clone(&fts));

        // === 起動時 reconciliation → supervisor spawn を同じ worker に渡す ===
        // supervisor が走る前に status != ok の残留行を整理する。dispatcher 経由で
        // Interactive 優先度で submit する (起動直後で他ジョブはほぼ無い)。
        if let Some(p) = progress.as_ref() {
            p("アイテム索引を整理中…");
        }
        let runtime = crate::metadata_reconfiguration::Runtime::start(
            crate::metadata_reconfiguration::Stores {
                meta: Arc::clone(&meta_db),
                fts: Arc::clone(&fts),
                writer: Arc::clone(&writer),
                io: Arc::clone(&io_sem),
                gate: Arc::clone(&activity_gate),
                similar: similar_notifier.clone(),
            },
            {
                let mut config = crate::metadata_reconfiguration::Configuration::new(
                    favorites,
                    excluded_roots.clone(),
                );
                config.similar_passwords = similar_passwords;
                config.skip_offline_change_scan = skip_offline_change_scan;
                config
            },
        )
        .ok()?;
        let mgr = IndexerManager {
            meta_db,
            fts,
            writer,
            io_sem,
            excluded_roots,
            runtime,
            startup_diag: StartupDiag {
                io_permits: permits,
                ..StartupDiag::default()
            },
        };
        Some(mgr)
    }

    /// 起動 overlay を閉じた後に呼ぶ。`fts_meta.db` の housekeeping (VACUUM) を
    /// 別スレッドで走らせる。数 GB DB で数分かかりうるため起動経路から外している。
    /// Mutex 取得で ingest と自動的に直列化される。
    pub fn spawn_housekeeping(&self, data_dir: &std::path::Path) {
        let meta = Arc::clone(&self.meta_db);
        let db_path = data_dir.join("fts_meta.db");
        std::thread::Builder::new()
            .name("fts-housekeeping".to_string())
            .spawn(move || {
                meta.run_housekeeping_if_needed(&db_path);
            })
            .ok();
    }

    /// 現在のお気に入り構成を worker に提出する。
    /// 重なるグループの停止・join・掃除・再作成は worker で直列に行う。
    /// UI スレッドは I/O・join・DB lock を待たない。
    pub fn sync_with_favorites(&mut self, favorites: &[FavoriteEntry]) {
        self.sync_with_configuration(favorites, self.excluded_roots.clone());
    }
    pub fn sync_with_configuration(
        &mut self,
        favorites: &[FavoriteEntry],
        excluded: Vec<std::path::PathBuf>,
    ) {
        self.excluded_roots = excluded.clone();
        self.runtime
            .submit(crate::metadata_reconfiguration::Configuration::new(
                favorites, excluded,
            ));
    }
    pub fn sync_with_configuration_and_passwords(
        &mut self,
        favorites: &[FavoriteEntry],
        excluded: Vec<std::path::PathBuf>,
        passwords: crate::pdf_passwords::PdfPasswordStore,
    ) {
        self.excluded_roots = excluded.clone();
        let mut config = crate::metadata_reconfiguration::Configuration::new(favorites, excluded);
        config.similar_passwords = Some(passwords);
        self.runtime.submit(config);
    }
    pub fn all_stats(&self) -> Vec<SupervisorStatsView> {
        let view = self.runtime.shared.state.lock().unwrap();
        view.controls
            .iter()
            .map(|(id, h)| {
                let f = view.favorites.iter().find(|f| f.id == *id);
                SupervisorStatsView {
                    favorite_id: *id,
                    favorite_name: f.map(|f| f.name.clone()).unwrap_or_default(),
                    favorite_path: f.map(|f| f.path.clone()).unwrap_or_default(),
                    stats: h.snapshot_stats(),
                }
            })
            .collect()
    }
    pub fn request_full_rescan(&self, id: Uuid) {
        if let Some(h) = self.runtime.shared.state.lock().unwrap().controls.get(&id) {
            h.request_full_rescan();
        }
    }
    /// 全体確認は構成採用後に metadata Full と similar Manual 1回を owner から発行する。
    pub fn request_shared_full_check(&self) {
        self.runtime.request_full_check();
    }
    #[cfg(test)]
    pub(crate) fn full_check_requested_for_test(&self) -> bool {
        self.runtime.full_check_requested_for_test()
    }
    #[cfg(test)]
    pub(crate) fn wait_full_check_dispatched_for_test(&self) {
        self.runtime.wait_full_check_dispatched_for_test();
    }
    pub fn take_notifications(&self) -> Vec<&'static str> {
        std::mem::take(&mut self.runtime.shared.state.lock().unwrap().notifications)
    }
    /// S3 must bypass startup markers for these owner roots.
    pub fn must_scan_owner_roots(&self) -> std::collections::HashSet<String> {
        self.runtime
            .shared
            .state
            .lock()
            .unwrap()
            .must_scan_roots
            .clone()
    }

    /// Ctrl+G 検索を別スレッドで起動する。
    /// `favorite_ids` は `auto_index_metadata = true` な favorite の UUID (IndexerManager が
    /// 実際に supervisor を立てているものに限られる)。
    /// `scope` はタイプ / 検索対象ドロップダウンの選択 (§19)。既定は全開放。
    ///
    /// 戻り値の `SearchHandle` を drop すると自動的に cancel が立つ (受信側の `rx` drop で
    /// 送信側が break することに依存)。明示的に cancel したい場合は `handle.cancel.store(true)`。
    pub fn spawn_search(
        &self,
        query: String,
        favorite_ids: Vec<Uuid>,
        scope: crate::global_search::SearchScope,
        wake: Option<crate::global_search::SearchWake>,
    ) -> SearchHandle {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx): (Sender<SearchStreamEvent>, Receiver<SearchStreamEvent>) =
            crossbeam_channel::unbounded();
        let fts = Arc::clone(&self.fts);
        let cancel_cl = Arc::clone(&cancel);

        std::thread::Builder::new()
            .name("ctrl-g-search".to_string())
            .spawn(move || {
                crate::global_search::run(
                    &query,
                    &favorite_ids,
                    &scope,
                    &fts,
                    &cancel_cl,
                    &tx,
                    wake.as_deref(),
                );
            })
            .ok();

        SearchHandle { cancel, rx }
    }

    /// favorite 数を返す (stats UI 用)。
    pub fn supervisor_count(&self) -> usize {
        self.runtime.shared.state.lock().unwrap().controls.len()
    }

    /// 全 supervisor が初期スキャンを完了しており、現在 full scan を実行していないか。
    /// supervisor 数 0 (自動索引が有効なお気に入りなし) でも true を返す。
    /// `spawn_housekeeping` の起動タイミングを「初回 ingest が落ち着いてから」に揃える
    /// ために使う (Codex 指摘)。
    pub fn all_supervisors_idle(&self) -> bool {
        let view = self.runtime.shared.state.lock().unwrap();
        !view.busy
            && view.pending.is_none()
            && view.controls.values().all(|h| {
                let s = h.snapshot_stats();
                s.initial_scan_done && !s.in_full_scan
            })
    }

    /// `Arc<FtsMetaDb>` を clone して返す。
    /// 検索 worker が status 確認・管理メタ取得のために使う
    /// (INDEX_VERSION=5 以降、原文は Tantivy 側にあるので fts_meta は管理メタ専用)。
    pub fn clone_fts_meta(&self) -> Arc<FtsMetaDb> {
        Arc::clone(&self.meta_db)
    }

    /// `Arc<FtsIndex>` を clone して返す。
    pub fn clone_fts_index(&self) -> Arc<FtsIndex> {
        Arc::clone(&self.fts)
    }

    /// 共有 IndexWriter をラップした `FtsWriterDispatcher` の Arc を clone して返す
    /// Tantivy は 1 Index につき writer 1 本しか許さないので、
    /// worker が独自に `fts.writer()` を呼ぶと LockBusy で失敗する。
    /// 必ずこの dispatcher 経由で `upsert` / `commit` を submit する (priority 指定可能)。
    pub fn clone_shared_writer(&self) -> Arc<crate::fts_writer_dispatcher::FtsWriterDispatcher> {
        Arc::clone(&self.writer)
    }

    /// 起動時 reconciliation または構成変更が進行中か (UI 表示用)。
    pub fn is_reconciling(&self) -> bool {
        let view = self.runtime.shared.state.lock().unwrap();
        view.busy || view.pending.is_some()
    }

    /// 起動時 reconciliation の結果 (UI 診断表示用)。
    pub fn startup_diag(&self) -> StartupDiag {
        let view = self.runtime.shared.state.lock().unwrap();
        StartupDiag {
            reconciliation_ms: view.reconciliation_ms,
            failed_cleaned: view.failed_cleaned,
            ..self.startup_diag
        }
    }

    /// v0.9: トレイ常駐中に I/O 並列度を強制的に 1 permit 相当へ絞る / 解除する。
    /// ウィンドウ非表示時に true、復帰時に false。
    pub fn set_io_throttled(&self, throttled: bool) {
        self.io_sem.set_throttled(throttled);
    }

    /// v0.9: `GlobalIoSemaphore` の Arc を取得する (トレイスレッドから直接 throttle 切替するため)。
    pub fn io_sem(&self) -> Arc<GlobalIoSemaphore> {
        Arc::clone(&self.io_sem)
    }
}

impl Drop for IndexerManager {
    fn drop(&mut self) {
        self.runtime.shutdown();
        let writer = Arc::clone(&self.writer);
        let fts = Arc::clone(&self.fts);
        let _ = std::thread::Builder::new()
            .name("fts-finalize".into())
            .spawn(move || {
                let _ = writer.commit(
                    true,
                    crate::fts_writer_dispatcher::WriterPriority::Background,
                );
                drop(writer);
                drop(fts);
            });
    }
}

#[derive(Clone, Debug)]
pub struct SupervisorStatsView {
    pub favorite_id: Uuid,
    pub favorite_name: String,
    pub favorite_path: std::path::PathBuf,
    pub stats: SupervisorStats,
}

// test-only としてのみ残している。
#[cfg(test)]
fn spawn_reconciliation(
    meta_db: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    favorites: Vec<FavoriteEntry>,
    done_flag: Arc<AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    std::thread::Builder::new()
        .name("fts-reconciliation".to_string())
        .spawn(move || {
            let writer_result = fts.writer();
            let result = match writer_result {
                Ok(mut w) => run_reconciliation(&meta_db, &fts, &mut w, &favorites),
                Err(e) => Err(format!("fts writer init: {e}")),
            };
            if let Err(e) = result {
                crate::logger::log(format!("reconciliation: failed: {e}"));
            }
            done_flag.store(false, Ordering::SeqCst);
        })
        .ok();
}

/// dispatcher 経由 reconciliation。delete + commit を 1 つの Batch で送る。
/// 起動直後で他ジョブはほぼ無いので Background 優先度でも即座に処理される。
/// Failed 行は「前回 ingest が失敗した path」なので、Tantivy 側を念のため
/// delete してから SQLite 行も物理削除する。次回 supervisor の walker が
/// "DB になし" として検出して再 ingest 候補に拾う。
///
/// `list_not_ok_paths` をお気に入りごとに回すと `idx_files_fav_kind` で
/// post-filter 化されてお気に入り配下の全行 (実測 65 万行で 1.1 秒) を読む。
/// `list_not_ok_paths_for_favorites` の 1 クエリ化で部分インデックス
/// `idx_files_status` (status != 0) が効き 17ms 程度に収まる。
pub(crate) fn run_reconciliation_via_dispatcher(
    meta_db: &FtsMetaDb,
    fts: &FtsIndex,
    writer: &crate::fts_writer_dispatcher::FtsWriterDispatcher,
    favorites: &[FavoriteEntry],
) -> Result<ReconciliationReport, String> {
    use crate::fts_writer_dispatcher::WriterPriority;
    let mut report = ReconciliationReport::default();
    let target_favs = crate::metadata_ownership::metadata_ownership(favorites, &[]).effective_ids();
    let not_ok = meta_db
        .list_not_ok_paths_for_favorites(&target_favs)
        .map_err(|e| format!("list_not_ok_paths_for_favorites: {e}"))?;
    // Failed cleanup must invalidate the owner before this startup decides to reuse it.
    let failed_ids: std::collections::HashSet<_> = not_ok.iter().map(|(_, id, _)| *id).collect();
    let failed_roots: std::collections::HashSet<_> = favorites
        .iter()
        .filter(|favorite| failed_ids.contains(&favorite.id))
        .map(|favorite| crate::metadata_ownership::root_key(&favorite.path))
        .collect();
    for root in failed_roots {
        meta_db
            .clear_scanned_once_for_root(&root)
            .map_err(|error| format!("reconciliation clear scan marker: {error}"))?;
    }
    let deletes: Vec<String> = not_ok.iter().map(|(p, _, _)| p.clone()).collect();
    let _ = fts;
    if !deletes.is_empty() {
        let deletes_for_sqlite = deletes.clone();
        writer
            .batch(vec![], deletes, true, true, WriterPriority::Background)
            .map_err(|e| format!("reconciliation batch: {e}"))?;
        meta_db
            .delete_paths(&deletes_for_sqlite)
            .map_err(|e| format!("reconciliation delete_paths: {e}"))?;
        report.failed_cleaned = deletes_for_sqlite.len();
    }
    crate::logger::log(format!(
        "reconciliation done: failed cleaned = {}",
        report.failed_cleaned
    ));
    Ok(report)
}

/// 旧版: 直接 `IndexWriter` を受け取る reconciliation。dispatcher 化後はテスト
/// (`spawn_reconciliation` + 単体テスト 3 件) でのみ使う。
#[cfg(test)]
fn run_reconciliation(
    meta_db: &FtsMetaDb,
    fts: &FtsIndex,
    writer: &mut tantivy::IndexWriter,
    favorites: &[FavoriteEntry],
) -> Result<ReconciliationReport, String> {
    let mut report = ReconciliationReport::default();
    let fields = fts.fields();

    for fav in favorites {
        if !fav.auto_index_metadata {
            continue;
        }
        let not_ok = meta_db
            .list_not_ok_paths(fav.id)
            .map_err(|e| format!("list_not_ok_paths: {e}"))?;
        for (path, _status) in not_ok {
            crate::fts_index::delete_doc(writer, fields, &path);
            if let Err(e) = meta_db.delete_paths(&[path.clone()]) {
                crate::logger::log(format!("reconciliation: delete_paths {path} failed: {e}"));
            }
            report.failed_cleaned += 1;
        }
    }
    writer.commit().map_err(|e| format!("writer.commit: {e}"))?;
    crate::logger::log(format!(
        "reconciliation done: failed cleaned = {}",
        report.failed_cleaned
    ));
    Ok(report)
}

#[derive(Default, Debug, Clone)]
pub(crate) struct ReconciliationReport {
    pub(crate) failed_cleaned: usize,
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_index::{Container, IndexDoc, IndexKind, QueryFilters, upsert_doc};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn test_manager(
        meta: Arc<FtsMetaDb>,
        fts: Arc<FtsIndex>,
        writer: Arc<crate::fts_writer_dispatcher::FtsWriterDispatcher>,
        io: Arc<GlobalIoSemaphore>,
        gate: Arc<ActivityGate>,
        similar: Option<crate::similar_index::SimilarIndexNotifier>,
    ) -> IndexerManager {
        let runtime = crate::metadata_reconfiguration::Runtime::start(
            crate::metadata_reconfiguration::Stores {
                meta: Arc::clone(&meta),
                fts: Arc::clone(&fts),
                writer: Arc::clone(&writer),
                io: Arc::clone(&io),
                gate: Arc::clone(&gate),
                similar: similar.clone(),
            },
            crate::metadata_reconfiguration::Configuration::new(&[], Vec::new()),
        )
        .unwrap();
        IndexerManager {
            meta_db: meta,
            fts,
            writer,
            io_sem: io,
            excluded_roots: Vec::new(),
            runtime,
            startup_diag: StartupDiag::default(),
        }
    }

    fn mk_fav(name: &str, path: &std::path::Path, metadata: bool) -> FavoriteEntry {
        let mut fav = FavoriteEntry::new(name.to_string(), path.to_path_buf());
        fav.auto_index_metadata = metadata;
        fav
    }

    fn seed_v9_meta(data_dir: &std::path::Path, with_row: bool) {
        let db_path = data_dir.join("fts_meta.db");
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        if with_row {
            db.upsert_meta_ok(
                "c:/legacy.png",
                Uuid::new_v4(),
                std::path::Path::new("C:/"),
                IndexKind::Image,
                1,
                1,
            )
            .unwrap();
        }
        drop(db);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE files SET index_version = 9", [])
            .unwrap();
        conn.execute_batch("DROP TABLE index_state; PRAGMA user_version = 9;")
            .unwrap();
    }

    #[test]
    fn recreated_fts_store_resets_matching_inventory_and_restores_docs_same_startup() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("pictures");
        std::fs::create_dir(&root).unwrap();
        image::RgbImage::new(2, 2)
            .save(root.join("restored.png"))
            .unwrap();
        let favorite = mk_fav("restore", &root, true);
        let key = crate::metadata_ownership::root_key(&root);
        let manager = IndexerManager::new_at(
            tmp.path(),
            std::slice::from_ref(&favorite),
            crate::settings::IndexerSpeedProfile::default(),
            Arc::new(ActivityGate::new(0)),
            vec![],
        )
        .unwrap();
        wait_until("first complete marker", || {
            manager.meta_db.scanned_once(&key).unwrap().is_some()
        });
        assert_eq!(manager.fts.searcher().num_docs(), 1);
        let mut manager = manager;
        manager.runtime.shutdown();
        let writer = Arc::downgrade(&manager.writer);
        let fts_weak = Arc::downgrade(&manager.fts);
        drop(manager);
        wait_until("store finalizer released", || {
            writer.strong_count() == 0 && fts_weak.strong_count() == 0
        });
        // Remove only the derived full-text store; unchanged stamps remain in the released DB.
        std::fs::remove_dir_all(tmp.path().join("fts_index")).unwrap();
        let (meta, fts) = open_stores_with_rebuild_sync(tmp.path(), "S3 recreation", None).unwrap();
        assert_eq!(meta.scanned_once(&key).unwrap(), None);
        assert!(
            meta.get(&crate::search_index_db::normalize_path(
                &root.join("restored.png")
            ))
            .unwrap()
            .is_none()
        );
        let manager = IndexerManager::new_with_stores(
            meta,
            fts,
            std::slice::from_ref(&favorite),
            crate::settings::IndexerSpeedProfile::default(),
            Arc::new(ActivityGate::new(0)),
            vec![],
            None,
            None,
            None,
            true,
        )
        .unwrap();
        wait_until("restored complete marker", || {
            manager.meta_db.scanned_once(&key).unwrap().is_some()
        });
        assert_eq!(manager.fts.searcher().num_docs(), 1);
        wait_until("restored supervisor complete", || {
            manager
                .all_stats()
                .iter()
                .any(|entry| entry.stats.ingested_ok == 1 && entry.stats.initial_scan_done)
        });
    }

    #[test]
    fn startup_failed_cleanup_invalidates_only_affected_root_before_skip_decision() {
        let tmp = TempDir::new().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("meta.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let favorite = mk_fav("failed", std::path::Path::new("C:/Images/"), true);
        let key = crate::metadata_ownership::root_key(&favorite.path);
        let fingerprint = crate::indexer_supervisor::fts_scan_fingerprint(
            &favorite.path,
            favorite.id,
            &[],
            &crate::indexer_supervisor::fts_scan_extensions(),
        );
        meta.mark_scanned_once(&key, &fingerprint).unwrap();
        meta.mark_scanned_once("c:/unrelated", "untouched").unwrap();
        for path in ["c:/images/a.jpg", "c:/images/b.jpg"] {
            meta.upsert_meta_ok(path, favorite.id, &favorite.path, IndexKind::Image, 1, 1)
                .unwrap();
            meta.mark_failed(path).unwrap();
        }
        assert!(crate::indexer_supervisor::can_skip_initial_scan(
            &meta,
            true,
            false,
            &favorite.path,
            favorite.id,
            &[]
        ));
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            Arc::clone(&fts),
        );
        let report = run_reconciliation_via_dispatcher(
            &meta,
            &fts,
            &writer,
            std::slice::from_ref(&favorite),
        )
        .unwrap();
        assert_eq!(report.failed_cleaned, 2);
        assert_eq!(meta.scanned_once(&key).unwrap(), None);
        assert_eq!(
            meta.scanned_once("c:/unrelated").unwrap().as_deref(),
            Some("untouched")
        );
        assert!(!crate::indexer_supervisor::can_skip_initial_scan(
            &meta,
            true,
            false,
            &favorite.path,
            favorite.id,
            &[]
        ));
    }

    fn seed_valid_fts_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
        let fts_dir = data_dir.join("fts_index");
        drop(FtsIndex::open_at(&fts_dir).unwrap());
        let sentinel = fts_dir.join("v9-sentinel.txt");
        std::fs::write(&sentinel, b"old-index").unwrap();
        sentinel
    }

    #[test]
    fn v9_to_v10_rebuild_wipes_tantivy_and_clears_pending_even_when_files_empty() {
        for with_row in [true, false] {
            let tmp = TempDir::new().unwrap();
            seed_v9_meta(tmp.path(), with_row);
            let sentinel = seed_valid_fts_dir(tmp.path());

            let (meta, fts) =
                open_stores_with_rebuild_sync(tmp.path(), "IndexerManager(migration-test)", None)
                    .expect("v9 migration succeeds");
            assert!(!sentinel.exists(), "old Tantivy directory must be wiped");
            assert!(meta.get("c:/legacy.png").unwrap().is_none());
            assert!(!meta.tantivy_rebuild_pending().unwrap());
            assert_eq!(fts.searcher().num_docs(), 0);

            let conn = rusqlite::Connection::open(tmp.path().join("fts_meta.db")).unwrap();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, crate::fts_meta::INDEX_VERSION);
        }
    }

    #[test]
    fn missing_meta_with_existing_tantivy_is_wiped_before_open() {
        let tmp = TempDir::new().unwrap();
        let sentinel = seed_valid_fts_dir(tmp.path());
        assert!(!tmp.path().join("fts_meta.db").exists());

        let (meta, _fts) =
            open_stores_with_rebuild_sync(tmp.path(), "IndexerManager(orphan-index-test)", None)
                .expect("orphan Tantivy migration succeeds");
        assert!(!sentinel.exists());
        assert!(!meta.tantivy_rebuild_pending().unwrap());
    }

    #[test]
    fn failed_wipe_keeps_pending_and_retry_clears_it_without_opening_old_index() {
        let tmp = TempDir::new().unwrap();
        seed_v9_meta(tmp.path(), true);
        let sentinel = seed_valid_fts_dir(tmp.path());

        let first = open_stores_with_rebuild_sync_using(
            tmp.path(),
            "IndexerManager(wipe-failure-test)",
            None,
            |_path| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected wipe failure",
                ))
            },
        );
        assert!(matches!(first, Err(StoreOpenFailure::RebuildDeferred)));
        assert!(
            sentinel.exists(),
            "failed wipe must not open/mutate old index"
        );

        let after_failure = FtsMetaDb::open_at(&tmp.path().join("fts_meta.db")).unwrap();
        assert!(after_failure.tantivy_rebuild_pending().unwrap());
        assert!(after_failure.get("c:/legacy.png").unwrap().is_none());
        drop(after_failure);

        let (meta, _fts) =
            open_stores_with_rebuild_sync(tmp.path(), "IndexerManager(wipe-retry-test)", None)
                .expect("next startup retries the pending wipe");
        assert!(!sentinel.exists());
        assert!(!meta.tantivy_rebuild_pending().unwrap());
    }

    #[test]
    fn constructor_without_similar_bridge_ignores_saved_similar_flag_but_keeps_metadata_watcher() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        let similar_db_path = crate::similar_db::SimilarDb::db_path_at(tmp.path());
        let similar_base_path = crate::similar_search_array::base_path(tmp.path());
        let db_sentinel = b"paused-invalid-similar-db";
        let base_sentinel = b"paused-invalid-similar-base";
        std::fs::write(&similar_db_path, db_sentinel).unwrap();
        std::fs::write(&similar_base_path, base_sentinel).unwrap();
        let mut favorite = mk_fav("paused", &root, false);
        favorite.auto_index_similar = true;

        let mut manager = IndexerManager::new_at(
            tmp.path(),
            &[favorite.clone()],
            crate::settings::IndexerSpeedProfile::default(),
            Arc::new(ActivityGate::new(0)),
            Vec::new(),
        )
        .expect("ordinary metadata stores still initialize");

        assert_eq!(manager.supervisor_count(), 0);
        assert_eq!(std::fs::read(&similar_db_path).unwrap(), db_sentinel);
        assert_eq!(std::fs::read(&similar_base_path).unwrap(), base_sentinel);

        favorite.auto_index_metadata = true;
        manager.sync_with_favorites(&[favorite]);
        wait_until("metadata supervisor adopted", || !manager.is_reconciling());
        assert_eq!(
            manager.supervisor_count(),
            1,
            "the ordinary metadata watcher remains available"
        );
        assert_eq!(std::fs::read(&similar_db_path).unwrap(), db_sentinel);
        assert_eq!(std::fs::read(&similar_base_path).unwrap(), base_sentinel);
    }

    // run_reconciliation の単体テスト。IndexerManager::new は APPDATA に
    // 依存するので、ここでは reconciliation 関数を直接テストする。

    #[test]
    fn reconciliation_cleans_failed_rows() {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap();
        let fts = FtsIndex::open_at(&tmp.path().join("fts")).unwrap();
        let fav_root = tmp.path().join("a");
        std::fs::create_dir_all(&fav_root).unwrap();

        let fav = mk_fav("A", &fav_root, true);

        meta.upsert_meta_ok("c:/a/1.jpg", fav.id, &fav_root, IndexKind::Image, 1, 1)
            .unwrap();
        meta.mark_failed("c:/a/1.jpg").unwrap();
        {
            let mut w = fts.writer().unwrap();
            upsert_doc(
                &w,
                fts.fields(),
                &IndexDoc {
                    path: "c:/a/1.jpg".into(),
                    container: Container::Fs,
                    zip_entry: String::new(),
                    favorite_id: fav.id,
                    kind: IndexKind::Image,
                    mtime: 1,
                    file_size: 1,
                    norms: crate::ingest_text::PerSourceText {
                        name: "1.jpg txt".into(),
                        ..Default::default()
                    },
                },
            )
            .unwrap();
            w.commit().unwrap();
        }

        let mut rw = fts.writer().unwrap();
        let r = run_reconciliation(&meta, &fts, &mut rw, &[fav.clone()]).unwrap();
        drop(rw);
        assert_eq!(r.failed_cleaned, 1);

        assert!(meta.get("c:/a/1.jpg").unwrap().is_none());

        fts.reload_reader().unwrap();
        let favs = [fav.id];
        let q = crate::fts_index::build_bigram_and_query(
            fts.fields(),
            &["txt"],
            &QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = fts.searcher();
        let hits = crate::fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 0, "Tantivy からも削除されているはず");
    }

    #[test]
    fn reconciliation_skips_favorites_without_metadata_flag() {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap();
        let fts = FtsIndex::open_at(&tmp.path().join("fts")).unwrap();
        let fav_root = tmp.path().join("a");
        std::fs::create_dir_all(&fav_root).unwrap();
        let fav = mk_fav("A", &fav_root, false);

        meta.upsert_meta_ok("c:/a/1.jpg", fav.id, &fav_root, IndexKind::Image, 1, 1)
            .unwrap();
        meta.mark_failed("c:/a/1.jpg").unwrap();

        let mut rw = fts.writer().unwrap();
        let r = run_reconciliation(&meta, &fts, &mut rw, &[fav]).unwrap();
        drop(rw);
        assert_eq!(
            r.failed_cleaned, 0,
            "OFF の favorite は reconciliation 対象外"
        );
        assert!(meta.get("c:/a/1.jpg").unwrap().is_some());
    }

    #[test]
    fn search_handle_runs_on_background_thread() {
        // IndexerManager 自体のフル初期化は APPDATA 依存で避けるが、
        // spawn_search 経路を確認するための最小 smoke test は別途作る。
        // ここでは global_search::run が crossbeam で event を送る契約の確認だけ。
        let tmp = TempDir::new().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let fav_id = Uuid::new_v4();

        let _ = meta;
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let fts_cl = Arc::clone(&fts);
        let cancel_cl = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let scope = crate::global_search::SearchScope::default();
            crate::global_search::run("dummy", &[fav_id], &scope, &fts_cl, &cancel_cl, &tx, None);
        });
        // 何らかの SearchStreamEvent が返ることを確認
        let ev = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(ev);
    }

    #[test]
    fn sync_respawns_on_path_change() {
        // Codex round-8 Must-fix #2 回帰:
        // 同じ UUID で path だけ変わった場合、watcher/walker が古い root を見続けないよう
        // drop + respawn されること (sync_with_favorites 直接の挙動を検証するため、
        // full IndexerManager::new ではなく手動で field を整える)
        let tmp = TempDir::new().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let io_sem = Arc::new(GlobalIoSemaphore::new(2));

        let root_old = tmp.path().join("old");
        let root_new = tmp.path().join("new");
        std::fs::create_dir_all(&root_old).unwrap();
        std::fs::create_dir_all(&root_new).unwrap();

        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            Arc::clone(&fts),
        );
        let mut mgr = test_manager(
            Arc::clone(&meta),
            Arc::clone(&fts),
            writer,
            Arc::clone(&io_sem),
            Arc::new(ActivityGate::new(1000)),
            None,
        );

        let mut fav = mk_fav("A", &root_old, true);
        // 初回 spawn
        mgr.sync_with_favorites(&[fav.clone()]);
        wait_until("first supervisor", || {
            mgr.supervisor_count() == 1 && !mgr.is_reconciling()
        });
        // path 変更 (id は同じ)
        fav.path = root_new.clone();
        mgr.sync_with_favorites(&[fav.clone()]);
        wait_until("new root supervisor", || {
            !mgr.is_reconciling() && mgr.all_stats().iter().any(|v| v.favorite_path == root_new)
        });
        assert_eq!(mgr.supervisor_count(), 1);
        drop(mgr);
    }

    /// 条件が満たされるまで待つ。締切は**ハングの検出だけ**を目的とし、処理の速さは測らない。
    ///
    /// 完全な lib テストは 7,500 件超を並列に走らせるので、worker が CPU を得るまでの待ちは
    /// 機械の混み具合で決まる。単体で 1.3 秒の初回照合が、混雑した完走で 20 秒の締切を一度
    /// 超えた。締切を機械の負荷が届かない位置へ置き、締切が鳴ったら本当に止まっている、と
    /// 読めるようにする。
    fn wait_until(what: &str, ready: impl FnMut() -> bool) {
        wait_until_with(|| what.to_owned(), ready);
    }

    /// 締切が鳴ったときの説明を、そのとき組み立てる版。待っている対象の状態を載せられる。
    fn wait_until_with(mut describe: impl FnMut() -> String, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(120);
        while !ready() {
            assert!(Instant::now() < deadline, "{}", describe());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn incremental_reconcile_uses_existing_supervisor_watcher_without_new_fulls() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("similar-only");
        std::fs::create_dir_all(&root).unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            Arc::clone(&fts),
        );
        let similar_data = tmp.path().join("similar");
        let similar = crate::similar_index::SimilarIndexManager::new(similar_data.clone());
        let activity_gate = Arc::new(ActivityGate::new(0));
        let mut manager = test_manager(
            meta,
            fts,
            writer,
            Arc::new(GlobalIoSemaphore::new(1)),
            Arc::clone(&activity_gate),
            Some(similar.notifier()),
        );
        let mut favorite = mk_fav("similar", &root, false);
        favorite.auto_index_similar = true;

        similar.configure(
            &[favorite.clone()],
            crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
            Some(Arc::clone(&activity_gate)),
            Vec::new(),
        );
        manager.sync_with_favorites(&[favorite.clone()]);

        wait_until("supervisor configuration", || {
            !manager.is_reconciling() && manager.supervisor_count() == 1
        });
        wait_until("similar-only watcher did not start", || {
            manager.all_supervisors_idle()
        });

        // 締切が鳴ったときに**何を待っていたのか**が分かるようにする。この待ちは以前から
        // 稀に鳴っており (本セッション最初の全体実行でも 20 秒で落ちた)、状態が分からないと
        // 次に鳴っても同じ推測を繰り返すことになる。
        let last_progress = std::cell::RefCell::new(String::new());
        wait_until_with(
            || {
                format!(
                    "initial similar reconciliation did not complete; last progress = {}",
                    last_progress.borrow()
                )
            },
            || {
                let progress = similar.progress();
                *last_progress.borrow_mut() = format!("{progress:?}");
                matches!(progress, crate::similar_index::IndexProgress::Complete(_))
            },
        );
        assert_eq!(similar.reconcile_job_counts_for_test(), (1, 0));

        // 同じ supervisor の watcher が追加と変更を類似索引へ渡すことを確認する。
        // 類似索引用の watcher を別に作る実装では、この結合テストを満たせない。
        let image_path = root.join("watched.png");
        image::RgbImage::from_fn(24, 24, |x, y| {
            image::Rgb([(x * 7) as u8, (y * 11) as u8, ((x + y) * 5) as u8])
        })
        .save(&image_path)
        .unwrap();
        let db_path = crate::similar_db::SimilarDb::db_path_at(&similar_data);
        let db = crate::similar_db::SimilarDb::open_at(&db_path).unwrap();
        let item_key = crate::similar_index::item_key_for_file(&image_path);
        wait_until("shared watcher did not reconcile an added image", || {
            db.load_search_rows(crate::similar_db::current_hash_version())
                .unwrap()
                .iter()
                .any(|row| row.item.item_key == item_key && row.item.width == 24)
        });

        image::RgbImage::from_fn(37, 31, |x, y| {
            image::Rgb([
                ((x * 17 + y * 3) % 251) as u8,
                ((y * 19 + x * 5) % 253) as u8,
                ((x * 13 + y * 23) % 255) as u8,
            ])
        })
        .save(&image_path)
        .unwrap();
        wait_until("shared watcher did not reconcile a changed image", || {
            db.load_search_rows(crate::similar_db::current_hash_version())
                .unwrap()
                .iter()
                .any(|row| row.item.item_key == item_key && row.item.width == 37)
        });

        let renamed_path = root.join("renamed.png");
        std::fs::rename(&image_path, &renamed_path).unwrap();
        let renamed_key = crate::similar_index::item_key_for_file(&renamed_path);
        wait_until("shared watcher did not reconcile both rename sides", || {
            let rows = db
                .load_search_rows(crate::similar_db::current_hash_version())
                .unwrap();
            rows.iter().all(|row| row.item.item_key != item_key)
                && rows.iter().any(|row| row.item.item_key == renamed_key)
        });
        std::fs::remove_file(&renamed_path).unwrap();
        wait_until("shared watcher did not remove the deleted image", || {
            db.load_search_rows(crate::similar_db::current_hash_version())
                .unwrap()
                .iter()
                .all(|row| row.item.item_key != renamed_key)
        });
        let (full_jobs, delta_jobs) = similar.reconcile_job_counts_for_test();
        assert_eq!(
            full_jobs, 1,
            "ordinary shared-watcher events must not restart the full reconcile"
        );
        assert!(
            delta_jobs >= 2,
            "add/change/rename/remove should converge through one or more delta batches"
        );

        // A metadata-only setting change replaces the shared supervisor while the Similar
        // configuration remains identical. The registration handoff owns one repair gap rather
        // than silently accepting the new watcher as an uninterrupted lifecycle.
        favorite.auto_index_metadata = true;
        manager.sync_with_favorites(&[favorite]);
        wait_until_with(
            || {
                format!(
                    "replacement watcher repair stalled: {:?}",
                    similar.progress()
                )
            },
            || {
                let (full_jobs, _) = similar.reconcile_job_counts_for_test();
                full_jobs >= 2
                    && matches!(
                        similar.progress(),
                        crate::similar_index::IndexProgress::Complete(_)
                    )
            },
        );
        assert_eq!(
            similar.reconcile_job_counts_for_test().0,
            2,
            "one shared-watcher replacement owns exactly one repair Full"
        );
    }

    #[test]
    fn incremental_reconcile_watch_recovery_repairs_metadata_and_similar_together() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("recovering-root");
        std::fs::create_dir_all(&root).unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            Arc::clone(&fts),
        );
        let similar_data = tmp.path().join("similar");
        let similar = crate::similar_index::SimilarIndexManager::new(similar_data.clone());
        let activity_gate = Arc::new(ActivityGate::new(0));
        let mut manager = test_manager(
            Arc::clone(&meta),
            fts,
            writer,
            Arc::new(GlobalIoSemaphore::new(1)),
            Arc::clone(&activity_gate),
            Some(similar.notifier()),
        );
        let mut favorite = mk_fav("recovering", &root, false);
        favorite.auto_index_similar = true;
        similar.configure(
            &[favorite.clone()],
            crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
            Some(Arc::clone(&activity_gate)),
            Vec::new(),
        );
        manager.sync_with_favorites(&[favorite.clone()]);
        wait_until("initial similar reconciliation did not complete", || {
            matches!(
                similar.progress(),
                crate::similar_index::IndexProgress::Complete(_)
            )
        });

        std::fs::remove_dir_all(&root).unwrap();
        favorite.auto_index_metadata = true;
        manager.sync_with_favorites(&[favorite]);
        wait_until_with(
            || {
                format!(
                    "watch unavailability was not observed: {:?}",
                    similar.progress()
                )
            },
            || {
                similar.reconcile_job_counts_for_test().0 >= 2
                    && matches!(
                        similar.progress(),
                        crate::similar_index::IndexProgress::Degraded { .. }
                    )
            },
        );

        std::fs::create_dir_all(&root).unwrap();
        let recovered = root.join("recovered.png");
        image::RgbImage::from_fn(29, 23, |x, y| {
            image::Rgb([(x * 3) as u8, (y * 5) as u8, ((x + y) * 7) as u8])
        })
        .save(&recovered)
        .unwrap();
        let recovered_key = crate::search_index_db::normalize_path(&recovered);
        let similar_key = crate::similar_index::item_key_for_file(&recovered);
        let db = crate::similar_db::SimilarDb::open_at(&crate::similar_db::SimilarDb::db_path_at(
            &similar_data,
        ))
        .unwrap();
        wait_until_with(
            || format!("shared watch recovery stalled: {:?}", similar.progress()),
            || {
                meta.get(&recovered_key).unwrap().is_some()
                    && db
                        .load_search_rows(crate::similar_db::current_hash_version())
                        .unwrap()
                        .iter()
                        .any(|row| row.item.item_key == similar_key)
                    && matches!(
                        similar.progress(),
                        crate::similar_index::IndexProgress::Complete(_)
                    )
            },
        );
        assert_eq!(
            similar.reconcile_job_counts_for_test().0,
            3,
            "initial, unavailable snapshot, and one recovered-gap Full are expected"
        );
    }

    #[test]
    fn spawn_reconciliation_flag_resets() {
        let tmp = TempDir::new().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let flag = Arc::new(AtomicBool::new(true));

        spawn_reconciliation(meta, fts, vec![], Arc::clone(&flag));
        // 完了で false に戻る
        wait_until("reconciliation flag did not reset", || {
            !flag.load(Ordering::SeqCst)
        });
    }
}
