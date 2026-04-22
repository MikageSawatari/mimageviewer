//! タグ書き込みのバックグラウンド worker (docs/tag-feature.md §5.6)。
//!
//! UI からの「タグ X をトグル」「すべてクリア」要求を受け取り、1 ファイルずつ
//! シリアルに `xmp_writer::apply_tag_op` を実行する。書き込み成功後は
//! `fts_meta.set_tags` + 共有 Tantivy writer 経由で index を即時更新し、
//! 次の Ctrl+G に反映させる。
//!
//! # Tantivy writer 共有
//!
//! Tantivy は 1 Index につき IndexWriter を 1 本しか許さないため、
//! `IndexerManager` が保有する `Arc<Mutex<IndexWriter>>` を共有して使う。
//! worker が独自に `fts.writer()` を呼ぶと LockBusy で全 upsert が無効化される。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use tantivy::IndexWriter;
use uuid::Uuid;

use crate::fts_index::{self, Container, FtsIndex, IndexDoc};
use crate::fts_meta::FtsMetaDb;
use crate::xmp_writer::{TagOp, WriteError};

/// バッチ commit の閾値 (件数と時間の OR)。ingest_worker の BATCH_FLUSH_COUNT=100 / 5s
/// よりは小さめ — タグ書き込みは UI 操作から来るので Ctrl+G への反映レイテンシを
/// 500ms 以内に抑えたい。かつ 100 ファイル一括トグルで commit 1 回に畳まれる。
const BATCH_FLUSH_COUNT: usize = 32;
const BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// UI が worker に渡す操作。XMP ファイル読み出しを伴う Toggle は worker 側で
/// Add/Remove に解決するので、UI スレッドは同期 I/O を一切行わなくて済む。
#[derive(Debug, Clone)]
pub enum TagJobKind {
    /// 現在のタグ状態を worker が XMP から読み出し、含まれていれば Remove、
    /// 含まれていなければ Add を実行する。
    Toggle(String),
    /// `#` で始まる全 Bag 要素を削除。
    ClearMiv,
}

#[derive(Debug, Clone)]
pub struct TagWriteJob {
    pub path: PathBuf,
    pub kind: TagJobKind,
    /// 検索インデックス更新用の favorite_id。None なら Tantivy upsert をスキップ
    /// (次回 notify-rs re-ingest で反映)。
    pub favorite_id: Option<Uuid>,
}

/// `Toggle` / `ClearMiv` が実際に何をしたかを UI に返すためのラベル。
/// 完了トーストで「付与 / 削除」の実際値を見せるのに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagAction {
    /// Toggle: タグを追加した (Add 経路に解決された)。
    Added,
    /// Toggle: タグを削除した (Remove 経路に解決された)。
    Removed,
    /// ClearMiv: `#` 始まりの要素をまとめて削除した (1 件以上の削除が発生)。
    Cleared,
    /// 実質変化なし (clear した時に元々空だったケース等)。
    NoOp,
}

#[derive(Debug, Clone)]
pub struct TagWriteResult {
    pub path: PathBuf,
    pub result: Result<TagAction, String>,
}

pub struct TagWriteHandle {
    job_tx: Sender<TagWriteJob>,
    result_rx: Receiver<TagWriteResult>,
    pub total: Arc<AtomicUsize>,
    pub done: Arc<AtomicUsize>,
    pub failures: Arc<AtomicUsize>,
    /// Tantivy writer バッファに upsert 済みだが、まだ commit されていない dirty ジョブ数。
    /// `is_busy()` で「XMP 書き込みは終わったが検索索引にはまだ反映されていない」
    /// 状態を UI に伝えるのに使う (完了 toast が commit より先に出る race を塞ぐ)。
    pub pending_in_writer: Arc<AtomicUsize>,
    _thread: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl TagWriteHandle {
    /// worker スレッドを起動する。`shared_writer` は必ず `IndexerManager::clone_shared_writer()`
    /// 由来のものを渡すこと。独自 writer を作ると Tantivy が LockBusy で落ちる。
    pub fn spawn(
        meta: Arc<FtsMetaDb>,
        fts: Arc<FtsIndex>,
        shared_writer: Arc<Mutex<IndexWriter>>,
    ) -> Self {
        let (job_tx, job_rx) = unbounded::<TagWriteJob>();
        let (result_tx, result_rx) = unbounded::<TagWriteResult>();
        let total = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(AtomicUsize::new(0));
        let pending_in_writer = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let w_done = done.clone();
        let w_failures = failures.clone();
        let w_pending = pending_in_writer.clone();
        let w_shutdown = shutdown.clone();
        let handle = std::thread::Builder::new()
            .name("tag-write-worker".into())
            .spawn(move || {
                run_worker(
                    &job_rx,
                    &result_tx,
                    meta,
                    fts,
                    shared_writer,
                    &w_done,
                    &w_failures,
                    &w_pending,
                    &w_shutdown,
                );
            })
            .expect("tag-write-worker spawn");

        Self {
            job_tx,
            result_rx,
            total,
            done,
            failures,
            pending_in_writer,
            _thread: Some(handle),
            shutdown,
        }
    }

    pub fn submit(&self, job: TagWriteJob) {
        self.total.fetch_add(1, Ordering::Relaxed);
        let _ = self.job_tx.send(job);
    }

    pub fn try_recv_result(&self) -> Option<TagWriteResult> {
        self.result_rx.try_recv().ok()
    }

    /// XMP 書き込みと Tantivy commit の両方が完了するまで busy を維持する。
    /// `done == total` だけでは commit 前に完了 toast が出て、その直後の Ctrl+G で
    /// 新タグが見えない race が発生するため、`pending_in_writer > 0` も busy 扱いにする。
    pub fn is_busy(&self) -> bool {
        self.total.load(Ordering::Relaxed) != self.done.load(Ordering::Relaxed)
            || self.pending_in_writer.load(Ordering::Relaxed) > 0
    }

    pub fn reset_counters_if_idle(&self) {
        if !self.is_busy() {
            self.total.store(0, Ordering::Relaxed);
            self.done.store(0, Ordering::Relaxed);
            self.failures.store(0, Ordering::Relaxed);
        }
    }
}

impl Drop for TagWriteHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    job_rx: &Receiver<TagWriteJob>,
    result_tx: &Sender<TagWriteResult>,
    meta: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    shared_writer: Arc<Mutex<IndexWriter>>,
    done: &Arc<AtomicUsize>,
    failures: &Arc<AtomicUsize>,
    pending_in_writer: &Arc<AtomicUsize>,
    shutdown: &Arc<AtomicBool>,
) {
    // バッチ commit 用の pending カウンタ。ingest_worker と同じ「N 件溜まったら
    // commit / idle で M ms 経ったら commit / 終了時に必ず flush」パターン。
    // これをやらないと N ファイル一括トグルで N 回 fsync が発生する。
    // `pending_in_writer` は UI 側の `is_busy()` と同期した外部ミラー。
    let mut last_flush = Instant::now();

    loop {
        if shutdown.load(Ordering::Relaxed) && job_rx.is_empty() {
            break;
        }
        let job = match job_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(j) => j,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if pending_in_writer.load(Ordering::Relaxed) > 0
                    && last_flush.elapsed() >= BATCH_FLUSH_INTERVAL
                {
                    flush_commit(&shared_writer, &fts, pending_in_writer);
                    last_flush = Instant::now();
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        let (res, dirtied) = process_job(&job, &meta, &fts, &shared_writer);
        if res.is_err() {
            failures.fetch_add(1, Ordering::Relaxed);
        }
        if dirtied {
            pending_in_writer.fetch_add(1, Ordering::Relaxed);
        }

        // Flush 判断は結果通知 + done 更新より **先** に行う。そうしないと UI 側の
        // `is_busy()` が total == done を観測して完了 toast を出した直後に、commit 前の
        // Ctrl+G が走って新タグを見つけられないバグが発生する (pending_in_writer も busy に
        // 含めているので最悪でも race window は 1 atomic op 分)。
        // 単発トグルを即反映させるため、キューが空なら閾値未満でも flush する。
        let pending_now = pending_in_writer.load(Ordering::Relaxed);
        if pending_now >= BATCH_FLUSH_COUNT || (pending_now > 0 && job_rx.is_empty()) {
            flush_commit(&shared_writer, &fts, pending_in_writer);
            last_flush = Instant::now();
        }

        done.fetch_add(1, Ordering::Relaxed);
        let _ = result_tx.send(TagWriteResult {
            path: job.path.clone(),
            result: res.map_err(|e| e.to_string()),
        });
    }
    // 終了時に残ピンを flush。忘れると最後のジョブが検索に反映されない。
    flush_commit(&shared_writer, &fts, pending_in_writer);
}

/// Tantivy writer を commit + reader reload する。pending が 0 なら no-op。
/// commit 失敗時は reader reload を走らせない (stale 読みを防ぐ)。
fn flush_commit(
    shared_writer: &Mutex<IndexWriter>,
    fts: &FtsIndex,
    pending_in_writer: &AtomicUsize,
) {
    if pending_in_writer.load(Ordering::Relaxed) == 0 {
        return;
    }
    // 取得待ち時間も計測 — 索引ワーカーに lock を取られているとここで詰まる。
    // 1 秒以上待ったら警告を出す (CLAUDE.md 並行処理ガイダンス)。
    let lock_t0 = std::time::Instant::now();
    let lock_result = shared_writer.lock();
    let lock_wait_ms = lock_t0.elapsed().as_millis();
    if lock_wait_ms > 1000 {
        crate::logger::log(format!(
            "[TAG] worker: writer lock acquired after {lock_wait_ms} ms wait \
             (indexer scan likely held it — see ingest_worker yield_writer_lock)"
        ));
    }
    let committed = match lock_result {
        Ok(mut w) => match w.commit() {
            Ok(_) => true,
            Err(e) => {
                crate::logger::log(format!("tag_write_worker: commit: {e}"));
                false
            }
        },
        Err(_) => {
            crate::logger::log(
                "tag_write_worker: shared writer mutex poisoned — skipping commit".to_string(),
            );
            false
        }
    };
    // Reset after commit attempt. 成功しなかった場合も pending は 0 に戻す (次回ジョブで
    // dirty 判定がやり直され、いずれ別の commit 機会で再試行される)。
    pending_in_writer.store(0, Ordering::Relaxed);
    if committed {
        if let Err(e) = fts.reload_reader() {
            crate::logger::log(format!("tag_write_worker: reload_reader: {e}"));
        }
    }
}

/// ジョブを 1 件処理する。戻り値の `bool` は「Tantivy writer バッファを dirty にしたか」で、
/// 呼び出し側 (`run_worker`) がバッチ commit の pending カウンタに使う。
fn process_job(
    job: &TagWriteJob,
    meta: &FtsMetaDb,
    fts: &FtsIndex,
    shared_writer: &Mutex<IndexWriter>,
) -> (Result<TagAction, WriteError>, bool) {
    let path_disp = job.path.display();
    // Toggle は worker 側で現在タグを読んで Add/Remove に解決する。
    // これで UI スレッドからの同期 I/O を不要にできる。
    // UI 側トースト用に、どちらに解決されたかを `TagAction` で返す。
    let (op, action) = match &job.kind {
        TagJobKind::Toggle(name) => {
            let current = crate::xmp_reader::read_dc_subject(&job.path);
            let with_hash = format!("#{name}");
            let already_has = current.iter().any(|t| *t == with_hash);
            crate::logger::log(format!(
                "[TAG] worker: read dc:subject → {current:?} (looking for {with_hash:?}) → {} | {path_disp}",
                if already_has { "REMOVE" } else { "ADD" }
            ));
            if already_has {
                (TagOp::Remove(with_hash), TagAction::Removed)
            } else {
                (TagOp::Add(with_hash), TagAction::Added)
            }
        }
        TagJobKind::ClearMiv => {
            let current = crate::xmp_reader::read_dc_subject(&job.path);
            let had = current.iter().any(|t| t.starts_with('#'));
            crate::logger::log(format!(
                "[TAG] worker: ClearMiv read dc:subject → {current:?} (had #-tags={had}) | {path_disp}"
            ));
            (TagOp::ClearMiv, if had { TagAction::Cleared } else { TagAction::NoOp })
        }
    };

    let new_tags = match crate::xmp_writer::apply_tag_op(&job.path, &op) {
        Ok(s) => s,
        Err(e) => {
            crate::logger::log(format!(
                "[TAG] worker: apply_tag_op FAILED ({e}) | {path_disp}"
            ));
            return (Err(e), false);
        }
    };
    crate::logger::log(format!(
        "[TAG] worker: write OK, new tags column = {new_tags:?} | {path_disp}"
    ));

    // 検索インデックス即時更新 (favorite_id がわかる時だけ)。
    let dirtied = match job.favorite_id {
        Some(fav_id) => upsert_tags_in_writer(&job.path, fav_id, &new_tags, meta, fts, shared_writer),
        None => {
            crate::logger::log(format!(
                "[TAG] worker: skip fts_meta upsert (no favorite_id) | {path_disp}"
            ));
            false
        }
    };
    (Ok(action), dirtied)
}

/// `fts_meta.tags_norm` を更新し、Tantivy writer にタグ差分のみの upsert を投入する。
/// commit / reload は呼び出し側がバッチ境界で行う (1 ジョブ 1 fsync を避けるため)。
///
/// 戻り値 `true`: writer に upsert を push した (呼び出し側は flush の pending に計上)。
/// 戻り値 `false`: 変更なし / fts_meta 書き込み失敗 / 行が無い など、writer に触っていない。
fn upsert_tags_in_writer(
    path: &std::path::Path,
    fav_id: Uuid,
    new_tags: &str,
    meta: &FtsMetaDb,
    fts: &FtsIndex,
    shared_writer: &Mutex<IndexWriter>,
) -> bool {
    let key = crate::search_index_db::normalize_path(path);
    let Ok(Some(row)) = meta.get(&key) else {
        // 行が無い (未インデックス favorite など) → notify-rs 経由の再 ingest に任せる
        return false;
    };
    // Dirty guard: new_tags が既存 tags_norm と同じなら Tantivy を触らない (no-op commit 回避)。
    // ClearMiv を #タグ無しの行に当てたケースや、Toggle で結果が元に戻るケースを省く。
    if row.norms.tags == new_tags {
        return false;
    }
    if meta.set_tags(&key, new_tags).is_err() {
        return false;
    }
    let mut norms = row.norms.clone();
    norms.tags = new_tags.to_string();
    let doc = IndexDoc {
        path: key,
        container: Container::Fs,
        zip_entry: String::new(),
        favorite_id: fav_id,
        kind: row.kind,
        mtime: row.mtime,
        file_size: row.file_size,
        norms,
    };
    let lock_t0 = std::time::Instant::now();
    let lock_result = shared_writer.lock();
    let lock_wait_ms = lock_t0.elapsed().as_millis();
    if lock_wait_ms > 1000 {
        crate::logger::log(format!(
            "[TAG] worker: writer lock for upsert acquired after {lock_wait_ms} ms wait"
        ));
    }
    match lock_result {
        Ok(mut w) => match fts_index::upsert_doc(&mut *w, fts.fields(), &doc) {
            Ok(_) => true,
            Err(e) => {
                crate::logger::log(format!("tag_write_worker: upsert_doc: {e}"));
                false
            }
        },
        Err(_) => {
            crate::logger::log(
                "tag_write_worker: writer mutex poisoned — skipping upsert".to_string(),
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Toggle ロジックは process_job 内で XMP 読みと直結しているため、
    // 統合テストは実ファイルが必要。単体テストは apply_tag_op 側 (xmp_writer::tests) で
    // 網羅してあるので、ここでは API 型の sanity check のみ。

    #[test]
    fn job_kinds_clone() {
        let j1 = TagJobKind::Toggle("tag".into());
        let j2 = TagJobKind::ClearMiv;
        assert!(matches!(j1.clone(), TagJobKind::Toggle(_)));
        assert!(matches!(j2.clone(), TagJobKind::ClearMiv));
    }

    /// `is_busy()` は `pending_in_writer > 0` も busy 扱いにする。
    /// これで「XMP 書き込みは done カウントに反映されたが commit 前」の窓で
    /// UI が完了 toast を出してしまう race を塞ぐ。
    #[test]
    fn is_busy_reflects_pending_in_writer() {
        let total = Arc::new(AtomicUsize::new(1));
        let done = Arc::new(AtomicUsize::new(1));
        let failures = Arc::new(AtomicUsize::new(0));
        let pending_in_writer = Arc::new(AtomicUsize::new(1));
        let (_job_tx, _job_rx) = unbounded::<TagWriteJob>();
        let (_result_tx, result_rx) = unbounded::<TagWriteResult>();
        let shutdown = Arc::new(AtomicBool::new(false));

        // 実スレッドを使わずに、handle だけ組み立てて is_busy の論理を検証する。
        // (Arc を流用、worker スレッド無しなので _thread は None)
        let handle = TagWriteHandle {
            job_tx: _job_tx,
            result_rx,
            total,
            done,
            failures,
            pending_in_writer: pending_in_writer.clone(),
            _thread: None,
            shutdown,
        };

        // total == done でも、pending_in_writer > 0 なら busy。
        assert!(handle.is_busy(), "commit 前は busy を維持する");

        // commit 後 (pending_in_writer = 0) に busy が下がる。
        pending_in_writer.store(0, Ordering::Relaxed);
        assert!(!handle.is_busy(), "commit 後は busy=false");
    }
}
