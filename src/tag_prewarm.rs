//! XMP rating hydration 用のバックグラウンドプリフェッチ。
//!
//! 旧タグ機能時代は XMP `dc:subject` もここで読んでいたが、タグ正本は `tags.db` に
//! 移ったため、この worker は設定 ON 時の `xmp:Rating` 取り込みだけを担当する。
//!
//! UI スレッドで同期読みすると HDD で数百 ms〜秒オーダーでブロックし得るため、
//! この worker が XMP 読みを背景スレッドへ逃がし、結果を `mpsc` で UI に返す。
//!
//! # インクリメンタル投入
//!
//! `spawn()` は空キューで worker を起動し、UI が毎フレーム
//! `App::enqueue_visible_tag_prewarms` から可視範囲近傍だけを `push_job` でキューに積む。
//! スクロールに追従して必要な分だけ読むため、
//! 大規模フォルダで全 XMP を舐める暴走を避ける。
//!
//! # キャンセル
//!
//! - フォルダ切替で旧 pending が `take()` されて `cancel()` + drop される
//!   (AtomicBool セット + job_tx 閉鎖 → worker が break)。
//! - UI 側が `pending = None` にすれば job_tx も drop され、worker は自然終了する。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crossbeam_channel::{Receiver as CbReceiver, Sender as CbSender, unbounded};

/// バックグラウンド XMP プリフェッチのハンドル。`job_tx` は UI が新規ジョブを push する
/// ための送信端。`rx` は worker が XMP rating 読みの結果を返す経路
/// (`App::poll_tag_prewarm_results` が drain する)。`in_flight` は push 済みだが
/// UI が drain していないジョブ数。
pub(crate) struct TagPrewarmPending {
    cancel: Arc<AtomicBool>,
    job_tx: CbSender<PrewarmJob>,
    pub rx: mpsc::Receiver<TagPrewarmResult>,
    /// 「push されたが UI がまだ drain していない」ジョブ数。worker が空でも
    /// 常に `Some(pending)` になった現設計で、idle 時に `request_repaint` を
    /// 無限に呼び続けないための busy シグナル源。push で +1、`on_result_drained` で -1。
    in_flight: Arc<AtomicUsize>,
}

impl TagPrewarmPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// ジョブを 1 件キューに追加する。worker が順に処理する。
    /// 重複チェックは UI 側 (`tag_prewarm_queued` HashSet) で済ませる想定。
    /// worker は XMP パケットから xmp:Rating を抽出して `TagPrewarmResult::rating` に
    /// 載せて返す (「rating を読まないジョブ」は存在しない — 読まないなら push しない)。
    pub(crate) fn push_job(&self, path: PathBuf, rating_generation: u64) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let _ = self.job_tx.send(PrewarmJob {
            path,
            rating_generation,
        });
    }

    /// UI が 1 件 drain した後に呼ぶ。`is_busy` を下げる。
    pub(crate) fn on_result_drained(&self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// push 済みで UI がまだ drain していないジョブが残っているか。
    /// `App::update` の repaint ゲートで使い、idle (0) 時は repaint 要求を出さない。
    pub(crate) fn is_busy(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed) > 0
    }
}

struct PrewarmJob {
    path: PathBuf,
    /// XMP 読み取りを投入した時点の App-global path rating 世代。
    rating_generation: u64,
}

/// 1 ファイル分のプリフェッチ結果。
/// `rating` は設定 ON かつ XMP に xmp:Rating が存在した場合のみ `Some` になる。
pub(crate) struct TagPrewarmResult {
    pub path: PathBuf,
    pub rating: Option<u8>,
    pub rating_generation: u64,
}

/// worker スレッドを起動する。フォルダ切替時に 1 回呼ぶ。
/// ジョブは後から `push_job` で逐次投入する (初期キューは空)。
pub(crate) fn spawn() -> TagPrewarmPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let in_flight = Arc::new(AtomicUsize::new(0));
    let (job_tx, job_rx) = unbounded::<PrewarmJob>();
    let (result_tx, result_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tag-prewarm".into())
        .spawn(move || run_worker(&job_rx, &result_tx, &cancel_w))
        .ok();
    TagPrewarmPending {
        cancel,
        job_tx,
        rx: result_rx,
        in_flight,
    }
}

/// ファイルを 1 回だけ read して、XMP パケットから xmp:Rating を抜き出す。
fn read_xmp_rating(path: &std::path::Path) -> Option<u8> {
    if crate::xmp_writer::is_video_for_sidecar(path) {
        return None;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return None;
    };
    if !crate::xmp_reader::has_xmp_capable_magic(&bytes) {
        return None;
    }
    let Some(xmp) = crate::xmp_reader::extract_xmp_packet(&bytes) else {
        return None;
    };
    crate::xmp_reader::parse_xmp_rating(&xmp)
}

fn run_worker(
    job_rx: &CbReceiver<PrewarmJob>,
    result_tx: &mpsc::Sender<TagPrewarmResult>,
    cancel: &AtomicBool,
) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match job_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(job) => {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let rating = read_xmp_rating(&job.path);
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if result_tx
                    .send(TagPrewarmResult {
                        path: job.path,
                        rating,
                        rating_generation: job.rating_generation,
                    })
                    .is_err()
                {
                    break; // UI 側が rx を drop した
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 存在しないパスを push しても worker は rating=None で結果を返し、エラーにならないこと。
    #[test]
    fn returns_empty_rating_for_nonexistent_path() {
        let pending = spawn();
        pending.push_job(PathBuf::from("Z:/does/not/exist.jpg"), 7);
        let start = Instant::now();
        loop {
            match pending.rx.try_recv() {
                Ok(res) => {
                    assert_eq!(res.rating, None);
                    assert_eq!(res.rating_generation, 7);
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if start.elapsed() > Duration::from_secs(2) {
                        panic!("worker did not send result within 2s");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("channel closed without result"),
            }
        }
    }

    /// idle 時 (push していない) は `is_busy()` が false を返し、push すると true、
    /// drain 後に再び false に戻ること。`App::update` の repaint ゲート用シグナル。
    #[test]
    fn is_busy_reflects_in_flight() {
        let pending = spawn();
        assert!(!pending.is_busy(), "初期状態は idle");

        pending.push_job(PathBuf::from("Z:/nope/a.jpg"), 0);
        assert!(pending.is_busy(), "push 直後は busy");

        // worker が送ってくるのを受け取り、on_result_drained で in_flight を戻す
        let start = Instant::now();
        loop {
            match pending.rx.try_recv() {
                Ok(_) => {
                    pending.on_result_drained();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if start.elapsed() > Duration::from_secs(2) {
                        panic!("worker did not send within 2s");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("disconnected"),
            }
        }
        assert!(!pending.is_busy(), "drain 後は idle");
    }

    /// cancel を立てれば worker は次ループで break し、drop 後に受信は終わる。
    #[test]
    fn cancel_stops_worker_loop() {
        let pending = spawn();
        for i in 0..1000 {
            pending.push_job(PathBuf::from(format!("Z:/nope/{i}.jpg")), 0);
        }
        pending.cancel();
        drop(pending);
        // パニックせず到達すれば OK (worker は cancel set + rx/job drop で break する)。
    }
}
