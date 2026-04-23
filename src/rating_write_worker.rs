//! レーティング書き込みのバックグラウンド worker。
//!
//! 環境設定「レーティングを XMP にも書き込む」が ON のとき、`App::set_rating` は
//! DB 更新と並行してこの worker にジョブを投げる。worker は 1 ファイルずつシリアルに
//! `xmp_writer::apply_rating` を実行し、結果 (成功 / 失敗) を mpsc で UI に返す。
//! UI 側は `poll_rating_write_results` で回収し、失敗があればトーストで通知する。
//!
//! タグ書き込み ([`crate::tag_write_worker`]) と違って:
//! - レーティングは全文検索索引の対象外なので Tantivy / fts_meta の更新は不要
//! - UI は rating_db に即時反映しているので worker は「XMP だけ後追いで揃える」役目
//!
//! そのため実装はタグ書き込み worker よりだいぶ小さい。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::xmp_writer;

#[derive(Debug, Clone)]
pub struct RatingWriteJob {
    pub path: PathBuf,
    /// 書き込む値。`None` / `Some(0)` は `xmp:Rating` を削除する (= 未評価)。
    pub rating: Option<u8>,
}

#[derive(Debug)]
pub struct RatingWriteResult {
    pub path: PathBuf,
    pub result: Result<(), String>,
}

pub struct RatingWriteHandle {
    job_tx: Sender<RatingWriteJob>,
    result_rx: Receiver<RatingWriteResult>,
    pub done: Arc<AtomicUsize>,
    pub failures: Arc<AtomicUsize>,
    _thread: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl RatingWriteHandle {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = unbounded::<RatingWriteJob>();
        let (result_tx, result_rx) = unbounded::<RatingWriteResult>();
        let done = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let w_done = done.clone();
        let w_failures = failures.clone();
        let w_shutdown = shutdown.clone();
        let handle = std::thread::Builder::new()
            .name("rating-write-worker".into())
            .spawn(move || {
                run_worker(&job_rx, &result_tx, &w_done, &w_failures, &w_shutdown);
            })
            .expect("rating-write-worker spawn");

        Self {
            job_tx,
            result_rx,
            done,
            failures,
            _thread: Some(handle),
            shutdown,
        }
    }

    pub fn submit(&self, job: RatingWriteJob) {
        let _ = self.job_tx.send(job);
    }

    pub fn try_recv_result(&self) -> Option<RatingWriteResult> {
        self.result_rx.try_recv().ok()
    }
}

impl Drop for RatingWriteHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // on_exit → App::drop → この Drop の流れで、worker が在庫ジョブを drain
        // し切るのを待つ。join しないと XMP 未書き出しのままプロセスが落ちる。
        if let Some(thread) = self._thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_worker(
    job_rx: &Receiver<RatingWriteJob>,
    result_tx: &Sender<RatingWriteResult>,
    done: &AtomicUsize,
    failures: &AtomicUsize,
    shutdown: &AtomicBool,
) {
    // Drop 時の shutdown=true だけで break するとキューに残ったジョブが捨てられ、
    // DB には書いたのに XMP には書いていないファイルが出る。tag_write_worker と同じく
    // 「shutdown かつキュー空」のときだけ break する drain-on-shutdown 方式にする。
    loop {
        if shutdown.load(Ordering::Relaxed) && job_rx.is_empty() {
            break;
        }
        let Ok(job) = job_rx.recv_timeout(std::time::Duration::from_millis(200)) else {
            continue;
        };
        let result = xmp_writer::apply_rating(&job.path, job.rating).map_err(|e| e.to_string());
        if result.is_err() {
            failures.fetch_add(1, Ordering::Relaxed);
        }
        done.fetch_add(1, Ordering::Relaxed);
        let _ = result_tx.send(RatingWriteResult {
            path: job.path,
            result,
        });
    }
}

