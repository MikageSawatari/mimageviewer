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

use crossbeam_channel::{Receiver, Sender, unbounded};
use tantivy::IndexWriter;
use uuid::Uuid;

use crate::fts_index::{self, Container, FtsIndex, IndexDoc};
use crate::fts_meta::FtsMetaDb;
use crate::xmp_writer::{TagOp, WriteError};

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

#[derive(Debug, Clone)]
pub struct TagWriteResult {
    pub path: PathBuf,
    pub result: Result<String, String>,
}

pub struct TagWriteHandle {
    job_tx: Sender<TagWriteJob>,
    result_rx: Receiver<TagWriteResult>,
    pub total: Arc<AtomicUsize>,
    pub done: Arc<AtomicUsize>,
    pub failures: Arc<AtomicUsize>,
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
        let shutdown = Arc::new(AtomicBool::new(false));

        let w_done = done.clone();
        let w_failures = failures.clone();
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

    pub fn is_busy(&self) -> bool {
        self.total.load(Ordering::Relaxed) != self.done.load(Ordering::Relaxed)
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

fn run_worker(
    job_rx: &Receiver<TagWriteJob>,
    result_tx: &Sender<TagWriteResult>,
    meta: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    shared_writer: Arc<Mutex<IndexWriter>>,
    done: &Arc<AtomicUsize>,
    failures: &Arc<AtomicUsize>,
    shutdown: &Arc<AtomicBool>,
) {
    loop {
        if shutdown.load(Ordering::Relaxed) && job_rx.is_empty() {
            break;
        }
        let job = match job_rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(j) => j,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        let res = process_job(&job, &meta, &fts, &shared_writer);
        if res.is_err() {
            failures.fetch_add(1, Ordering::Relaxed);
        }
        done.fetch_add(1, Ordering::Relaxed);

        let _ = result_tx.send(TagWriteResult {
            path: job.path.clone(),
            result: res.map_err(|e| e.to_string()),
        });
    }
}

fn process_job(
    job: &TagWriteJob,
    meta: &FtsMetaDb,
    fts: &FtsIndex,
    shared_writer: &Mutex<IndexWriter>,
) -> Result<String, WriteError> {
    // Toggle は worker 側で現在タグを読んで Add/Remove に解決する。
    // これで UI スレッドからの同期 I/O を不要にできる。
    let op = match &job.kind {
        TagJobKind::Toggle(name) => {
            let current = crate::xmp_reader::read_dc_subject(&job.path);
            let with_hash = format!("#{name}");
            if current.iter().any(|t| *t == with_hash) {
                TagOp::Remove(with_hash)
            } else {
                TagOp::Add(with_hash)
            }
        }
        TagJobKind::ClearMiv => TagOp::ClearMiv,
    };

    let new_tags = crate::xmp_writer::apply_tag_op(&job.path, &op)?;

    // 検索インデックス即時更新 (favorite_id がわかる時だけ)。
    // 失敗しても書き込み自体は成功扱いとし、notify-rs 経由の再 ingest に任せる。
    if let Some(fav_id) = job.favorite_id {
        update_search_index(&job.path, fav_id, &new_tags, meta, fts, shared_writer);
    }
    Ok(new_tags)
}

/// fts_meta.tags 更新 + Tantivy の tags フィールドを共有 writer 経由で upsert。
fn update_search_index(
    path: &std::path::Path,
    fav_id: Uuid,
    new_tags: &str,
    meta: &FtsMetaDb,
    fts: &FtsIndex,
    shared_writer: &Mutex<IndexWriter>,
) {
    let key = crate::search_index_db::normalize_path(path);
    if meta.set_tags(&key, new_tags).is_err() {
        return;
    }
    let Ok(Some(row)) = meta.get(&key) else {
        return;
    };
    // row.norms を base にして tags 列だけ更新 (他の per-source テキストは保持)。
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
    // 共有 writer を短時間だけロック。Codex P2 #2: タグ書き込み後の Ctrl+G 反映を即時化する
    // ため、ここで **commit + reader reload** まで行う。旧実装は ingest_worker の flush を
    // 当てにしていたが、notify-rs の watcher が missed / off / アプリ終了のケースで次の
    // commit まで新タグが Ctrl+G に反映されず、ユーザ契約「書き込み直後に検索で見える」が崩れていた。
    if let Ok(mut w) = shared_writer.lock() {
        if let Err(e) = fts_index::upsert_doc(&mut *w, fts.fields(), &doc) {
            crate::logger::log(format!("tag_write_worker: upsert_doc: {e}"));
            return;
        }
        if let Err(e) = w.commit() {
            crate::logger::log(format!("tag_write_worker: commit: {e}"));
            return;
        }
    }
    // commit は writer ロック外で reader reload する (他 writer をブロックしないため)。
    if let Err(e) = fts.reload_reader() {
        crate::logger::log(format!("tag_write_worker: reload_reader: {e}"));
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
}
