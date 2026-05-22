//! 名前索引 (Ctrl+S 検索用 `search_index_db`) のスーパーバイザ。
//!
//! ## 責務
//!
//! お気に入りで `auto_index_structure = true` の間、以下を 1 スレッドで担当する:
//!
//! 1. 起動時の初期バルクスキャン (`name_bulk_indexer::run_bulk_name_index`)
//! 2. FS 監視 (`FsWatcher` = notify-rs) を張り続け、debounce 済みイベントを受信
//! 3. 変更された path の parent フォルダを再走査して `search_index_db::upsert_children`
//!    で差分反映
//! 4. `NameIndexStats` を UI に snapshot 可能な形で公開
//!
//! ## メタ索引 Supervisor との違い
//!
//! - 書き込み先が SQLite (`SearchIndexDb`) なので Tantivy writer 制約がない →
//!   **複数お気に入りの name supervisor は真に並列で動ける**
//! - Ingest フェーズが軽量 (upsert_children = `INSERT OR REPLACE`) なので、メタ側の
//!   ように `writer.lock()` 直前に「取込待ち」状態を出す必要はない
//!
//! ## 既存の `name_bulk_indexer` との関係
//!
//! `name_bulk_indexer::run_bulk_name_index` をワンショット bulk の実装本体として
//! 再利用する。旧来 `spawn_bulk` で thread を投げっぱなしにしていた経路は
//! `NameIndexSupervisor` で長期スレッド化した形に置き換わる。
//!
//! 2026-04 ユーザー指摘: 旧 `name_bulk_indexer::spawn_bulk` は初期 1 回だけ走って
//! 終了していたため、「✅ 索引あり」表示はスナップショット時点の静的状態を示すに
//! 過ぎず、その後 FS に追加されたフォルダ/ZIP/PDF/動画は Ctrl+S 検索にヒットしなかった。
//! この module が FsWatcher を握って差分追従する責務を担う。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, bounded, select};
use uuid::Uuid;

use crate::folder_tree::walk_dirs_recursive_with_progress;
use crate::indexer_progress::ProgressReporter;
use crate::name_bulk_indexer::{collect_index_entries, run_bulk_name_index};
use crate::search_index_db::SearchIndexDb;
use crate::search_watcher::{ChangeKind, DebouncedChange, FsWatcher, OVERFLOW_MARKER_PATH};

/// UI が snapshot するステータス。
#[derive(Clone, Debug, Default)]
pub struct NameIndexStats {
    /// 初期バルクスキャンが完了したか
    pub initial_scan_done: bool,
    /// フル scan (初期 or 手動再構築) を実行中か
    pub in_full_scan: bool,
    /// 初期バルクで書き込まれたエントリ件数 (参考)
    pub initial_entries_written: usize,
    /// watcher イベントで適用した更新回数 (参考)
    pub updates_applied: usize,
    /// 最新の `progress` 情報 (snapshot 時に合成される)
    pub current_activity: Option<String>,
    /// 現在のカウントベース進捗 (バルク取込中のみ)。
    pub eta: Option<crate::indexer_progress::EtaSnapshot>,
}

pub enum NameIndexCommand {
    Stop,
    #[allow(dead_code)] // 将来の手動再構築ボタン用
    FullRescan,
}

pub struct NameIndexSupervisorHandle {
    pub favorite_id: Uuid,
    cmd_tx: Sender<NameIndexCommand>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<NameIndexStats>>,
    progress: ProgressReporter,
    thread: Option<JoinHandle<()>>,
}

impl NameIndexSupervisorHandle {
    pub fn snapshot_stats(&self) -> NameIndexStats {
        let mut s = self.stats.lock().unwrap().clone();
        s.current_activity = self.progress.snapshot();
        s.eta = self.progress.snapshot_eta();
        s
    }

    /// cancel シグナルだけ送り、thread join は待たない。
    /// `IndexerManager::drop` と同じ「全員 signal_stop → drain」パターン用。
    pub fn signal_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(NameIndexCommand::Stop);
    }
}

impl Drop for NameIndexSupervisorHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(NameIndexCommand::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 1 お気に入り分の name index supervisor を起動する。
///
/// `activity_gate` が `Some` のとき、bulk scan は UI 操作中に自動で待機する (2026-04 F)。
/// テスト・レガシー経路で指定不要なら `None` を渡す。
pub fn spawn(
    favorite_id: Uuid,
    favorite_root: PathBuf,
    db: Arc<SearchIndexDb>,
    activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
) -> NameIndexSupervisorHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Mutex::new(NameIndexStats::default()));
    let progress = ProgressReporter::new();
    let (cmd_tx, cmd_rx) = bounded::<NameIndexCommand>(4);
    let (change_tx, change_rx) = crossbeam_channel::unbounded::<DebouncedChange>();

    let cancel_cl = Arc::clone(&cancel);
    let stats_cl = Arc::clone(&stats);
    let progress_cl = progress.clone();
    let root_cl = favorite_root.clone();

    crate::logger::log(format!(
        "name_index[{favorite_id}]: supervisor starting for {}",
        favorite_root.display()
    ));

    let thread = std::thread::Builder::new()
        .name(format!("name-index-{}", favorite_id.as_simple()))
        .spawn(move || {
            supervisor_loop(
                favorite_id,
                root_cl,
                db,
                activity_gate,
                cancel_cl,
                stats_cl,
                progress_cl,
                cmd_rx,
                change_tx,
                change_rx,
            );
        })
        .expect("failed to spawn name index supervisor");

    NameIndexSupervisorHandle {
        favorite_id,
        cmd_tx,
        cancel,
        stats,
        progress,
        thread: Some(thread),
    }
}

#[allow(clippy::too_many_arguments)]
fn supervisor_loop(
    favorite_id: Uuid,
    favorite_root: PathBuf,
    db: Arc<SearchIndexDb>,
    activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<NameIndexStats>>,
    progress: ProgressReporter,
    cmd_rx: Receiver<NameIndexCommand>,
    change_tx: Sender<DebouncedChange>,
    change_rx: Receiver<DebouncedChange>,
) {
    // 1. FsWatcher を先に起動して変更を取りこぼさない
    //    (初期 bulk 中に変更が起きても change_rx にたまる)
    let watcher = FsWatcher::start(favorite_id, &favorite_root, change_tx.clone()).ok();
    if watcher.is_none() {
        crate::logger::log(format!(
            "name_index[{favorite_id}]: FsWatcher start failed (will still run initial bulk)"
        ));
    }

    // 2. 初期バルク
    run_full_scan(
        favorite_id,
        &favorite_root,
        &db,
        activity_gate.as_deref(),
        &cancel,
        &stats,
        &progress,
    );

    // 3. watcher イベント + cmd を select で受信するループ
    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        select! {
            recv(cmd_rx) -> msg => {
                match msg {
                    Ok(NameIndexCommand::Stop) => break,
                    Ok(NameIndexCommand::FullRescan) => {
                        run_full_scan(
                            favorite_id,
                            &favorite_root,
                            &db,
                            activity_gate.as_deref(),
                            &cancel,
                            &stats,
                            &progress,
                        );
                    }
                    Err(_) => break,
                }
            }
            recv(change_rx) -> msg => {
                match msg {
                    Ok(DebouncedChange { favorite_id: fid, path, kind }) => {
                        if fid != favorite_id { continue; }
                        if path.to_string_lossy() == OVERFLOW_MARKER_PATH {
                            // overflow → フル再スキャン
                            crate::logger::log(format!(
                                "name_index[{favorite_id}]: watcher overflow, running full rescan"
                            ));
                            run_full_scan(
                                favorite_id,
                                &favorite_root,
                                &db,
                                activity_gate.as_deref(),
                                &cancel,
                                &stats,
                                &progress,
                            );
                            continue;
                        }
                        apply_single_change(
                            &favorite_root,
                            &db,
                            &path,
                            kind,
                            &progress,
                            &stats,
                            &cancel,
                            activity_gate.as_deref(),
                        );
                    }
                    Err(_) => break,
                }
            }
        }
    }

    drop(watcher);
    crate::logger::log(format!("name_index[{favorite_id}]: supervisor exiting"));
}

/// 初期 bulk / 手動再構築 / overflow で呼ばれる「フル scan」経路。
#[allow(clippy::too_many_arguments)]
fn run_full_scan(
    favorite_id: Uuid,
    favorite_root: &Path,
    db: &SearchIndexDb,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &AtomicBool,
    stats: &Mutex<NameIndexStats>,
    progress: &ProgressReporter,
) {
    let is_initial = !stats.lock().unwrap().initial_scan_done;
    stats.lock().unwrap().in_full_scan = true;
    let t0 = Instant::now();

    crate::logger::log(format!(
        "name_index[{favorite_id}]: {} scan starting",
        if is_initial { "initial" } else { "rescan" }
    ));

    let summary = run_bulk_name_index(favorite_root, db, activity_gate, cancel, Some(progress));

    let dur_ms = t0.elapsed().as_millis() as u64;
    crate::logger::log(format!(
        "name_index[{favorite_id}]: {} scan done in {dur_ms} ms (folders={}, entries={}, cancelled={})",
        if is_initial { "initial" } else { "rescan" },
        summary.folders_visited,
        summary.entries_written,
        summary.cancelled,
    ));

    {
        let mut s = stats.lock().unwrap();
        s.in_full_scan = false;
        if is_initial {
            s.initial_scan_done = true;
            s.initial_entries_written = summary.entries_written;
        }
    }
    progress.clear();
}

/// `run_subtree_scan` の結果。`apply_single_change` の `Ok(true) + is_dir()` 分岐で
/// post-scan prune を実行するかどうかを判定するために返す (Codex 第 9 レビュー指摘の
/// helper 抽出: cancel / read_dir error / prune-skip の小状態機械を明示)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubtreeScanOutcome {
    /// 全フォルダの read_dir + upsert が clean に完了。post-scan prune を実行できる。
    Completed,
    /// 途中で cancel が立った。post-scan prune は skip。
    Cancelled,
    /// read_dir / upsert 失敗が 1 件以上あった。post-scan prune は skip。
    Errored,
}

/// 1 つの notify-rs 変更イベントを `search_index.db` に反映する。
///
/// ## 仕様 (Codex レビュー反映、B4)
///
/// 1. **favorite 境界チェック**: `changed_path` が `favorite_root` 配下でなければ no-op。
/// 2. **`changed_path.try_exists()` を唯一の削除判定にする** (`kind` はヒントのみ。
///    `ChangeKind::Remove` の直後に再作成されるレースがあり得るため):
///    - `Ok(false)`: 確実に存在しない → ancestor chain を辿り「最も浅い missing 祖先」を
///      `delete_subtree`。favorite_root 自身まで届いた場合は `clear_for_favorite`。
///    - `Ok(true)`: 存在する → sibling 整合 (parent refresh) + (dir なら) subtree
///      recursive upsert + post-scan prune (`prune_stale_under_subtree`)。
///      ただし `changed_path == favorite_root` のときは parent refresh を **skip**
///      (`favorite_root.parent()` は favorite 配下外なので、そこを upsert すると
///      sibling フォルダを fav の行として誤投入する事故になる)。
///    - `Err(e)`: アクセス拒否 / 一時ロック等の曖昧状態 → log only、destructive cleanup
///      も再帰 upsert も走らせない。次回 watcher event / 起動時 walker 3-way diff で復旧。
///
/// 3. `scan_start` を parent refresh の **前** に取る (= parent refresh が `changed_path`
///    自身の行を新 stamp で書き戻す前)。さもないと post-scan prune が `changed_path`
///    自身を stale 扱いで消す。
///
/// 4. subtree scan が cancel / error で不完全だった場合は post-scan prune を skip
///    (フル scan と同じ「不完全観測では destructive 操作なし」ポリシー)。
#[allow(clippy::too_many_arguments)]
fn apply_single_change(
    favorite_root: &Path,
    db: &SearchIndexDb,
    changed_path: &Path,
    kind: ChangeKind,
    progress: &ProgressReporter,
    stats: &Mutex<NameIndexStats>,
    cancel: &AtomicBool,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
) {
    // 1. favorite 境界チェック (notify-rs の境界外イベントは watcher 設定上来ないはずだが、
    //    防衛的に)。
    if !crate::search_index_db::is_under(changed_path, favorite_root) {
        return;
    }

    progress.set(format!("更新: {}", changed_path.display()));

    // 2. try_exists() で分岐
    match changed_path.try_exists() {
        Ok(false) => {
            handle_missing_path(favorite_root, db, changed_path);
            stats.lock().unwrap().updates_applied += 1;
        }
        Ok(true) => {
            handle_existing_path(
                favorite_root,
                db,
                changed_path,
                progress,
                cancel,
                activity_gate,
            );
            stats.lock().unwrap().updates_applied += 1;
        }
        Err(e) => {
            crate::logger::log(format!(
                "name_index: try_exists failed for {} (kind={:?}): {e}",
                changed_path.display(),
                kind
            ));
            // destructive cleanup なし
        }
    }
    progress.clear();
}

/// `apply_single_change` の `try_exists() == Ok(false)` 分岐。
///
/// `changed_path` から `favorite_root` 手前まで親を辿り、`try_exists() == Ok(false)` で
/// 連鎖する祖先のうち **最も浅い missing 祖先** を `delete_subtree` の対象にする。
/// 連鎖が favorite_root 自身まで届いた場合 (= favorite root も消えた) は
/// `clear_for_favorite` で全消し。
///
/// `changed_path == favorite_root` の特殊ケース: ancestor chain では拾えないので
/// `clear_for_favorite` で全消し (supervisor 自身の停止は `IndexerManager::sync_with_favorites`
/// 経由で別途処理されるので、ここでは index データの掃除だけ責任を持つ)。
fn handle_missing_path(favorite_root: &Path, db: &SearchIndexDb, changed_path: &Path) {
    // 特殊ケース: changed_path == favorite_root
    if path_equals(changed_path, favorite_root) {
        if let Err(e) = db.clear_for_favorite(favorite_root) {
            crate::logger::log(format!(
                "name_index: clear_for_favorite failed for {}: {e}",
                favorite_root.display()
            ));
        }
        return;
    }

    // ancestor chain を辿って最も浅い missing 祖先を見つける
    let mut highest_missing: PathBuf = changed_path.to_path_buf();
    let mut cur: PathBuf = changed_path.to_path_buf();
    loop {
        let Some(parent) = cur.parent() else { break };
        let parent_path = parent.to_path_buf();

        if path_equals(&parent_path, favorite_root) {
            // 次は favorite_root 自身を check することになる。
            match favorite_root.try_exists() {
                Ok(false) => {
                    // favorite root も消えた → 全消し
                    if let Err(e) = db.clear_for_favorite(favorite_root) {
                        crate::logger::log(format!(
                            "name_index: clear_for_favorite failed for {}: {e}",
                            favorite_root.display()
                        ));
                    }
                    return;
                }
                Ok(true) => break, // favorite root は存在 → 現在の highest_missing で確定
                Err(e) => {
                    crate::logger::log(format!(
                        "name_index: try_exists ambiguous for favorite_root {}: {e}",
                        favorite_root.display()
                    ));
                    break;
                }
            }
        }

        // parent は favorite_root より下 (favorite 配下)
        match parent_path.try_exists() {
            Ok(false) => {
                highest_missing = parent_path.clone();
                cur = parent_path;
            }
            Ok(true) => break,
            Err(e) => {
                crate::logger::log(format!(
                    "name_index: try_exists ambiguous for ancestor {}: {e}",
                    parent_path.display()
                ));
                break;
            }
        }
    }

    if let Err(e) = db.delete_subtree(favorite_root, &highest_missing) {
        crate::logger::log(format!(
            "name_index: delete_subtree failed for {}: {e}",
            highest_missing.display()
        ));
    }
}

/// `apply_single_change` の `try_exists() == Ok(true)` 分岐。
///
/// 1. `scan_start` を **最初に** 取る (parent refresh の前)。さもないと post-scan prune が
///    `changed_path` 自身を stale と誤判定する (Codex P1 第 7 レビュー指摘)。
/// 2. `changed_path != favorite_root` の場合は parent refresh で sibling 整合を取る
///    (`favorite_root.parent()` は favorite 配下外なので、`==` のときは skip)。
/// 3. `changed_path.is_dir()` なら subtree recursive upsert。**outcome が `Completed` かつ
///    parent refresh が成功したとき** だけ post-scan prune を実行
///    (Codex P1 第 10 レビュー指摘: parent refresh が失敗した状態だと `changed_path` 自身の
///    Folder 行が更新されておらず、prune が `prune_stale_under_subtree` の `path = root_path`
///    マッチでそれを stale 扱いで消す事故がある)。
fn handle_existing_path(
    favorite_root: &Path,
    db: &SearchIndexDb,
    changed_path: &Path,
    progress: &ProgressReporter,
    cancel: &AtomicBool,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
) {
    // 1. scan_start は最初に取る
    let scan_start = crate::search_index_db::next_write_stamp();

    // 2. parent refresh (changed_path != favorite_root のみ)。失敗時は subtree prune を skip。
    //    `changed_path == favorite_root` の場合は parent refresh 自体が要らないので
    //    refresh は "成功" 扱いにする (subtree scan が changed_path 自身の更新を兼ねるため)。
    let parent_refresh_ok = if path_equals(changed_path, favorite_root) {
        true
    } else if let Some(parent) = changed_path.parent() {
        refresh_parent_listing(favorite_root, db, parent, cancel, activity_gate)
    } else {
        // changed_path に parent が無い (ルートそのもの) — favorite_root 配下なら通常起こらない
        true
    };

    // 3. ディレクトリなら subtree recursive upsert + post-scan prune
    if changed_path.is_dir() {
        let outcome = run_subtree_scan(
            favorite_root,
            changed_path,
            db,
            cancel,
            activity_gate,
            progress,
        );
        // prune は (a) subtree scan が clean に完了し、(b) parent refresh も成功している
        // ときのみ。どちらか欠けると「changed_path 自身の行が古い stamp のまま」「subtree が
        // 不完全観測」のいずれかで、prune が正当行を消す危険がある。
        if outcome == SubtreeScanOutcome::Completed && parent_refresh_ok {
            match db.prune_stale_under_subtree(favorite_root, changed_path, scan_start) {
                Ok(0) => {}
                Ok(n) => crate::logger::log(format!(
                    "name_index: pruned {n} stale rows under {}",
                    changed_path.display()
                )),
                Err(e) => crate::logger::log(format!(
                    "name_index: prune_stale_under_subtree failed for {}: {e}",
                    changed_path.display()
                )),
            }
        }
    }
}

/// `parent` の `read_dir` 結果で `upsert_children` を呼んで sibling 整合を取る。
/// `read_dir` が `Err` の場合は parent の存在を再判定し、`Ok(false)` なら親まるごと
/// missing として ancestor chain prune を回す。それ以外は log only。
///
/// 戻り値 `true` は「`changed_path` の Folder 行が parent の `upsert_children` で
/// 最新 stamp に更新された」を保証する (= 呼び出し側の post-scan prune が
/// `changed_path` 自身を stale と誤判定しない)。`false` は parent 経由の更新ができて
/// いない状態を示し、呼び出し側は subtree prune を skip すべき (Codex P1 第 10
/// レビュー指摘)。
#[must_use]
fn refresh_parent_listing(
    favorite_root: &Path,
    db: &SearchIndexDb,
    parent: &Path,
    // watcher event の parent refresh も 1 フォルダ規模で huge folder (1万件) が来うる
    // (Codex P2 第 16 ラウンド指摘)。`collect_index_entries` に gate/cancel を渡し、
    // entries ループ内で 64 件ごとに yield する。
    cancel: &AtomicBool,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
) -> bool {
    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(e) => {
            crate::logger::log(format!(
                "name_index: read_dir failed for {}: {e}",
                parent.display()
            ));
            // parent が消えているかも → try_exists で再判定
            match parent.try_exists() {
                Ok(false) => {
                    handle_missing_path(favorite_root, db, parent);
                }
                _ => {
                    // ambiguous or still exists → 何もしない
                }
            }
            // いずれにせよ parent 経由での `changed_path` 行更新は走っていない
            return false;
        }
    };
    // parent refresh も huge folder では 1 万件級の file_type が走る。
    // 動画オープン中などに HDD seek を握り続けないよう yield_check を渡す
    // (Codex P2 第 16 ラウンド指摘)。
    let (children, had_entry_error) = collect_index_entries(
        entries,
        "name_index parent refresh",
        activity_gate.map(|g| (g, cancel)),
    );
    if had_entry_error {
        // **upsert を skip する** (Codex P2 第 12 レビュー指摘):
        // 不完全 children で `upsert_children` を呼ぶと、観測できなかった legit 子エントリが
        // 親直下 DELETE で消える。upsert 自体を skip して既存行を保護する。
        // caller (`handle_existing_path`) は false を見て subtree prune も skip する。
        crate::logger::log(format!(
            "name_index: skipping upsert_children for parent {} (per-entry error: incomplete observation)",
            parent.display()
        ));
        return false;
    }
    // **upsert 直前 cancel race ガード** (Codex P2 第 17 ラウンド指摘):
    // `apply_favorite_name_index_change` は `signal_stop()` 後に join を別スレッドへ逃がしてから
    // `clear_for_favorite` するため、既にここに入っていた supervisor が cancel 後に
    // upsert を投げると、clear で消した行が再投入される窓が残る。`name_bulk_indexer` 側の
    // 同種ガード ([src/name_bulk_indexer.rs] の upsert 直前 cancel check) と整合させる。
    // 小さい親フォルダ (< 64 件) では `collect_index_entries` 内でも cancel に当たらないので
    // ここで明示的に確認する。
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    if let Err(e) = db.upsert_children(favorite_root, parent, &children) {
        crate::logger::log(format!(
            "name_index: upsert_children failed for {}: {e}",
            parent.display()
        ));
        return false;
    }
    true
}

/// `changed_path` 配下を再帰的に enumerate して各フォルダ直下を `upsert_children` する。
/// 結果は呼び出し側 (`handle_existing_path`) が post-scan prune を実行するかどうかの
/// 判断に使う。
fn run_subtree_scan(
    favorite_root: &Path,
    root_path: &Path,
    db: &SearchIndexDb,
    cancel: &AtomicBool,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    progress: &ProgressReporter,
) -> SubtreeScanOutcome {
    let mut subtree_folders: Vec<PathBuf> = Vec::new();
    let mut had_error = false;

    // Pass 1: 再帰的に folder を列挙。`on_visit` で activity_gate を待つ + `on_error` で log。
    walk_dirs_recursive_with_progress(
        root_path,
        &mut subtree_folders,
        cancel,
        &mut |cur| {
            if let Some(gate) = activity_gate {
                gate.wait_until_idle(cancel);
            }
            progress.set(format!("更新スキャン: {}", cur.display()));
        },
        &mut |p, e| {
            had_error = true;
            crate::logger::log(format!(
                "name_index: read_dir failed for {} (subtree scan Pass 1): {e}",
                p.display()
            ));
        },
        // huge folder の entries ループ内でも 64 件ごとに ActivityGate を見る。
        activity_gate,
    );

    if cancel.load(Ordering::Relaxed) {
        return SubtreeScanOutcome::Cancelled;
    }

    // Pass 2: 各フォルダ直下の Folder/ZipFile/PdfFile を upsert (動画は除外 §4.2)
    for folder in &subtree_folders {
        if crate::activity_gate::wait_and_check_cancel(activity_gate, cancel) {
            return SubtreeScanOutcome::Cancelled;
        }
        let entries = match std::fs::read_dir(folder) {
            Ok(e) => e,
            Err(e) => {
                had_error = true;
                crate::logger::log(format!(
                    "name_index: read_dir failed for {} (subtree scan Pass 2): {e}",
                    folder.display()
                ));
                continue;
            }
        };
        // `collect_index_entries` が per-entry エラー (DirEntry::Err / file_type 失敗) を
        // 検知して had_entry_error=true で返す (Codex P2 第 11 レビュー指摘)。
        // yield_check で 64 entry ごとに ActivityGate を見る (大きいフォルダの
        // file_type ループ中でも動画オープン等で indexer を即抑制できる)。
        let (children, had_entry_error) = collect_index_entries(
            entries,
            "name_index subtree scan Pass 2",
            activity_gate.map(|g| (g, cancel)),
        );
        if had_entry_error {
            // **upsert を skip する** (Codex P2 第 12 レビュー指摘): 不完全 children で
            // `upsert_children` を呼ぶと観測できなかった legit 子エントリが親直下 DELETE で
            // 消える。upsert 自体を skip して既存行を保護する。subtree 全体は
            // `Errored` 返却で post-scan prune が skip される。
            had_error = true;
            crate::logger::log(format!(
                "name_index: skipping upsert_children for {} (per-entry error: incomplete observation)",
                folder.display()
            ));
            continue;
        }
        // **upsert 直前 cancel race ガード** (Codex P2 第 17 ラウンド指摘):
        // `apply_favorite_name_index_change` の cancel → `clear_for_favorite` 直後に
        // 古い supervisor が upsert を投げて clear 済み行を再投入する窓を最小化する。
        // 小さい子フォルダ (< 64 件) では `collect_index_entries` 内 yield に到達せず
        // cancel を見ない可能性があるため、ここで明示的に確認する。`name_bulk_indexer`
        // 側の同種ガードと整合 ([src/name_bulk_indexer.rs] の `Codex P2 race 対策`)。
        if cancel.load(Ordering::Relaxed) {
            return SubtreeScanOutcome::Cancelled;
        }
        if let Err(e) = db.upsert_children(favorite_root, folder, &children) {
            had_error = true;
            crate::logger::log(format!(
                "name_index: upsert_children failed for {}: {e}",
                folder.display()
            ));
        }
    }

    // Pass 2 ループ最後の `wait_and_check_cancel` 後、最後の `upsert_children` 中 / 直後に
    // cancel が立つケースを拾う。これを check しないと、ループは全要素処理して
    // `Completed` を返し、呼び出し側で post-scan prune まで進んでしまう
    // (Codex P2 第 10 レビュー指摘: 不完全観測時は prune skip の設計に揃える)。
    if cancel.load(Ordering::Relaxed) {
        return SubtreeScanOutcome::Cancelled;
    }

    if had_error {
        SubtreeScanOutcome::Errored
    } else {
        SubtreeScanOutcome::Completed
    }
}

/// 2 つの path を `search_index_db::normalize_path` 経由で比較する。
/// `apply_single_change` 系の特殊ケース判定 (`changed_path == favorite_root`) で使う。
/// Windows の大文字小文字非区別と区切り文字混在 (`\` vs `/`) に対応するため、
/// 単純な `==` ではなく normalized 比較にする。
fn path_equals(a: &Path, b: &Path) -> bool {
    crate::search_index_db::normalize_path(a) == crate::search_index_db::normalize_path(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;

    /// 初期スキャン完了 + stats 反映を確認する最小 smoke test。
    /// watcher の E2E は OS 依存なので固定秒で polling する。
    #[test]
    fn initial_scan_populates_stats() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.zip"), b"").unwrap();
        fs::write(root.join("b.pdf"), b"").unwrap();

        let db = Arc::new(SearchIndexDb::open_in_memory().unwrap());
        let fav_id = Uuid::new_v4();
        let handle = spawn(fav_id, root.clone(), Arc::clone(&db), None);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = handle.snapshot_stats();
            if s.initial_scan_done && s.initial_entries_written >= 3 {
                break;
            }
            if Instant::now() >= deadline {
                panic!("initial scan did not complete: {s:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(handle);
    }

    #[test]
    fn drop_handle_stops_cleanly() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        fs::create_dir_all(&root).unwrap();
        let db = Arc::new(SearchIndexDb::open_in_memory().unwrap());
        let handle = spawn(Uuid::new_v4(), root, db, None);
        // drop で無限待ちしないこと
        let t0 = Instant::now();
        drop(handle);
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "drop took too long ({:?})",
            t0.elapsed()
        );
    }

    // -----------------------------------------------------------------------
    // apply_single_change の private unit test (B4: Codex レビュー反映)
    // integration test (tests/search_name_e2e.rs) からは apply_single_change が
    // private なので呼べないため、ここに置く (API surface を広げないため pub 化は避ける)。
    // -----------------------------------------------------------------------

    use crate::search_query::MatchMode;

    fn empty_zip_bytes() -> [u8; 22] {
        // "End of central directory record" 最小形式 (zip が空でも PK\x05\x06 で終わる)
        [
            0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    }

    fn write_empty_zip(p: &Path) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, empty_zip_bytes()).unwrap();
    }

    fn count_hits(db: &SearchIndexDb, q: &str, roots: &[PathBuf]) -> usize {
        db.search(q, roots, None, MatchMode::And).unwrap().len()
    }

    /// B4 の本懐: 深い `marker.zip` の Remove イベントしか届かないケースで、
    /// ancestor chain prune が `new_top` / `mid` / `deep` の Folder 行まで掃除する。
    #[test]
    fn watcher_prunes_on_deep_leaf_only_event() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        // root/new_top/mid/deep/marker.zip を作って初期 index
        let deep = root.join("new_top").join("mid").join("deep");
        write_empty_zip(&deep.join("marker.zip"));
        fs::write(deep.join("marker.pdf"), b"fake").unwrap();
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        crate::name_bulk_indexer::run_bulk_name_index(&root, &db, None, &cancel, None);
        let roots = vec![root.clone()];
        assert!(
            count_hits(&db, "marker", &roots) >= 2,
            "初期 index で marker がヒット"
        );
        assert!(count_hits(&db, "new_top", &roots) >= 1);
        assert!(count_hits(&db, "mid", &roots) >= 1);
        assert!(count_hits(&db, "deep", &roots) >= 1);

        // 物理削除 (subtree 丸ごと)
        fs::remove_dir_all(root.join("new_top")).unwrap();

        // notify-rs 経由ではなく `apply_single_change` を直接呼んで「最深イベントだけ届いた」
        // を擬似化
        let progress = ProgressReporter::new();
        let stats = Mutex::new(NameIndexStats::default());
        apply_single_change(
            &root,
            &db,
            &deep.join("marker.zip"),
            ChangeKind::Remove,
            &progress,
            &stats,
            &cancel,
            None,
        );

        // すべて消える
        assert_eq!(
            count_hits(&db, "marker", &roots),
            0,
            "marker.zip/pdf が消える"
        );
        assert_eq!(count_hits(&db, "deep", &roots), 0, "deep Folder 行が消える");
        assert_eq!(count_hits(&db, "mid", &roots), 0, "mid Folder 行が消える");
        assert_eq!(
            count_hits(&db, "new_top", &roots),
            0,
            "new_top Folder 行が消える (ancestor chain prune)"
        );
    }

    /// `changed_path == favorite_root` で parent refresh が skip されること。
    /// `favorite_root.parent()` を `read_dir` すると sibling フォルダを fav 配下と誤投入する
    /// 事故を防ぐ。
    #[test]
    fn watcher_changed_path_eq_fav_root_skips_parent_refresh() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().to_path_buf();
        // outer/fav と outer/other_sibling の 2 つを作る
        let fav = outer.join("fav");
        let other = outer.join("other_sibling");
        fs::create_dir_all(&fav).unwrap();
        fs::create_dir_all(&other).unwrap();
        // fav 内に内容を put
        fs::create_dir_all(fav.join("inside")).unwrap();

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        // 初期 index は空。apply_single_change(changed=fav, kind=Upsert) を呼ぶ
        let progress = ProgressReporter::new();
        let stats = Mutex::new(NameIndexStats::default());
        apply_single_change(
            &fav,
            &db,
            &fav,
            ChangeKind::Upsert,
            &progress,
            &stats,
            &cancel,
            None,
        );

        let roots = vec![fav.clone()];

        // `inside` フォルダは fav 配下なので入る
        assert!(count_hits(&db, "inside", &roots) >= 1, "fav 配下は入る");
        // `other_sibling` は fav の sibling なので、絶対に入っていてはいけない
        // (parent refresh が favorite_root.parent() = outer を走査していたら誤投入される)
        assert_eq!(
            count_hits(&db, "other_sibling", &roots),
            0,
            "sibling フォルダが fav 配下に誤投入されてはいけない"
        );
    }

    /// `changed_path == favorite_root` で fav 自体が消えたケース。`clear_for_favorite`
    /// 相当の全消しが走る。
    #[test]
    fn watcher_changed_path_eq_fav_root_missing_clears_all() {
        let tmp = TempDir::new().unwrap();
        let fav = tmp.path().join("fav");
        fs::create_dir_all(fav.join("sub")).unwrap();
        write_empty_zip(&fav.join("sub").join("x.zip"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        crate::name_bulk_indexer::run_bulk_name_index(&fav, &db, None, &cancel, None);
        let roots = vec![fav.clone()];
        assert!(count_hits(&db, "x.zip", &roots) >= 1);

        // fav 自体を物理削除
        fs::remove_dir_all(&fav).unwrap();

        let progress = ProgressReporter::new();
        let stats = Mutex::new(NameIndexStats::default());
        apply_single_change(
            &fav,
            &db,
            &fav,
            ChangeKind::Remove,
            &progress,
            &stats,
            &cancel,
            None,
        );

        // fav 配下のすべての行が消える
        assert_eq!(count_hits(&db, "x.zip", &roots), 0);
        assert_eq!(count_hits(&db, "sub", &roots), 0);
        assert_eq!(
            db.count_for_favorite(&fav).unwrap(),
            0,
            "fav 配下が全消えしている"
        );
    }

    /// 同名ディレクトリの fast delete → recreate (子孫が減る) で stale 孫行が掃除される。
    #[test]
    fn watcher_prunes_stale_grandchildren_on_dir_recreate() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        // root/sub/deep/old.zip を put
        let sub = root.join("sub");
        let deep = sub.join("deep");
        write_empty_zip(&deep.join("old.zip"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        crate::name_bulk_indexer::run_bulk_name_index(&root, &db, None, &cancel, None);
        let roots = vec![root.clone()];
        assert!(count_hits(&db, "old.zip", &roots) >= 1);
        assert!(count_hits(&db, "deep", &roots) >= 1);

        // sub を丸ごと remove → 「shallow_only/new.zip」だけの構成で recreate
        fs::remove_dir_all(&sub).unwrap();
        let shallow = sub.join("shallow_only");
        write_empty_zip(&shallow.join("new.zip"));

        // changed_path = sub の Upsert イベントを擬似発火
        let progress = ProgressReporter::new();
        let stats = Mutex::new(NameIndexStats::default());
        apply_single_change(
            &root,
            &db,
            &sub,
            ChangeKind::Upsert,
            &progress,
            &stats,
            &cancel,
            None,
        );

        // 新しい実体はヒット
        assert!(
            count_hits(&db, "new.zip", &roots) >= 1,
            "new.zip がヒットすべき"
        );
        assert!(
            count_hits(&db, "shallow_only", &roots) >= 1,
            "shallow_only がヒットすべき"
        );
        // 古い stale 孫行が消える (= post-scan prune が掃除した)
        assert_eq!(
            count_hits(&db, "old.zip", &roots),
            0,
            "old.zip は recreate で消えた孫なので残ってはいけない"
        );
        assert_eq!(
            count_hits(&db, "deep", &roots),
            0,
            "deep フォルダ行も消えるべき"
        );
    }

    /// cancel が立った状態で apply_single_change を呼ぶと、subtree scan が即 Cancelled に
    /// なって prune が skip され、既存行が壊れない。
    #[test]
    fn watcher_subtree_cancel_skips_prune() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let sub = root.join("sub");
        write_empty_zip(&sub.join("old.zip"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel_setup = AtomicBool::new(false);
        crate::name_bulk_indexer::run_bulk_name_index(&root, &db, None, &cancel_setup, None);
        let roots = vec![root.clone()];
        assert!(count_hits(&db, "old.zip", &roots) >= 1);
        let before_total = db.count_for_favorite(&root).unwrap();

        // 既に cancel が立った状態で apply_single_change を呼ぶ → subtree scan が
        // 早期 return → post-scan prune skip → 既存行不変
        let cancel = AtomicBool::new(true);
        let progress = ProgressReporter::new();
        let stats = Mutex::new(NameIndexStats::default());
        apply_single_change(
            &root,
            &db,
            &sub,
            ChangeKind::Upsert,
            &progress,
            &stats,
            &cancel,
            None,
        );

        // sub の parent refresh は走ったかも知れないが、subtree prune は skip されて
        // old.zip 等の既存行は破壊されていないはず
        assert!(
            count_hits(&db, "old.zip", &roots) >= 1,
            "cancel 中の subtree scan が prune を走らせて行を消してはいけない"
        );
        let after_total = db.count_for_favorite(&root).unwrap();
        assert!(
            after_total >= before_total - 1,
            "cancel 中の apply で索引が壊滅してはいけない (before={before_total}, after={after_total})"
        );
    }
}
