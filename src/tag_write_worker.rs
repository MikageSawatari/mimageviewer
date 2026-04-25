//! タグ書き込みのバックグラウンド worker (docs/tag-feature.md §5.6)。
//!
//! UI からの「タグ X をトグル」「すべてクリア」要求を受け取り、1 ファイルずつ
//! シリアルに `xmp_writer::apply_tag_op` を実行する。書き込み成功後は
//! `fts_meta.set_tags` + 共有 Tantivy writer 経由で index を即時更新し、
//! 次の Ctrl+G に反映させる。
//!
//! # Tantivy writer 共有
//!
//! Tantivy は 1 Index につき IndexWriter を 1 本しか許さないため、`IndexerManager` が
//! 保有する `Arc<FtsWriterDispatcher>` を共有して使う。タグ書き込みは
//! `WriterPriority::Interactive` で submit され、background ingest の sub-batch
//! 境界で必ず割り込めるので、起動直後の大規模スキャン中でも応答性が保たれる。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use uuid::Uuid;

use crate::fts_index::{Container, FtsIndex, IndexDoc};
use crate::fts_meta::FtsMetaDb;
use crate::fts_writer_dispatcher::{FtsWriterDispatcher, WriterPriority};
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
    /// `dc:subject` を指定リストで完全置換する。Undo/Redo で「操作直前の状態」に
    /// 戻すために使う。Toggle の逆操作だと外部ツールでの書き換え後にズレるが、
    /// この置換ジョブなら mIV が記録した状態へ確実に戻せる。
    SetTags(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct TagWriteJob {
    pub path: PathBuf,
    pub kind: TagJobKind,
    /// 検索インデックス更新用の favorite_id。None なら Tantivy upsert をスキップ
    /// (次回 notify-rs re-ingest で反映)。
    pub favorite_id: Option<Uuid>,
}

/// `Toggle` / `ClearMiv` / `SetTags` が実際に何をしたかを UI に返すためのラベル。
/// 完了トーストで「付与 / 削除」の実際値を見せるのに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagAction {
    /// Toggle: タグを追加した (Add 経路に解決された)。
    Added,
    /// Toggle: タグを削除した (Remove 経路に解決された)。
    Removed,
    /// ClearMiv: `#` 始まりの要素をまとめて削除した (1 件以上の削除が発生)。
    Cleared,
    /// SetTags: Undo/Redo による状態復元で `dc:subject` を置き換えた。
    Restored,
    /// 実質変化なし (clear した時に元々空だったケース等)。
    NoOp,
}

#[derive(Debug, Clone)]
pub struct TagWriteResult {
    pub path: PathBuf,
    pub result: Result<TagAction, String>,
    /// 書き込み後の dc:subject 一覧 (成功時のみ意味あり、失敗時は空)。
    /// UI 側はこれを `tags_cache` に直接書き戻すことで、fts_meta に行が無い
    /// (未インデックス favorite 等) ファイルでもグリッドバッジが即時反映される。
    pub tags_after: Vec<String>,
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
    /// worker スレッドを起動する。`writer` は `IndexerManager::clone_shared_writer()` 由来の
    /// `FtsWriterDispatcher` を渡す。タグ書き込みは `WriterPriority::Interactive` で submit され、
    /// 並行する indexer の Background batch より先に処理される。
    pub fn spawn(
        meta: Arc<FtsMetaDb>,
        fts: Arc<FtsIndex>,
        writer: Arc<FtsWriterDispatcher>,
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
                    writer,
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
    writer: Arc<FtsWriterDispatcher>,
    done: &Arc<AtomicUsize>,
    failures: &Arc<AtomicUsize>,
    pending_in_writer: &Arc<AtomicUsize>,
    shutdown: &Arc<AtomicBool>,
) {
    // バッチ commit 用の pending カウンタ。N 件溜まったら commit / idle で M ms 経ったら
    // commit / 終了時に必ず flush。これをやらないと N ファイル一括トグルで N 回 fsync。
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
                    flush_commit(&writer, &fts, pending_in_writer);
                    last_flush = Instant::now();
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        let (res, dirtied, tags_after) = process_job(&job, &meta, &fts, &writer);
        if res.is_err() {
            failures.fetch_add(1, Ordering::Relaxed);
        }
        if dirtied {
            pending_in_writer.fetch_add(1, Ordering::Relaxed);
        }

        // Flush 判断は結果通知 + done 更新より **先** に行う (race window 抑制)。
        // 単発トグルを即反映させるため、キューが空なら閾値未満でも flush。
        let pending_now = pending_in_writer.load(Ordering::Relaxed);
        if pending_now >= BATCH_FLUSH_COUNT || (pending_now > 0 && job_rx.is_empty()) {
            flush_commit(&writer, &fts, pending_in_writer);
            last_flush = Instant::now();
        }

        done.fetch_add(1, Ordering::Relaxed);
        let _ = result_tx.send(TagWriteResult {
            path: job.path.clone(),
            result: res.map_err(|e| e.to_string()),
            tags_after,
        });
    }
    // 終了時に残ピンを flush。
    flush_commit(&writer, &fts, pending_in_writer);
}

/// dispatcher 経由で commit + reader reload を依頼する。pending が 0 なら no-op。
/// dispatcher は Interactive 優先度のジョブを Background より先に処理するので、
/// indexer の長時間 batch 中でも 1 sub-batch (1〜2 秒) 以内に応答が返る。
fn flush_commit(
    writer: &FtsWriterDispatcher,
    fts: &FtsIndex,
    pending_in_writer: &AtomicUsize,
) {
    if pending_in_writer.load(Ordering::Relaxed) == 0 {
        return;
    }
    let t0 = std::time::Instant::now();
    let res = writer.commit(true /* reload */, WriterPriority::Interactive);
    let wait_ms = t0.elapsed().as_millis();
    if wait_ms > 1000 {
        crate::logger::log(format!(
            "[TAG] worker: dispatcher commit took {wait_ms} ms (background ingest in flight?)"
        ));
    }
    if let Err(e) = res {
        crate::logger::log(format!("tag_write_worker: commit failed: {e}"));
    }
    let _ = fts; // reload は dispatcher が行う
    // Reset after commit attempt. 失敗時も pending は 0 に戻して次回 commit を待つ。
    pending_in_writer.store(0, Ordering::Relaxed);
}

/// ジョブを 1 件処理する。戻り値は:
/// - `Result<TagAction, WriteError>`: UI 側トースト用の結果ラベル
/// - `bool` dirtied: dispatcher に upsert を投げたか (呼び出し側がバッチ commit の pending に使う)
/// - `Vec<String>` tags_after: 書き込み後の dc:subject 一覧 (エラー時は空)。UI が
///   `tags_cache` に直接書き戻して、fts_meta 行の有無に依存せず grid バッジを更新するのに使う。
fn process_job(
    job: &TagWriteJob,
    meta: &FtsMetaDb,
    fts: &FtsIndex,
    writer: &FtsWriterDispatcher,
) -> (Result<TagAction, WriteError>, bool, Vec<String>) {
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
        TagJobKind::SetTags(target) => {
            // Codex P3: SetTags は Undo/Redo 経路でしか使わないので、disk が既に target に
            // 一致していても (変化ゼロでも) `Restored` を返す。`NoOp` を返すと
            // tag_ops の `format_completion_toast` で「mIV タグをクリア」誤表示になるため。
            let current = crate::xmp_reader::read_dc_subject(&job.path);
            let changed = current != *target;
            crate::logger::log(format!(
                "[TAG] worker: SetTags current={current:?} target={target:?} (changed={changed}) | {path_disp}"
            ));
            (TagOp::Set(target.clone()), TagAction::Restored)
        }
    };

    let new_tags = match crate::xmp_writer::apply_tag_op(&job.path, &op) {
        Ok(s) => s,
        Err(e) => {
            crate::logger::log(format!(
                "[TAG] worker: apply_tag_op FAILED ({e}) | {path_disp}"
            ));
            return (Err(e), false, Vec::new());
        }
    };
    crate::logger::log(format!(
        "[TAG] worker: write OK, new tags column = {new_tags:?} | {path_disp}"
    ));
    let tags_after = crate::ingest_text::parse_tags_column(&new_tags);

    // 検索インデックス即時更新 (favorite_id がわかる時だけ)。
    let dirtied = match job.favorite_id {
        Some(fav_id) => upsert_tags_via_dispatcher(&job.path, fav_id, &new_tags, meta, fts, writer),
        None => {
            crate::logger::log(format!(
                "[TAG] worker: skip fts upsert (no favorite_id) | {path_disp}"
            ));
            false
        }
    };
    (Ok(action), dirtied, tags_after)
}

/// 既存の Tantivy doc から他ソースのテキストを引き継ぎつつ、`tags` フィールドだけ
/// 差し替えて upsert を依頼する。INDEX_VERSION=5 以降は fts_meta.db に norms が
/// 無くなったため、原文の取り出しは Tantivy 側 (STORED) から行う。
///
/// 戻り値 `true`: dispatcher に upsert を投げた (呼び出し側は flush の pending に計上)。
/// 戻り値 `false`: 変更なし / 該当 doc が Tantivy に未投入 (pending) など、writer に触っていない。
fn upsert_tags_via_dispatcher(
    path: &std::path::Path,
    fav_id: Uuid,
    new_tags: &str,
    meta: &FtsMetaDb,
    fts: &FtsIndex,
    writer: &FtsWriterDispatcher,
) -> bool {
    let key = crate::search_index_db::normalize_path(path);
    // 管理メタ (kind / mtime / file_size) は fts_meta.db から引く。
    // 行が無い (未インデックス favorite など) なら notify-rs 経由の再 ingest に任せる。
    let Ok(Some(row)) = meta.get(&key) else {
        return false;
    };
    // 既存 Tantivy doc から他ソース原文を引き継ぐ。doc が無い (= まだ pending) なら
    // 通常 ingest に任せて何もしない。
    let searcher = fts.searcher();
    let addr = match crate::fts_index::find_doc_by_path(&searcher, fts.fields(), &key) {
        Ok(Some(a)) => a,
        _ => return false,
    };
    let mut norms = match crate::fts_index::doc_per_source_text(&searcher, fts.fields(), addr) {
        Ok(n) => n,
        Err(_) => return false,
    };
    if norms.tags == new_tags {
        return false;
    }
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
    let t0 = std::time::Instant::now();
    let res = writer.upsert(doc, WriterPriority::Interactive);
    let wait_ms = t0.elapsed().as_millis();
    if wait_ms > 1000 {
        crate::logger::log(format!(
            "[TAG] worker: dispatcher upsert took {wait_ms} ms"
        ));
    }
    match res {
        Ok(()) => true,
        Err(e) => {
            crate::logger::log(format!("tag_write_worker: upsert_doc: {e}"));
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
        let j3 = TagJobKind::SetTags(vec!["#a".into(), "#b".into()]);
        assert!(matches!(j1.clone(), TagJobKind::Toggle(_)));
        assert!(matches!(j2.clone(), TagJobKind::ClearMiv));
        assert!(matches!(j3.clone(), TagJobKind::SetTags(_)));
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
