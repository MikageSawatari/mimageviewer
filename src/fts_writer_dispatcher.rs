//! Tantivy IndexWriter の優先度付きディスパッチャー (CLAUDE.md 並行処理ガイダンス準拠)。
//!
//! ## 背景
//!
//! Tantivy は 1 Index につき IndexWriter を 1 本しか許さない。
//! 旧設計は `Arc<Mutex<IndexWriter>>` を全 supervisor と interactive job で共有し、
//! 各自 `lock()` していたが、indexer supervisor が 1 回 lock を握ったまま
//! `session.apply` で 67 秒間ハードに使い続けるため、ユーザー操作起点の
//! interactive job が分単位で starve するバグが発生した
//! ([panic.log] 2026-04-22 ユーザー再現 + commit 14037af 参照)。
//!
//! ## 採用パターン
//!
//! `src/pdf_loader.rs::PdfWorkerPool` と同じ「Mutex + Condvar で保護した優先度キュー +
//! 専用ディスパッチャースレッド」構造。リソース利用者は Job を enqueue して
//! `mpsc::Receiver` で応答待ち、ディスパッチャーが `Condvar::wait` で起床して
//! `Interactive` を先に、無ければ `Background` を pop して 1 件処理する。
//!
//! - **`Interactive`** = ユーザー操作起点の小ジョブ。
//! - **`Background`** = indexer supervisor の batch ingest など、長時間ジョブの構成要素。
//!
//! 大規模 batch も「sub-batch (= 100 件 upsert + 1 commit) を `Background` で 1 ジョブずつ
//! submit する」設計にすることで、各 sub-batch の境界で dispatcher が Interactive キューを
//! check できる。interactive job の worst-case 待ち時間 = 1 sub-batch 処理時間 (= 1〜2 秒程度)。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use tantivy::IndexWriter;

use crate::fts_index::{self, FtsIndex, IndexDoc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterPriority {
    /// インタラクティブ操作 (タグ書き込み等)。Background より常に先に処理される。
    Interactive,
    /// バックグラウンド処理 (indexer の batch ingest)。
    Background,
}

/// dispatcher が処理する 1 ジョブ。完了通知は `reply` 経由で同期的に返す。
///
/// `Batch` で複数操作を 1 ジョブにまとめると dispatcher 内で連続実行される (中断されない)。
/// 大規模 ingest は `Batch` を sub-batch サイズ (~100 件) で submit すること — そうすれば
/// 各 sub-batch の境界で Interactive ジョブが割り込める。
enum WriterJob {
    Upsert {
        doc: IndexDoc,
        reply: mpsc::Sender<tantivy::Result<()>>,
    },
    Delete {
        path: String,
        reply: mpsc::Sender<()>,
    },
    Commit {
        reload: bool,
        reply: mpsc::Sender<tantivy::Result<()>>,
    },
    Batch {
        upserts: Vec<IndexDoc>,
        deletes: Vec<String>,
        commit_after: bool,
        reload_after_commit: bool,
        reply: mpsc::Sender<tantivy::Result<()>>,
    },
    /// テスト専用: dispatcher を `dur` 間スリープさせて占拠する。優先度キューの
    /// preempt 動作を検証するために、dispatcher を「ジョブ処理中」状態に固定するのに使う。
    #[cfg(test)]
    TestSleep {
        dur: std::time::Duration,
        reply: mpsc::Sender<()>,
    },
}

#[derive(Default)]
struct Queue {
    interactive: VecDeque<WriterJob>,
    background: VecDeque<WriterJob>,
}

/// ログ prefix (検索しやすさのため固定文字列を一箇所に)。
const LOG_PREFIX: &str = "[fts-dispatcher]";

/// 単一ディスパッチャースレッド + 優先度キュー構造。`start` で起動し、Drop で停止する。
/// 利用者は `upsert` / `delete` / `commit` / `batch` ヘルパ経由で同期的に処理を依頼する。
pub struct FtsWriterDispatcher {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    shutdown: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl FtsWriterDispatcher {
    /// dispatcher を起動する。`writer` の所有権を dispatcher スレッドに渡す。
    /// `fts` は commit 後の reader reload で使う (dispatcher スレッド内に move される)。
    pub fn start(writer: IndexWriter, fts: Arc<FtsIndex>) -> Arc<Self> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let q_clone = Arc::clone(&queue);
        let sd_clone = Arc::clone(&shutdown);
        let thread = std::thread::Builder::new()
            .name("fts-writer-dispatcher".into())
            .spawn(move || run_dispatcher(q_clone, writer, fts, sd_clone))
            .expect("fts-writer-dispatcher spawn");
        Arc::new(Self {
            queue,
            shutdown,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// 1 件 upsert を依頼して完了を待つ。
    pub fn upsert(&self, doc: IndexDoc, priority: WriterPriority) -> tantivy::Result<()> {
        self.submit_with_reply(priority, |reply| WriterJob::Upsert { doc, reply })
    }

    /// 1 件 delete を依頼して完了を待つ (delete は失敗しないので Result 不要)。
    pub fn delete(&self, path: String, priority: WriterPriority) {
        let (tx, rx) = mpsc::channel();
        self.submit(WriterJob::Delete { path, reply: tx }, priority);
        let _ = rx.recv();
    }

    /// commit を依頼して完了を待つ。`reload=true` なら成功時に reader reload も行う。
    pub fn commit(&self, reload: bool, priority: WriterPriority) -> tantivy::Result<()> {
        self.submit_with_reply(priority, |reply| WriterJob::Commit { reload, reply })
    }

    /// upsert / delete を一括で送る (sub-batch)。`commit_after=true` で末尾 commit。
    /// 1 ジョブ = 1 sub-batch なので、これを 100 件ずつ submit するのが背景 ingest の標準形。
    pub fn batch(
        &self,
        upserts: Vec<IndexDoc>,
        deletes: Vec<String>,
        commit_after: bool,
        reload_after_commit: bool,
        priority: WriterPriority,
    ) -> tantivy::Result<()> {
        self.submit_with_reply(priority, |reply| WriterJob::Batch {
            upserts,
            deletes,
            commit_after,
            reload_after_commit,
            reply,
        })
    }

    /// `tantivy::Result` を返す系ジョブ (Upsert / Commit / Batch) の共通実装。
    /// channel 切断は同じ SystemError にマップする。
    fn submit_with_reply<F>(&self, priority: WriterPriority, build: F) -> tantivy::Result<()>
    where
        F: FnOnce(mpsc::Sender<tantivy::Result<()>>) -> WriterJob,
    {
        let (tx, rx) = mpsc::channel();
        self.submit(build(tx), priority);
        rx.recv().unwrap_or_else(|_| {
            Err(tantivy::TantivyError::SystemError(format!(
                "{LOG_PREFIX} reply channel closed"
            )))
        })
    }

    fn submit(&self, job: WriterJob, priority: WriterPriority) {
        let (mtx, cv) = &*self.queue;
        let mut q = mtx.lock().unwrap();
        match priority {
            WriterPriority::Interactive => q.interactive.push_back(job),
            WriterPriority::Background => q.background.push_back(job),
        }
        // notify_one で十分 (dispatcher は 1 スレッドのみ)
        cv.notify_one();
    }

    /// pending 状況のスナップショット (`(interactive, background)` 件数)。perf/log 用。
    pub fn pending_snapshot(&self) -> (usize, usize) {
        let (mtx, _) = &*self.queue;
        let q = mtx.lock().unwrap();
        (q.interactive.len(), q.background.len())
    }

    /// テスト専用: dispatcher に sleep ジョブを submit する。`recv()` を待つと完了通知が来る。
    /// 優先度 preempt の検証で「dispatcher を占拠したまま後続ジョブを enqueue する」のに使う。
    #[cfg(test)]
    fn submit_test_sleep(
        &self,
        dur: std::time::Duration,
        priority: WriterPriority,
    ) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        self.submit(WriterJob::TestSleep { dur, reply: tx }, priority);
        rx
    }
}

impl Drop for FtsWriterDispatcher {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let (_, cv) = &*self.queue;
        cv.notify_all();
        if let Some(t) = self.thread.lock().ok().and_then(|mut g| g.take()) {
            let _ = t.join();
        }
    }
}

/// dispatcher のメインループ。Interactive を最優先、それも無ければ Background を 1 件処理。
/// 空なら `Condvar::wait` で起床待ち。
fn run_dispatcher(
    queue: Arc<(Mutex<Queue>, Condvar)>,
    mut writer: IndexWriter,
    fts: Arc<FtsIndex>,
    shutdown: Arc<AtomicBool>,
) {
    crate::logger::log(format!("{LOG_PREFIX} started"));
    loop {
        let job = {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            loop {
                if shutdown.load(Ordering::Relaxed)
                    && q.interactive.is_empty()
                    && q.background.is_empty()
                {
                    crate::logger::log(format!("{LOG_PREFIX} shutdown clean"));
                    return;
                }
                if let Some(j) = q.interactive.pop_front() {
                    break j;
                }
                if let Some(j) = q.background.pop_front() {
                    break j;
                }
                q = cv.wait(q).unwrap();
            }
        };
        process_job(&mut writer, &fts, job);
    }
}

fn process_job(writer: &mut IndexWriter, fts: &FtsIndex, job: WriterJob) {
    match job {
        WriterJob::Upsert { doc, reply } => {
            let r = fts_index::upsert_doc(writer, fts.fields(), &doc);
            let _ = reply.send(r);
        }
        WriterJob::Delete { path, reply } => {
            fts_index::delete_doc(writer, fts.fields(), &path);
            let _ = reply.send(());
        }
        WriterJob::Commit { reload, reply } => {
            let r = writer.commit();
            if r.is_ok() && reload {
                if let Err(e) = fts.reload_reader() {
                    crate::logger::log(format!("{LOG_PREFIX} reload_reader: {e}"));
                }
            }
            let _ = reply.send(r.map(|_| ()));
        }
        WriterJob::Batch {
            upserts,
            deletes,
            commit_after,
            reload_after_commit,
            reply,
        } => {
            let r = process_batch(
                writer,
                fts,
                upserts,
                deletes,
                commit_after,
                reload_after_commit,
            );
            let _ = reply.send(r);
        }
        #[cfg(test)]
        WriterJob::TestSleep { dur, reply } => {
            std::thread::sleep(dur);
            let _ = reply.send(());
        }
    }
}

fn process_batch(
    writer: &mut IndexWriter,
    fts: &FtsIndex,
    upserts: Vec<IndexDoc>,
    deletes: Vec<String>,
    commit_after: bool,
    reload_after_commit: bool,
) -> tantivy::Result<()> {
    for path in &deletes {
        fts_index::delete_doc(writer, fts.fields(), path);
    }
    for doc in &upserts {
        fts_index::upsert_doc(writer, fts.fields(), doc)?;
    }
    if commit_after {
        writer.commit()?;
        if reload_after_commit {
            if let Err(e) = fts.reload_reader() {
                crate::logger::log(format!("{LOG_PREFIX} reload_reader after batch: {e}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_index::{Container, IndexDoc, IndexKind};
    use crate::ingest_text::PerSourceText;
    use crate::search_index_db::normalize_path;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn setup() -> (TempDir, Arc<FtsIndex>, Arc<FtsWriterDispatcher>) {
        let tmp = TempDir::new().unwrap();
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts_index")).unwrap());
        let writer = fts.writer().unwrap();
        let disp = FtsWriterDispatcher::start(writer, Arc::clone(&fts));
        (tmp, fts, disp)
    }

    fn sample_doc(path: &str, fav: Uuid, text: &str) -> IndexDoc {
        let key = normalize_path(&PathBuf::from(path));
        IndexDoc {
            path: key,
            container: Container::Fs,
            zip_entry: String::new(),
            favorite_id: fav,
            kind: IndexKind::Image,
            mtime: 0,
            file_size: 0,
            norms: PerSourceText {
                name: crate::search_norm::normalize_for_match(text),
                ..PerSourceText::default()
            },
        }
    }

    #[test]
    fn upsert_then_commit_makes_doc_searchable() {
        let (_tmp, fts, disp) = setup();
        let fav = Uuid::new_v4();
        disp.upsert(
            sample_doc("c:/a.jpg", fav, "夕焼け 海辺"),
            WriterPriority::Background,
        )
        .unwrap();
        disp.commit(true, WriterPriority::Background).unwrap();

        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["夕焼け"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn interactive_preempts_queued_background() {
        // Codex P3 回帰: 旧テストは upsert が同期戻りなので「Background が完了 →
        // Interactive を投入」となり、両者がキューに並ぶ瞬間が無く優先度を検証していなかった。
        //
        // 新テストの構造:
        // 1. Background の `TestSleep(150ms)` で dispatcher を占拠する
        // 2. dispatcher が sleep 処理中に、別スレッドから Background upsert を 3 本 enqueue
        // 3. 短い猶予を入れて、Interactive upsert を 1 本 enqueue
        // 4. sleep が解放されると dispatcher は順序: Interactive → Background × 3 で処理する
        // 5. 各スレッドは upsert の elapsed を測定 → Interactive が submit 時刻が遅いのに
        //    完了時刻が早い (= 完了 elapsed が 3 つの Background より短い) ことを検証
        let (_tmp, _fts, disp) = setup();
        let fav = Uuid::new_v4();

        // 1. dispatcher を占拠する sleep ジョブ (Background)。500ms あれば parallel test 環境の
        //    スケジューラ揺れでも余裕で同時 enqueue できる。
        let blocker_done = disp.submit_test_sleep(
            std::time::Duration::from_millis(500),
            WriterPriority::Background,
        );
        // dispatcher が blocker を pop して sleep に入るまで pending_snapshot で確認
        // (固定 sleep より確実 — busy load でも race にならない)
        for _ in 0..50 {
            if disp.pending_snapshot() == (0, 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            disp.pending_snapshot(),
            (0, 0),
            "blocker が dispatcher に pop された"
        );

        // 2. Background ジョブ 3 本を別スレッドから enqueue
        let bg_threads: Vec<_> = (0..3)
            .map(|i| {
                let d = Arc::clone(&disp);
                let doc = sample_doc(&format!("c:/bg{i}.jpg"), fav, "bg");
                std::thread::spawn(move || {
                    let t0 = std::time::Instant::now();
                    d.upsert(doc, WriterPriority::Background).unwrap();
                    t0.elapsed()
                })
            })
            .collect();
        // background 3 本がキューに入るまで pending_snapshot で待つ
        for _ in 0..50 {
            if disp.pending_snapshot().1 >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(disp.pending_snapshot().1 >= 3, "background がキュー入り");

        // 3. Interactive ジョブを enqueue (Background 3 本より後に submit)
        let intr_thread = {
            let d = Arc::clone(&disp);
            let doc = sample_doc("c:/intr.jpg", fav, "intr");
            std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                d.upsert(doc, WriterPriority::Interactive).unwrap();
                t0.elapsed()
            })
        };
        // interactive がキューに入った瞬間から 4 件 (background 3 + interactive 1) が並ぶ
        for _ in 0..50 {
            if disp.pending_snapshot().0 >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            disp.pending_snapshot(),
            (1, 3),
            "interactive 1 + background 3 が同時に並ぶ"
        );

        // 4. sleep 解放を待つ。dispatcher は Interactive を先に処理し、その後 background 3 件。
        let _ = blocker_done.recv();

        // 5. 全 elapsed を集計
        let intr_dur = intr_thread.join().unwrap();
        let bg_durs: Vec<_> = bg_threads.into_iter().map(|h| h.join().unwrap()).collect();
        let min_bg = bg_durs.iter().copied().min().unwrap();
        assert!(
            intr_dur < min_bg,
            "interactive ({intr_dur:?}) は全 background ({bg_durs:?}, min={min_bg:?}) \
             より早く完了する必要がある (Background より後に submit したのに!)"
        );
    }

    #[test]
    fn batch_runs_atomically() {
        let (_tmp, fts, disp) = setup();
        let fav = Uuid::new_v4();
        let docs: Vec<_> = (0..10)
            .map(|i| sample_doc(&format!("c:/{i}.jpg"), fav, "夕焼け"))
            .collect();
        disp.batch(docs, vec![], true, true, WriterPriority::Background)
            .unwrap();

        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["夕焼け"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 100).unwrap();
        assert_eq!(hits.len(), 10);
    }
}
