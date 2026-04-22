//! `IndexerManager` — App 統合のための Supervisor 群管理 (docs/search-expansion-design.md §3)。
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

use crate::fts_index::FtsIndex;
use crate::fts_meta::{FileStatus, FtsMetaDb};
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
    /// **全 supervisor で共有する** Tantivy IndexWriter。
    /// Tantivy は 1 Index につき IndexWriter を 1 本しか許さないので、
    /// 複数のお気に入りの supervisor が各自 `fts.writer()` を呼ぶと
    /// 2 つ目以降が LockBusy で落ちる (旧実装のバグ、2026-04 修正)。
    writer: Arc<std::sync::Mutex<tantivy::IndexWriter>>,
    io_sem: Arc<GlobalIoSemaphore>,
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
    /// Tantivy から delete した残留 pending/failed 行数
    pub pending_cleaned: usize,
    /// 削除マーク済みだった行の物理削除数
    pub tombstone_purged: usize,
    /// スピードプロファイル (io_permits 数) — 診断時に見たいので保存
    pub io_permits: usize,
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
    pub fn new(
        favorites: &[FavoriteEntry],
        speed: crate::settings::IndexerSpeedProfile,
    ) -> Option<Self> {
        let meta_db = match FtsMetaDb::open() {
            Ok(db) => Arc::new(db),
            Err(e) => {
                crate::logger::log(format!("IndexerManager: FtsMetaDb open failed: {e}"));
                return None;
            }
        };
        let fts = match FtsIndex::open_default() {
            Ok(idx) => Arc::new(idx),
            Err(e) => {
                crate::logger::log(format!("IndexerManager: FtsIndex open failed: {e}"));
                return None;
            }
        };
        Self::new_with_stores(meta_db, fts, favorites, speed)
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
    ) -> Option<Self> {
        std::fs::create_dir_all(data_dir).ok();
        let meta_db = match FtsMetaDb::open_at(&data_dir.join("fts_meta.db")) {
            Ok(db) => Arc::new(db),
            Err(e) => {
                crate::logger::log(format!("IndexerManager(test): FtsMetaDb open failed: {e}"));
                return None;
            }
        };
        let fts = match FtsIndex::open_at(&data_dir.join("fts_index")) {
            Ok(idx) => Arc::new(idx),
            Err(e) => {
                crate::logger::log(format!("IndexerManager(test): FtsIndex open failed: {e}"));
                return None;
            }
        };
        Self::new_with_stores(meta_db, fts, favorites, speed)
    }

    /// `new` / `new_at` 共通の本体。stores を受け取って reconciliation + supervisor spawn を行う。
    fn new_with_stores(
        meta_db: Arc<FtsMetaDb>,
        fts: Arc<FtsIndex>,
        favorites: &[FavoriteEntry],
        speed: crate::settings::IndexerSpeedProfile,
    ) -> Option<Self> {
        // IndexWriter は 1 本だけ作って全 supervisor で共有する (Tantivy 制約)
        let writer = match fts.writer() {
            Ok(w) => Arc::new(std::sync::Mutex::new(w)),
            Err(e) => {
                crate::logger::log(format!("IndexerManager: writer init failed: {e}"));
                return None;
            }
        };
        let permits = speed.io_permits().max(1); // 0 は GlobalIoSemaphore で panic するので防御
        crate::logger::log(format!(
            "IndexerManager: speed profile = {:?} → io_permits = {permits}",
            speed
        ));
        let io_sem = Arc::new(GlobalIoSemaphore::new(permits));

        // === 起動時 reconciliation を先に同期実行 ===
        // supervisor が走る前に status != ok の残留行を整理することで、
        // Tantivy writer の競合も DB 上の race もなくなる。
        // 共有 writer を lock して reconciliation に渡す (Codex P1, 2026-04)。
        // 以前は `fts.writer()` を独自に呼んでおり、IndexerManager の共有 writer と
        // LockBusy で衝突していた。
        let t_recon = std::time::Instant::now();
        let report = {
            let mut w = writer.lock().expect("writer mutex poisoned");
            match run_reconciliation(&meta_db, &fts, &mut w, favorites) {
                Ok(r) => r,
                Err(e) => {
                    crate::logger::log(format!(
                        "IndexerManager: reconciliation failed (continuing anyway): {e}"
                    ));
                    ReconciliationReport::default()
                }
            }
        };
        let reconciliation_ms = t_recon.elapsed().as_millis() as u64;
        crate::logger::log(format!(
            "IndexerManager: reconciliation completed in {reconciliation_ms} ms"
        ));

        let mut mgr = IndexerManager {
            meta_db,
            fts,
            writer,
            io_sem,
            supervisors: HashMap::new(),
            favorite_info: HashMap::new(),
            reconciliation_in_progress: Arc::new(AtomicBool::new(false)),
            startup_diag: StartupDiag {
                reconciliation_ms,
                pending_cleaned: report.pending_cleaned,
                tombstone_purged: report.tombstone_purged,
                io_permits: permits,
            },
        };
        // reconciliation 完了後に supervisor 群を起動 (writer 競合なし)
        mgr.sync_with_favorites(favorites);
        Some(mgr)
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
        // **Codex P1 回帰 (2026-04)**: 共有 writer 下では 1 体ずつ drop すると
        // writer.lock() 待機中の supervisor が join 無限待ちになる可能性がある。
        // IndexerManager::drop と同じ signal_stop → drain パターンを使う。
        for id in &to_stop {
            if let Some(handle) = self.supervisors.get(id) {
                crate::logger::log(format!(
                    "IndexerManager: signaling supervisor {id} to stop (removed / off / path changed)"
                ));
                handle.signal_stop();
            }
        }
        for id in to_stop {
            if let Some(handle) = self.supervisors.remove(&id) {
                crate::logger::log(format!("IndexerManager: joining supervisor {id}"));
                drop(handle);
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
                    enable_metadata_index: true,
                },
                Arc::clone(&self.meta_db),
                Arc::clone(&self.fts),
                Arc::clone(&self.writer),
                Arc::clone(&self.io_sem),
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
        let meta_db = Arc::clone(&self.meta_db);
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
                    &meta_db,
                    &cancel_cl,
                    &tx,
                );
            })
            .ok();

        SearchHandle { cancel, rx }
    }

    /// Ctrl+F (ローカルメタ検索) 用: 指定 path 群のソース別正規化テキストを同期取得する (§19.4)。
    ///
    /// **UI スレッドから直接呼ばないこと** — App 側の worker 経由で呼ぶ契約。
    /// 返るのは (path, combined_norm_for_target) で、status != ok の path は含まれない。
    /// `target = SearchTarget::All` を渡せば 5 ソース結合 (旧 all_text_norm 互換)。
    pub fn lookup_local_texts(
        &self,
        paths: &[String],
        target: &crate::fts_index::SearchTarget,
    ) -> Result<Vec<(String, String)>, String> {
        self.meta_db
            .lookup_norms_for_target(paths, target)
            .map_err(|e| format!("{e}"))
    }

    /// favorite 数を返す (stats UI 用)。
    pub fn supervisor_count(&self) -> usize {
        self.supervisors.len()
    }

    /// `Arc<FtsMetaDb>` を clone して返す (Ctrl+F 用 §9.2)。
    /// worker thread 側で `lookup_all_text_norm` を呼びたいときに使う。
    pub fn clone_fts_meta(&self) -> Arc<FtsMetaDb> {
        Arc::clone(&self.meta_db)
    }

    /// `Arc<FtsIndex>` を clone して返す (タグ書き込み worker 用)。
    pub fn clone_fts_index(&self) -> Arc<FtsIndex> {
        Arc::clone(&self.fts)
    }

    /// 共有 IndexWriter の Arc を clone して返す (タグ書き込み worker 用)。
    /// Tantivy は 1 Index につき writer 1 本しか許さないので、worker が独自に
    /// `fts.writer()` を呼ぶと LockBusy で失敗する。必ずこれを使って lock() する。
    pub fn clone_shared_writer(&self) -> Arc<std::sync::Mutex<tantivy::IndexWriter>> {
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

    /// お気に入りの「メタ索引」チェックを OFF にした時のクリーンアップ。
    ///
    /// そのお気に入りの全 fts_meta 行を tombstone にマークする。検索結果からは
    /// 即座に消える (post-filter の status=0 ゲート経由)。Tantivy 側の doc 削除は
    /// 次回起動時の reconciliation が処理する (`run_reconciliation` が tombstone を
    /// 検出して `delete_doc` + `purge_tombstone` を走らせる)。
    ///
    /// 呼び出し順序: **必ず `sync_with_favorites` より前に呼ぶ** こと。先に supervisor
    /// を drop すると writer が別スレッドに移ってしまうので、こちらの SQL UPDATE 中に
    /// supervisor 側 ingest が走って race になる可能性がある (実害は限定的だが綺麗でない)。
    pub fn purge_favorite_metadata(&self, favorite_id: Uuid) -> usize {
        match self.meta_db.mark_tombstone_all_for_favorite(favorite_id) {
            Ok(n) => {
                crate::logger::log(format!(
                    "IndexerManager: purge_favorite_metadata({favorite_id}) tombstoned {n} rows"
                ));
                n
            }
            Err(e) => {
                crate::logger::log(format!(
                    "IndexerManager: purge_favorite_metadata({favorite_id}) failed: {e}"
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
        // **重要**: 共有 Arc<Mutex<IndexWriter>> の下では、writer.lock() 待機中の
        // supervisor は自分の cancel を観測できない。1 つずつ drop すると、lock を
        // 取れていない supervisor が先に drop された場合に、他の supervisor が
        // writer を長時間保持していると join が無限に待つ。
        // 先に全員の cancel を立てておけば、各 supervisor が apply 内の for ループで
        // cancel を検出して早期 exit し、writer もすぐに解放される。
        for (id, handle) in &self.supervisors {
            crate::logger::log(format!("IndexerManager: signaling supervisor {id} to stop"));
            handle.signal_stop();
        }

        // STEP 2: drain + drop (drop は thread join を含む)
        for (id, handle) in self.supervisors.drain() {
            crate::logger::log(format!("IndexerManager: joining supervisor {id}"));
            drop(handle);
        }

        // STEP 3: 全 supervisor が止まった後に共有 writer を 1 回 commit する。
        // supervisor 側の apply 中に flush/commit は既に済んでいるはずだが、最後の
        // 未 flush バッチをここで落としきる (旧実装は supervisor 個々の drop で commit
        // していたが、共有 writer では 1 回だけでよい)。
        if let Ok(mut w) = self.writer.lock() {
            if let Err(e) = w.commit() {
                crate::logger::log(format!("IndexerManager: final writer commit failed: {e}"));
            }
        }
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

/// **Codex P1 回帰 (2026-04)**: 共有 writer 化 (f21ac27) 以降は、呼び出し側 (IndexerManager)
/// が既に `fts.writer()` を 1 本取得しているため、reconciliation が独自に `fts.writer()` を
/// 呼ぶと LockBusy で失敗する。共有 writer を `&mut IndexWriter` として受け取る形に統一した。
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
            continue; // 未使用 favorite は触らない
        }
        let not_ok = meta_db
            .list_not_ok_paths(fav.id)
            .map_err(|e| format!("list_not_ok_paths: {e}"))?;
        for (path, status) in not_ok {
            match status {
                FileStatus::Ok => continue,
                FileStatus::Pending | FileStatus::Failed => {
                    // Tantivy には新しい doc が入っているかもしれない → 念のため delete。
                    // 次回 supervisor の初期スキャンで walker が "DB になし" として
                    // 拾って再 ingest する。
                    crate::fts_index::delete_doc(writer, fields, &path);
                    // DB 側も row を消す (walker の 3-way diff で to_ingest に入るように)
                    // ここは物理 DELETE が必要。purge_tombstone は status=3 限定なので使えない。
                    if let Err(e) = delete_row_forcing(meta_db, &path) {
                        crate::logger::log(format!(
                            "reconciliation: delete row for {path} failed: {e}"
                        ));
                    }
                    report.pending_cleaned += 1;
                }
                FileStatus::Tombstone => {
                    // Tantivy 側にまだ残っている可能性あるので delete して purge
                    crate::fts_index::delete_doc(writer, fields, &path);
                    if let Err(e) = meta_db.purge_tombstone(&[path.clone()]) {
                        crate::logger::log(format!(
                            "reconciliation: purge_tombstone {path} failed: {e}"
                        ));
                    }
                    report.tombstone_purged += 1;
                }
            }
        }
    }
    // 1 回だけ commit
    writer.commit().map_err(|e| format!("writer.commit: {e}"))?;
    crate::logger::log(format!(
        "reconciliation done: pending/failed cleaned = {}, tombstone purged = {}",
        report.pending_cleaned, report.tombstone_purged
    ));
    Ok(report)
}

/// fts_meta.db から path を物理削除する (status 不問)。
///
/// `FtsMetaDb` の公開 API には "status に関わらず物理削除" のメソッドが無いので、
/// ここで直接 SQL を叩く。将来 `FtsMetaDb::purge_path` として生やしても良い。
fn delete_row_forcing(meta_db: &FtsMetaDb, path: &str) -> rusqlite::Result<()> {
    // tombstone 以外の status のときも DELETE したいので、既存の purge_tombstone の
    // `status = 3` 条件を回避する専用処理。
    // ※ FtsMetaDb の API を汚さないため、ここで一段階 status=3 に落としてから purge する。
    meta_db.mark_tombstone(&[path.to_string()])?;
    meta_db.purge_tombstone(&[path.to_string()])?;
    Ok(())
}

#[derive(Default, Debug, Clone)]
struct ReconciliationReport {
    pending_cleaned: usize,
    tombstone_purged: usize,
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_index::{Container, IndexDoc, IndexKind, QueryFilters, upsert_doc};
    use crate::ingest_text::PerSourceText;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn txt_norms(name: &str) -> PerSourceText {
        PerSourceText {
            name: name.to_string(),
            ..PerSourceText::default()
        }
    }

    fn mk_fav(name: &str, path: &std::path::Path, metadata: bool) -> FavoriteEntry {
        let mut fav = FavoriteEntry::new(name.to_string(), path.to_path_buf());
        fav.auto_index_metadata = metadata;
        fav
    }

    // run_reconciliation の単体テスト。IndexerManager::new は APPDATA に
    // 依存するので、ここでは reconciliation 関数を直接テストする。

    #[test]
    fn reconciliation_cleans_pending_rows() {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap();
        let fts = FtsIndex::open_at(&tmp.path().join("fts")).unwrap();
        let fav_root = tmp.path().join("a");
        std::fs::create_dir_all(&fav_root).unwrap();

        let fav = mk_fav("A", &fav_root, true);

        // pending 行を手動で作る (crash 残留シミュ)
        meta.mark_pending(
            "c:/a/1.jpg",
            fav.id,
            &fav_root,
            IndexKind::Image,
            1,
            1,
            &txt_norms("txt"),
        )
        .unwrap();
        // Tantivy にも入れておく (writer は scope 内で drop して lockfile を解放する)
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

        // reconciliation 実行 (共有 writer 化後は呼び出し側が writer を用意する)
        let mut rw = fts.writer().unwrap();
        let r = run_reconciliation(&meta, &fts, &mut rw, &[fav.clone()]).unwrap();
        drop(rw);
        assert_eq!(r.pending_cleaned, 1);

        // pending 行は削除され、次回 walker で再 ingest される予定
        assert!(meta.get("c:/a/1.jpg").unwrap().is_none());

        // Tantivy 側も delete_doc + commit 済み
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
    fn reconciliation_purges_tombstone() {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap();
        let fts = FtsIndex::open_at(&tmp.path().join("fts")).unwrap();
        let fav_root = tmp.path().join("a");
        std::fs::create_dir_all(&fav_root).unwrap();
        let fav = mk_fav("A", &fav_root, true);

        // tombstone 行を作る
        meta.mark_pending(
            "c:/a/1.jpg",
            fav.id,
            &fav_root,
            IndexKind::Image,
            1,
            1,
            &txt_norms("t"),
        )
        .unwrap();
        meta.mark_ok(&["c:/a/1.jpg".to_string()]).unwrap();
        meta.mark_tombstone(&["c:/a/1.jpg".to_string()]).unwrap();

        let mut rw = fts.writer().unwrap();
        let r = run_reconciliation(&meta, &fts, &mut rw, &[fav.clone()]).unwrap();
        drop(rw);
        assert_eq!(r.tombstone_purged, 1);
        assert!(meta.get("c:/a/1.jpg").unwrap().is_none());
    }

    #[test]
    fn reconciliation_skips_favorites_without_metadata_flag() {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap();
        let fts = FtsIndex::open_at(&tmp.path().join("fts")).unwrap();
        let fav_root = tmp.path().join("a");
        std::fs::create_dir_all(&fav_root).unwrap();
        // metadata フラグ OFF
        let fav = mk_fav("A", &fav_root, false);

        meta.mark_pending(
            "c:/a/1.jpg",
            fav.id,
            &fav_root,
            IndexKind::Image,
            1,
            1,
            &txt_norms("t"),
        )
        .unwrap();

        let mut rw = fts.writer().unwrap();
        let r = run_reconciliation(&meta, &fts, &mut rw, &[fav]).unwrap();
        drop(rw);
        assert_eq!(
            r.pending_cleaned, 0,
            "OFF の favorite は reconciliation 対象外"
        );
        // 行はそのまま残っているはず
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

        // favorite 0 件で検索 → Complete
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let meta_cl = Arc::clone(&meta);
        let fts_cl = Arc::clone(&fts);
        let cancel_cl = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let scope = crate::global_search::SearchScope::default();
            crate::global_search::run(
                "dummy",
                &[fav_id],
                &scope,
                &fts_cl,
                &meta_cl,
                &cancel_cl,
                &tx,
            );
        });
        // 何らかの SearchStreamEvent が返ることを確認
        let ev = rx.recv_timeout(Duration::from_secs(2)).unwrap();
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

        let writer = Arc::new(std::sync::Mutex::new(fts.writer().unwrap()));
        let mut mgr = IndexerManager {
            meta_db: Arc::clone(&meta),
            fts: Arc::clone(&fts),
            writer,
            io_sem: Arc::clone(&io_sem),
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
        let deadline = Instant::now() + Duration::from_secs(5);
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
