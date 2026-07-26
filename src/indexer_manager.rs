//! `IndexerManager` — App 統合のための Supervisor 群管理 (docs/archive/search-metadata/search-expansion-design.md §3)。
//!
//! 全お気に入りの `SupervisorHandle` を束ねて、以下を提供する:
//!
//! - 起動時: `auto_index_metadata = true` のお気に入りに対して Supervisor を spawn
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
//!           └── auto_index_metadata=true のお気に入りに Supervisor を spawn
//!   ...
//!   App::update ループ:
//!     - all_stats() で進捗取得 (軽量な lock)
//!     - Ctrl+G 入力 → spawn_search()
//!     - お気に入り編集保存時 → sync_with_favorites()
//!   ...
//!   App::drop → IndexerManager::drop → 全 Supervisor drop
//! ```
//!
//! ## CLAUDE.md UI 応答性の遵守
//!
//! - `all_stats()`: 1 つの Mutex lock × N favorite。N=20 上限でも Mutex 1 回 × 1μs → 20μs で済む。
//!   毎フレーム呼んでも問題ない。
//! - `sync_with_favorites()`: Supervisor drop が発生する可能性あり。drop は内部で FsWatcher
//!   join (最大 ~250ms) を伴う。**環境設定ダイアログの OK ボタン押下時のみ** 呼ぶ方針 (毎フレーム不可)。
//! - `spawn_search()`: 別スレッド起動のみなので O(1)。
//! - `new()` の起動時 reconciliation: **同期実行** に変更済み (Codex round-8 Must-fix #1)。
//!   通常クラッシュ残留の僅かな行だけを処理するので 100ms 以下で完了する見込み。
//!   同期化により supervisor 群の spawn と writer 競合しなくなった。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crossbeam_channel::{Receiver, Sender};
use uuid::Uuid;

use crate::activity_gate::ActivityGate;
use crate::fts_index::FtsIndex;
use crate::fts_meta::FtsMetaDb;
use crate::global_search::SearchStreamEvent;
use crate::indexer_supervisor::{self, SupervisorHandle, SupervisorParams, SupervisorStats};
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
    /// UI 入力があると `bump` され、ingest ワーカーが unit of work の前にこれで待つ。
    /// `App::update` が ActivityGate を所有し、IndexerManager は `Arc` を受け取って保管する。
    activity_gate: Arc<ActivityGate>,
    /// アプリ管理下で生成する派生コンテンツなど、検索索引から除外する root。
    excluded_roots: Vec<std::path::PathBuf>,
    /// お気に入り UUID → Supervisor ハンドル
    supervisors: HashMap<Uuid, SupervisorHandle>,
    /// 有効化されていないお気に入りでも、お気に入り UUID → (name, path) を記憶しておく
    /// (stats UI で name を出すため)
    favorite_info: HashMap<Uuid, (String, std::path::PathBuf)>,
    /// reconciliation が進行中なら true (UI に "DB 初期化中" 表示用)
    pub reconciliation_in_progress: Arc<AtomicBool>,
    /// 起動時 reconciliation の診断情報 (UI 表示用)
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
/// このため fts_meta open → 旧 STORED tags を tags.db へ移行 → `rebuilt_on_open`
/// チェック → 必要なら `fts_index` 削除 → fts open の順で実行する。
///
/// 本番 `new()` とテスト用 `new_at()` の両方から呼ぶ (Codex P3 指摘: 旧コードでは
/// `new_at` が wipe を再現していなかったため、version bump の挙動を統合テストで検証できず
/// 本番と乖離していた)。
/// 起動進捗 (UI 側のオーバーレイに表示する短文) を更新するためのフック。
///
/// `IndexerManager::new` 内部で各 sub-step の前に呼ばれる。
/// `None` を渡せば従来の挙動。`Some` の場合は文字列を Mutex に書き込む。
pub type StartupProgressHook = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

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
) -> Option<(Arc<FtsMetaDb>, Arc<FtsIndex>)> {
    if let Some(p) = progress {
        p("アイテム索引データベースを開いています…");
    }
    let t_meta = std::time::Instant::now();
    let meta_db = match FtsMetaDb::open_at(&data_dir.join("fts_meta.db")) {
        Ok(db) => db,
        Err(e) => {
            crate::logger::log(format!("{log_tag}: FtsMetaDb open failed: {e}"));
            return None;
        }
    };
    crate::perf::emit_ms("startup", "fts_meta_open", 0, t_meta);
    let fts_dir = data_dir.join("fts_index");
    run_legacy_tantivy_tag_import(data_dir, &fts_dir, log_tag, progress);
    if meta_db.rebuilt_on_open() {
        if let Some(p) = progress {
            p("古いインデックスを削除しています…");
        }
        crate::logger::log(format!(
            "{log_tag}: fts_meta rebuilt → wiping Tantivy index dir {}",
            fts_dir.display()
        ));
        let t_wipe = std::time::Instant::now();
        if let Err(e) = std::fs::remove_dir_all(&fts_dir) {
            // Not-found は想定内。他は warn してそのまま続行 (次の open で
            // schema_is_stale 経路で再 wipe されることを期待)。
            if e.kind() != std::io::ErrorKind::NotFound {
                crate::logger::log(format!(
                    "{log_tag}: wipe fts_index failed: {e} (continuing)"
                ));
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
            return None;
        }
    };
    crate::perf::emit_ms("startup", "fts_index_open", 0, t_fts);
    Some((meta_db, fts))
}

impl IndexerManager {
    /// DB/index を開き、起動時 reconciliation → auto_index_metadata=true のお気に入りに
    /// Supervisor を spawn する。
    ///
    /// DB 初期化に失敗したら None (App 側は fts 機能なしで動作継続する)。
    ///
    /// **Codex round-8 Must-fix #1 反映**: 起動時 reconciliation は
    /// supervisors spawn の **前** に同期実行する。旧実装はバックグラウンド化していたが、
    /// reconciliation の IndexWriter ロック中に supervisor の writer 初期化が失敗し、
    /// supervisor thread が即 return する race があった。
    /// reconciliation は通常クラッシュ残留の僅かな行だけを処理するので、
    /// アプリ起動の許容範囲内 (通常 100ms 以下) で終わる。
    /// `progress` を渡すと各 sub-step (FtsMetaDb open / FtsIndex open / writer init /
    /// reconciliation / supervisor spawn) の前に短い進捗文字列が書き込まれる。
    /// 起動オーバーレイで状態を見せたい場合に渡す。`None` なら従来通り無音。
    pub fn new(
        favorites: &[FavoriteEntry],
        speed: crate::settings::IndexerSpeedProfile,
        activity_gate: Arc<ActivityGate>,
        excluded_roots: Vec<std::path::PathBuf>,
        progress: Option<StartupProgressHook>,
    ) -> Option<Self> {
        let data_dir = crate::data_dir::get();
        let (meta_db, fts) =
            match open_stores_with_rebuild_sync(&data_dir, "IndexerManager", progress.as_ref()) {
                Some(stores) => stores,
                None => return None,
            };
        Self::new_with_stores(
            meta_db,
            fts,
            favorites,
            speed,
            activity_gate,
            excluded_roots,
            progress,
        )
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
                Some(stores) => stores,
                None => return None,
            };
        Self::new_with_stores(
            meta_db,
            fts,
            favorites,
            speed,
            activity_gate,
            excluded_roots,
            None,
        )
    }

    /// `new` / `new_at` 共通の本体。stores を受け取って reconciliation + supervisor spawn を行う。
    fn new_with_stores(
        meta_db: Arc<FtsMetaDb>,
        fts: Arc<FtsIndex>,
        favorites: &[FavoriteEntry],
        speed: crate::settings::IndexerSpeedProfile,
        activity_gate: Arc<ActivityGate>,
        excluded_roots: Vec<std::path::PathBuf>,
        progress: Option<StartupProgressHook>,
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

        // === 起動時 reconciliation を先に同期実行 ===
        // supervisor が走る前に status != ok の残留行を整理する。dispatcher 経由で
        // Interactive 優先度で submit する (起動直後で他ジョブはほぼ無い)。
        if let Some(p) = progress.as_ref() {
            p("アイテム索引を整理中…");
        }
        let t_recon = std::time::Instant::now();
        let report = match run_reconciliation_via_dispatcher(&meta_db, &fts, &writer, favorites) {
            Ok(r) => r,
            Err(e) => {
                crate::logger::log(format!(
                    "IndexerManager: reconciliation failed (continuing anyway): {e}"
                ));
                ReconciliationReport::default()
            }
        };
        let reconciliation_ms = t_recon.elapsed().as_millis() as u64;
        crate::perf::emit_ms("startup", "fts_reconciliation", 0, t_recon);
        crate::logger::log(format!(
            "IndexerManager: reconciliation completed in {reconciliation_ms} ms"
        ));

        let mut mgr = IndexerManager {
            meta_db,
            fts,
            writer,
            io_sem,
            activity_gate,
            excluded_roots,
            supervisors: HashMap::new(),
            favorite_info: HashMap::new(),
            reconciliation_in_progress: Arc::new(AtomicBool::new(false)),
            startup_diag: StartupDiag {
                reconciliation_ms,
                failed_cleaned: report.failed_cleaned,
                io_permits: permits,
            },
        };
        // reconciliation 完了後に supervisor 群を起動 (writer 競合なし)
        if let Some(p) = progress.as_ref() {
            p("お気に入りの監視を起動中…");
        }
        mgr.sync_with_favorites(favorites);
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

    /// 現在のお気に入り一覧と supervisors を同期。
    /// - 新規 `auto_index_metadata = true` → spawn
    /// - 既存で OFF に切り替わった / 削除された → drop
    /// - 既存で ON のまま **かつ path 不変** → 維持
    /// - 既存で ON のまま **かつ path 変更** → drop + respawn (Codex round-8 Must-fix #2)
    ///
    /// **UI スレッドから呼ぶ時の注意**: Supervisor の drop は内部で thread join を伴うため、
    /// 多数の stop が発生する場面 (例: 全 OFF) ではブロックする可能性がある。
    /// 環境設定ダイアログの OK 押下時のような、ユーザが待ってもよいタイミングで呼ぶこと。
    pub fn sync_with_favorites(&mut self, favorites: &[FavoriteEntry]) {
        // path 変更の検出は favorite_info 更新 **前** に行う (旧 path と比較するため)
        let path_changed: std::collections::HashSet<Uuid> = favorites
            .iter()
            .filter_map(|f| {
                let old_path = self.favorite_info.get(&f.id).map(|(_, p)| p.clone())?;
                if old_path != f.path { Some(f.id) } else { None }
            })
            .collect();

        // favorite_info を最新化
        self.favorite_info.clear();
        for f in favorites {
            self.favorite_info
                .insert(f.id, (f.name.clone(), f.path.clone()));
        }

        // 削除 / OFF 化 / **path 変更** されたものを drop 対象に含める
        let current_on_ids: std::collections::HashSet<Uuid> = favorites
            .iter()
            .filter(|f| f.auto_index_metadata)
            .map(|f| f.id)
            .collect();
        let to_stop: Vec<Uuid> = self
            .supervisors
            .keys()
            .filter(|id| !current_on_ids.contains(id) || path_changed.contains(id))
            .copied()
            .collect();
        // dispatcher 化後 (commit 30338a3) も signal_stop → drain パターンを維持する:
        // 1 体ずつ drop すると、その supervisor が `dispatcher.batch().recv()` でブロック中の
        // sub-batch が完了するまで join できない (= sub-batch 1 個分の hang)。先に全員に
        // cancel を立てておけば、現在の sub-batch が終わった時点で各 supervisor が
        // 次のループ先頭で cancel を検出し、即 exit できる。
        for id in &to_stop {
            if let Some(handle) = self.supervisors.get(id) {
                crate::logger::log(format!(
                    "IndexerManager: signaling supervisor {id} to stop (removed / off / path changed)"
                ));
                handle.signal_stop();
            }
        }
        // **非同期 join** (2026-04 B): 各 supervisor の drop は `thread.join()` を待つため、
        // ingest の sub-batch (commit に数百 ms かかる) を抱えた supervisor を join すると
        // UI スレッドが丸ごとブロックする (計測で 1162ms のヒッチを観測)。
        // 既に上の `signal_stop()` で全員の cancel は立てているので、drop (= join) 自体は
        // バックグラウンドスレッドに逃がして UI は即 return させる。
        let handles_to_join: Vec<SupervisorHandle> = to_stop
            .into_iter()
            .filter_map(|id| self.supervisors.remove(&id))
            .collect();
        if !handles_to_join.is_empty() {
            let n = handles_to_join.len();
            crate::logger::log(format!(
                "IndexerManager: spawning joiner thread for {n} supervisor(s)"
            ));
            if let Err(e) = std::thread::Builder::new()
                .name("indexer-joiner".into())
                .spawn(move || {
                    for handle in handles_to_join {
                        let id = handle.favorite_id;
                        drop(handle);
                        crate::logger::log(format!(
                            "IndexerManager(joiner): supervisor {id} joined"
                        ));
                    }
                })
            {
                // spawn 失敗時は closure が現スレッドで drop される = 同期 join になる。
                // UI がブロックするが整合性は保たれる (稀なリソース枯渇時のフェイルセーフ)。
                crate::logger::log(format!(
                    "IndexerManager: joiner spawn failed, sync join instead: {e}"
                ));
            }
        }

        // 新規 ON を spawn (path 変更で drop したものも新 path で respawn される)
        for f in favorites {
            if !f.auto_index_metadata {
                continue;
            }
            if self.supervisors.contains_key(&f.id) {
                continue;
            }
            let handle = indexer_supervisor::spawn(
                SupervisorParams {
                    favorite_id: f.id,
                    favorite_root: f.path.clone(),
                    excluded_roots: self.excluded_roots.clone(),
                    enable_metadata_index: true,
                },
                Arc::clone(&self.meta_db),
                Arc::clone(&self.fts),
                Arc::clone(&self.writer),
                Arc::clone(&self.io_sem),
                Arc::clone(&self.activity_gate),
            );
            self.supervisors.insert(f.id, handle);
        }
    }

    /// 現在アクティブな全 supervisor の stats を取得 (UI 表示用)。
    /// 戻り値の順序は favorite 登録順ではないので、UI 側でソートすること。
    pub fn all_stats(&self) -> Vec<SupervisorStatsView> {
        self.supervisors
            .iter()
            .map(|(id, handle)| {
                let info = self.favorite_info.get(id).cloned();
                SupervisorStatsView {
                    favorite_id: *id,
                    favorite_name: info.as_ref().map(|(n, _)| n.clone()).unwrap_or_default(),
                    favorite_path: info.map(|(_, p)| p).unwrap_or_else(std::path::PathBuf::new),
                    stats: handle.snapshot_stats(),
                }
            })
            .collect()
    }

    /// 指定 favorite の Supervisor に手動 full-rescan を要求する。
    /// 見つからない (auto_index_metadata = false の) favorite は no-op。
    pub fn request_full_rescan(&self, favorite_id: Uuid) {
        if let Some(h) = self.supervisors.get(&favorite_id) {
            h.request_full_rescan();
        }
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
    ) -> SearchHandle {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx): (Sender<SearchStreamEvent>, Receiver<SearchStreamEvent>) =
            crossbeam_channel::unbounded();
        let fts = Arc::clone(&self.fts);
        let cancel_cl = Arc::clone(&cancel);

        std::thread::Builder::new()
            .name("ctrl-g-search".to_string())
            .spawn(move || {
                crate::global_search::run(&query, &favorite_ids, &scope, &fts, &cancel_cl, &tx);
            })
            .ok();

        SearchHandle { cancel, rx }
    }

    /// favorite 数を返す (stats UI 用)。
    pub fn supervisor_count(&self) -> usize {
        self.supervisors.len()
    }

    /// 全 supervisor が初期スキャンを完了しており、現在 full scan を実行していないか。
    /// supervisor 数 0 (auto_index_metadata=true のお気に入りなし) でも true を返す。
    /// `spawn_housekeeping` の起動タイミングを「初回 ingest が落ち着いてから」に揃える
    /// ために使う (Codex 指摘)。
    pub fn all_supervisors_idle(&self) -> bool {
        self.supervisors.values().all(|h| {
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

    /// 起動時 reconciliation が進行中か (UI インジケータ表示用)。
    /// 現状は同期実行で即 false に戻るが、将来 async 化したときに値が変わる余地を残す。
    pub fn is_reconciling(&self) -> bool {
        self.reconciliation_in_progress
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 起動時 reconciliation の結果 (UI 診断表示用)。
    pub fn startup_diag(&self) -> StartupDiag {
        self.startup_diag
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

    /// お気に入りの「メタ索引」チェックを OFF にした時のクリーンアップ。
    /// SQLite 行と Tantivy doc 両方を確実に消す。reconciliation は status=Failed の
    /// 行しか走査しないので、ここで Tantivy delete_term を出さないと孤児 doc が
    /// 残り続けてしまう。
    ///
    /// 呼び出し順序: **必ず `sync_with_favorites` より前に呼ぶ** こと。先に supervisor
    /// を drop すると writer が別スレッドに移ってしまうので、こちらの SQL DELETE 中に
    /// supervisor 側 ingest が走って race になる可能性がある (実害は限定的だが綺麗でない)。
    pub fn purge_favorite_metadata(&self, favorite_id: Uuid) -> usize {
        use crate::fts_writer_dispatcher::WriterPriority;
        let paths = match self.meta_db.list_all_paths_for_favorite(favorite_id) {
            Ok(p) => p,
            Err(e) => {
                crate::logger::log(format!(
                    "IndexerManager: purge_favorite_metadata({favorite_id}) list failed: {e}"
                ));
                return 0;
            }
        };
        if paths.is_empty() {
            return 0;
        }
        // Tantivy First: delete batch が失敗したら SQLite には触れない。
        // 次回のメタ ON/OFF 切替や reconciliation で再試行できるよう、SQLite の
        // 行を再試行の手がかりとして残しておく。
        if let Err(e) = self.writer.batch(
            vec![],
            paths.clone(),
            true,
            true,
            WriterPriority::Background,
        ) {
            crate::logger::log(format!(
                "IndexerManager: purge_favorite_metadata({favorite_id}) tantivy batch failed: {e} \
                 (SQLite rows preserved for retry)"
            ));
            return 0;
        }
        match self.meta_db.delete_all_for_favorite(favorite_id) {
            Ok(n) => {
                crate::logger::log(format!(
                    "IndexerManager: purge_favorite_metadata({favorite_id}) deleted {n} rows"
                ));
                n
            }
            Err(e) => {
                crate::logger::log(format!(
                    "IndexerManager: purge_favorite_metadata({favorite_id}) sqlite failed: {e}"
                ));
                0
            }
        }
    }
}

impl Drop for IndexerManager {
    fn drop(&mut self) {
        // STEP 1: 全 supervisor に同時に cancel シグナルを送る。
        //
        // **重要**: dispatcher 化後 (commit 30338a3) も同じ「全員に先に cancel → 順次 join」
        // パターンを維持する。各 supervisor は `dispatcher.batch().recv()` で sub-batch 完了
        // 待ちにブロックすることがあり、1 体ずつ drop すると現在処理中の sub-batch が
        // 終わるまで止まれない。先に全員の cancel を立てておけば、現 sub-batch 完了直後の
        // ループ先頭で cancel を検出して即 exit する。
        for (id, handle) in &self.supervisors {
            crate::logger::log(format!("IndexerManager: signaling supervisor {id} to stop"));
            handle.signal_stop();
        }

        // STEP 2: manager 全体で 4 秒だけ join を待つ。期限後の JoinHandle は detach し、
        // プロセス終了を supervisor 内の将来の長時間処理で塞がない。
        let shutdown_deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let supervisor_count = self.supervisors.len();
        let mut detached = 0usize;
        for (id, handle) in self.supervisors.drain() {
            crate::logger::log(format!("IndexerManager: joining supervisor {id}"));
            if !handle.join_until(shutdown_deadline) {
                detached += 1;
            }
        }
        let joined = supervisor_count - detached;
        if detached > 0 {
            crate::logger::log(format!(
                "IndexerManager: shutdown deadline reached; joined={joined}, detached={detached}"
            ));
        } else {
            crate::logger::log(format!(
                "IndexerManager: all supervisors joined within deadline ({joined})"
            ));
        }

        // STEP 3: join済み / detach済みにかかわらず共有writerへbest-effort commitを送る。
        // main threadでは待たない。cancel-aware waiter が abandoned job を
        // dispatcher queue に残していても、この clone が dispatcher の最終 Drop/join を
        // background 側で所有する。Tantivy First + 次回 3-way diff の不変条件は維持される。
        let writer = Arc::clone(&self.writer);
        if let Err(e) = std::thread::Builder::new()
            .name("indexer-writer-finalizer".into())
            .spawn(move || {
                if let Err(e) = writer.commit(
                    false,
                    crate::fts_writer_dispatcher::WriterPriority::Background,
                ) {
                    crate::logger::log(format!("IndexerManager: final writer commit failed: {e}"));
                } else {
                    crate::logger::log(
                        "IndexerManager: final writer commit completed (best effort)",
                    );
                }
            })
        {
            crate::logger::log(format!(
                "IndexerManager: writer finalizer spawn failed; skipping final commit: {e}"
            ));
        }
        // dispatcher 自身は Arc::strong_count が 0 になった時点で Drop → スレッド join される。
    }
}

/// UI 表示用の SupervisorStats + 名前/パス。
#[derive(Clone, Debug)]
pub struct SupervisorStatsView {
    pub favorite_id: Uuid,
    pub favorite_name: String,
    pub favorite_path: std::path::PathBuf,
    pub stats: SupervisorStats,
}

// -----------------------------------------------------------------------
// 起動時 reconciliation (§5.6.3)
// -----------------------------------------------------------------------

/// `status != ok` の行を掃除する reconciliation を別スレッドで走らせる。
///
/// - Ok: 対象外
/// - Pending: pending のまま残すと次回もフィルタから漏れる。supervisor 起動時の walker
///   が再 ingest するので、ここでは何もしないで良い (walker が "DB になし" と判定して追加)。
///   ただし Tantivy 側に残っている古い doc があれば整合性を取るため delete しておく。
/// - Failed: 永久リトライを避けるため、24 時間経っていない failed はスキップ (v1 では簡略化で全部再試行)
/// - Tombstone: tombstone として DB に残っているが Tantivy delete が commit されていない
///   可能性があるので、念のため Tantivy 側を delete してから purge する
///
/// v1 実装はシンプル: `list_not_ok_paths` で取った path について
///   - Pending → Tantivy delete_doc + DB row 削除 (次回 walker scan で再 ingest)
///   - Failed → 同じ (再試行)
///   - Tombstone → Tantivy delete_doc + purge_tombstone
/// 起動時 reconciliation を別スレッドで走らせる (v1 ではテスト専用)。
///
/// 本番経路では `IndexerManager::new` が `run_reconciliation` を同期実行する
/// (Codex round-8 Must-fix #1 対応)。この関数は AtomicBool 通知付き非同期版で、
/// 将来的な「実行中 reconciliation」UI 表示や定期再 reconciliation で使う余地を残すため
/// test-only としてのみ残している。
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
fn run_reconciliation_via_dispatcher(
    meta_db: &FtsMetaDb,
    fts: &FtsIndex,
    writer: &crate::fts_writer_dispatcher::FtsWriterDispatcher,
    favorites: &[FavoriteEntry],
) -> Result<ReconciliationReport, String> {
    use crate::fts_writer_dispatcher::WriterPriority;
    let mut report = ReconciliationReport::default();
    let target_favs: Vec<Uuid> = favorites
        .iter()
        .filter(|f| f.auto_index_metadata)
        .map(|f| f.id)
        .collect();
    let not_ok = meta_db
        .list_not_ok_paths_for_favorites(&target_favs)
        .map_err(|e| format!("list_not_ok_paths_for_favorites: {e}"))?;
    let deletes: Vec<String> = not_ok.iter().map(|(p, _, _)| p.clone()).collect();
    let _ = fts;
    if !deletes.is_empty() {
        let deletes_for_sqlite = deletes.clone();
        writer
            .batch(vec![], deletes, true, true, WriterPriority::Background)
            .map_err(|e| format!("reconciliation batch: {e}"))?;
        if let Err(e) = meta_db.delete_paths(&deletes_for_sqlite) {
            crate::logger::log(format!("reconciliation: delete_paths failed: {e}"));
        }
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
struct ReconciliationReport {
    failed_cleaned: usize,
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

    fn mk_fav(name: &str, path: &std::path::Path, metadata: bool) -> FavoriteEntry {
        let mut fav = FavoriteEntry::new(name.to_string(), path.to_path_buf());
        fav.auto_index_metadata = metadata;
        fav
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
            crate::global_search::run("dummy", &[fav_id], &scope, &fts_cl, &cancel_cl, &tx);
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
        let mut mgr = IndexerManager {
            meta_db: Arc::clone(&meta),
            fts: Arc::clone(&fts),
            writer,
            io_sem: Arc::clone(&io_sem),
            activity_gate: Arc::new(ActivityGate::new(1000)),
            excluded_roots: Vec::new(),
            supervisors: HashMap::new(),
            favorite_info: HashMap::new(),
            reconciliation_in_progress: Arc::new(AtomicBool::new(false)),
            startup_diag: StartupDiag::default(),
        };

        let mut fav = mk_fav("A", &root_old, true);
        // 初回 spawn
        mgr.sync_with_favorites(&[fav.clone()]);
        let handle1_thread_id = mgr
            .supervisors
            .get(&fav.id)
            .map(|h| h.favorite_id)
            .expect("handle inserted");

        // path 変更 (id は同じ)
        fav.path = root_new.clone();
        mgr.sync_with_favorites(&[fav.clone()]);
        // supervisors にはちゃんと入ったまま (新 path で respawn されたはず)
        assert!(mgr.supervisors.contains_key(&fav.id));
        // favorite_info の path が新パスになっている
        assert_eq!(mgr.favorite_info.get(&fav.id).unwrap().1, root_new);
        // favorite_id は変わっていない
        assert_eq!(
            mgr.supervisors.get(&fav.id).unwrap().favorite_id,
            handle1_thread_id
        );

        // 明示 drop で clean shutdown
        drop(mgr);
    }

    #[test]
    fn spawn_reconciliation_flag_resets() {
        let tmp = TempDir::new().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let flag = Arc::new(AtomicBool::new(true));

        spawn_reconciliation(meta, fts, vec![], Arc::clone(&flag));
        // 完了で false に戻る
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if !flag.load(Ordering::SeqCst) {
                break;
            }
            if Instant::now() >= deadline {
                panic!("reconciliation flag did not reset");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
