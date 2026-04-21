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
//! - `new()` の起動時 reconciliation: バックグラウンドスレッドで遅延実行する設計にしてある。

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use uuid::Uuid;

use crate::fts_index::FtsIndex;
use crate::fts_meta::{FileStatus, FtsMetaDb};
use crate::global_search::SearchStreamEvent;
use crate::indexer_supervisor::{self, SupervisorHandle, SupervisorParams, SupervisorStats};
use crate::io_semaphore::GlobalIoSemaphore;
use crate::settings::FavoriteEntry;

/// 全 Supervisor の I/O 同時実行上限。ハードコード値で v1 は OK。
/// (§7.6 の Low/Med/High プロファイルで調整できるようにするのは v1.x)
const IO_PERMITS: usize = 2;

/// 検索ハンドル。UI 側が `try_recv` で stream を受け取る。
pub struct SearchHandle {
    pub cancel: Arc<AtomicBool>,
    pub rx: Receiver<SearchStreamEvent>,
}

/// IndexerManager のコア。App が保有する。
pub struct IndexerManager {
    meta_db: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    io_sem: Arc<GlobalIoSemaphore>,
    /// お気に入り UUID → Supervisor ハンドル
    supervisors: HashMap<Uuid, SupervisorHandle>,
    /// 有効化されていないお気に入りでも、お気に入り UUID → (name, path) を記憶しておく
    /// (stats UI で name を出すため)
    favorite_info: HashMap<Uuid, (String, std::path::PathBuf)>,
    /// reconciliation が進行中なら true (UI に "DB 初期化中" 表示用)
    pub reconciliation_in_progress: Arc<AtomicBool>,
}

impl IndexerManager {
    /// DB/index を開き、auto_index_metadata=true のお気に入りに Supervisor を spawn する。
    ///
    /// DB 初期化に失敗したら None (App 側は fts 機能なしで動作継続する)。
    /// 起動時 reconciliation は **バックグラウンドスレッド** で遅延実行 — UI を止めない。
    pub fn new(favorites: &[FavoriteEntry]) -> Option<Self> {
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
        let io_sem = Arc::new(GlobalIoSemaphore::new(IO_PERMITS));
        let reconciliation_flag = Arc::new(AtomicBool::new(true));

        // 起動時 reconciliation: status != ok の行を整理 (別スレッド)。
        // 対象お気に入りを渡してバックグラウンド処理。完了後に `reconciliation_in_progress`
        // を false に戻す。
        spawn_reconciliation(
            Arc::clone(&meta_db),
            Arc::clone(&fts),
            favorites.to_vec(),
            Arc::clone(&reconciliation_flag),
        );

        let mut mgr = IndexerManager {
            meta_db,
            fts,
            io_sem,
            supervisors: HashMap::new(),
            favorite_info: HashMap::new(),
            reconciliation_in_progress: reconciliation_flag,
        };
        // 最初の supervisor 群を起動
        mgr.sync_with_favorites(favorites);
        Some(mgr)
    }

    /// 現在のお気に入り一覧と supervisors を同期。
    /// - 新規 `auto_index_metadata = true` → spawn
    /// - 既存で OFF に切り替わった / 削除された → drop
    /// - 既存で ON のまま → 維持
    ///
    /// **UI スレッドから呼ぶ時の注意**: Supervisor の drop は内部で thread join を伴うため、
    /// 多数の stop が発生する場面 (例: 全 OFF) ではブロックする可能性がある。
    /// 環境設定ダイアログの OK 押下時のような、ユーザが待ってもよいタイミングで呼ぶこと。
    pub fn sync_with_favorites(&mut self, favorites: &[FavoriteEntry]) {
        // favorite_info を最新化
        self.favorite_info.clear();
        for f in favorites {
            self.favorite_info.insert(f.id, (f.name.clone(), f.path.clone()));
        }

        // 削除 / OFF 化されたものを特定
        let current_ids: std::collections::HashSet<Uuid> = favorites
            .iter()
            .filter(|f| f.auto_index_metadata)
            .map(|f| f.id)
            .collect();
        let to_stop: Vec<Uuid> = self
            .supervisors
            .keys()
            .filter(|id| !current_ids.contains(id))
            .copied()
            .collect();
        for id in to_stop {
            if let Some(handle) = self.supervisors.remove(&id) {
                // handle drop = cancel + thread join
                drop(handle);
            }
        }

        // 新規 ON を spawn
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
                    favorite_path: info
                        .map(|(_, p)| p)
                        .unwrap_or_else(std::path::PathBuf::new),
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
    ///
    /// 戻り値の `SearchHandle` を drop すると自動的に cancel が立つ (受信側の `rx` drop で
    /// 送信側が break することに依存)。明示的に cancel したい場合は `handle.cancel.store(true)`。
    pub fn spawn_search(&self, query: String, favorite_ids: Vec<Uuid>) -> SearchHandle {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx): (Sender<SearchStreamEvent>, Receiver<SearchStreamEvent>) =
            crossbeam_channel::unbounded();
        let meta_db = Arc::clone(&self.meta_db);
        let fts = Arc::clone(&self.fts);
        let cancel_cl = Arc::clone(&cancel);

        std::thread::Builder::new()
            .name("ctrl-g-search".to_string())
            .spawn(move || {
                crate::global_search::run(&query, &favorite_ids, &fts, &meta_db, &cancel_cl, &tx);
            })
            .ok();

        SearchHandle { cancel, rx }
    }

    /// Ctrl+F (ローカルメタ検索) 用: 指定 path 群の all_text_norm を同期取得する。
    ///
    /// **UI スレッドから直接呼ばないこと** — App 側の worker 経由で呼ぶ契約。
    /// 返るのは (path, all_text_norm) で、status != ok の path は含まれない。
    pub fn lookup_local_texts(
        &self,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        self.meta_db
            .lookup_all_text_norm(paths)
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
}

impl Drop for IndexerManager {
    fn drop(&mut self) {
        // 全 supervisor を drop。handle.drop() は cancel + thread join を行う。
        // 多数 favorite で時間がかかる可能性があるが、App 終了時のみなので許容。
        for (id, handle) in self.supervisors.drain() {
            crate::logger::log(format!("IndexerManager: stopping supervisor {id}"));
            drop(handle);
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
            let result = run_reconciliation(&meta_db, &fts, &favorites);
            if let Err(e) = result {
                crate::logger::log(format!("reconciliation: failed: {e}"));
            }
            done_flag.store(false, Ordering::SeqCst);
        })
        .ok();
}

fn run_reconciliation(
    meta_db: &FtsMetaDb,
    fts: &FtsIndex,
    favorites: &[FavoriteEntry],
) -> Result<ReconciliationReport, String> {
    let mut report = ReconciliationReport::default();
    let mut writer = fts
        .writer()
        .map_err(|e| format!("fts writer init: {e}"))?;
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
                    crate::fts_index::delete_doc(&writer, fields, &path);
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
                    crate::fts_index::delete_doc(&writer, fields, &path);
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
    writer
        .commit()
        .map_err(|e| format!("writer.commit: {e}"))?;
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
    use crate::fts_index::{upsert_doc, Container, IndexDoc};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn mk_fav(name: &str, path: &std::path::Path, metadata: bool) -> FavoriteEntry {
        let mut fav = FavoriteEntry::new(name.to_string(), path.to_path_buf());
        fav.auto_index_metadata = metadata;
        fav
    }

    fn write_image(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), b"fake-image").unwrap();
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
        meta.mark_pending("c:/a/1.jpg", fav.id, &fav_root, 1, 1, "txt")
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
                    mtime: 1,
                    file_size: 1,
                    name: "1.jpg".into(),
                    all_text: "txt".into(),
                },
            )
            .unwrap();
            w.commit().unwrap();
        }

        // reconciliation 実行 (自前で writer を取るので前段 writer は drop 済み必須)
        let r = run_reconciliation(&meta, &fts, &[fav.clone()]).unwrap();
        assert_eq!(r.pending_cleaned, 1);

        // pending 行は削除され、次回 walker で再 ingest される予定
        assert!(meta.get("c:/a/1.jpg").unwrap().is_none());

        // Tantivy 側も delete_doc + commit 済み
        fts.reload_reader().unwrap();
        let q = crate::fts_index::build_bigram_and_query(
            fts.fields(),
            &["txt"],
            Some(&[fav.id]),
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
        meta.mark_pending("c:/a/1.jpg", fav.id, &fav_root, 1, 1, "t")
            .unwrap();
        meta.mark_ok(&["c:/a/1.jpg".to_string()]).unwrap();
        meta.mark_tombstone(&["c:/a/1.jpg".to_string()]).unwrap();

        let r = run_reconciliation(&meta, &fts, &[fav.clone()]).unwrap();
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

        meta.mark_pending("c:/a/1.jpg", fav.id, &fav_root, 1, 1, "t")
            .unwrap();

        let r = run_reconciliation(&meta, &fts, &[fav]).unwrap();
        assert_eq!(r.pending_cleaned, 0, "OFF の favorite は reconciliation 対象外");
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
            crate::global_search::run(
                "dummy",
                &[fav_id],
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
