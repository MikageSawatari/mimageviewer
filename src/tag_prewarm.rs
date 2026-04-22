//! グリッド表示用 XMP `dc:subject` のバックグラウンドプリフェッチ。
//!
//! [`App::prewarm_grid_tags`] は第一段階として `fts_meta.db` からタグ列を一括取得するが、
//! インデックス未構築のお気に入り / インデックス対象外のフォルダに居るファイルは
//! そこには載らない (行が無い)。そうしたファイルについては **XMP を直接読む** しか
//! 手段が無く、UI スレッドで同期読みすると HDD で数百ms〜秒オーダーでブロックする。
//!
//! この worker はそれを背景スレッドに逃がす。結果は `mpsc` で UI に返し、
//! `App::poll_tag_prewarm_results` が `tags_cache` へ反映する。キャッシュ反映は
//! `entry().or_insert()` で行うので、タグ書き込み worker が先に載せた最新状態を
//! stale XMP で踏まないようになっている (書き込み worker → cache 先着 → 背景 XMP 後着)。
//!
//! # インクリメンタル投入 (v0.8: P2 対応)
//!
//! 以前はフォルダ展開時に全 Image を一括で `spawn(requests)` に渡していたが、
//! 数千枚フォルダで XMP 全読みが発生して thumbnail I/O を圧迫する問題があった。
//! 現在は `spawn()` で空キューの worker を起動し、UI が毎フレーム
//! `App::enqueue_visible_tag_prewarms` から `keep_range` (可視範囲 + prev/next ページ)
//! 分だけを `push_job` でキューに積む。スクロールに追従して必要な分だけ読む。
//!
//! # キャンセル
//!
//! - フォルダ切替で旧 pending が `take()` されて `cancel()` + drop される
//!   (AtomicBool セット + job_tx 閉鎖 → worker が break)。
//! - UI 側が `pending = None` にすれば job_tx も drop され、worker は自然終了する。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use crossbeam_channel::{Receiver as CbReceiver, Sender as CbSender, unbounded};

/// バックグラウンド XMP プリフェッチのハンドル。`job_tx` は UI が新規ジョブを push する
/// ための送信端。`result_rx` は worker が XMP 読みの結果を返す経路。
pub(crate) struct TagPrewarmPending {
    pub cancel: Arc<AtomicBool>,
    job_tx: CbSender<PrewarmJob>,
    pub rx: mpsc::Receiver<TagPrewarmResult>,
}

impl TagPrewarmPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// ジョブを 1 件キューに追加する。worker が順に処理する。
    /// 重複チェックは UI 側 (`tag_prewarm_queued` HashSet) で済ませる想定。
    pub(crate) fn push_job(&self, path: PathBuf, cache_key: String) {
        let _ = self.job_tx.send(PrewarmJob { path, cache_key });
    }
}

struct PrewarmJob {
    path: PathBuf,
    cache_key: String,
}

/// 1 ファイル分のプリフェッチ結果。`cache_key` は `adjustment_db::normalize_path(path)` 相当
/// (UI 側 `tags_cache` のキー形式に揃える)。
pub(crate) struct TagPrewarmResult {
    pub cache_key: String,
    pub tags: Vec<String>,
}

/// worker スレッドを起動する。フォルダ切替時に 1 回呼ぶ。
/// ジョブは後から `push_job` で逐次投入する (初期キューは空)。
pub(crate) fn spawn() -> TagPrewarmPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
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
    }
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
                let tags = crate::xmp_reader::read_dc_subject(&job.path);
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if result_tx
                    .send(TagPrewarmResult {
                        cache_key: job.cache_key,
                        tags,
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

    /// 存在しないパスを push しても worker は空タグで結果を返し、エラーにならないこと。
    /// (read_dc_subject は read 失敗で Vec::new() を返す設計)
    #[test]
    fn returns_empty_tags_for_nonexistent_path() {
        let pending = spawn();
        pending.push_job(PathBuf::from("Z:/does/not/exist.jpg"), "key1".to_string());
        let start = Instant::now();
        loop {
            match pending.rx.try_recv() {
                Ok(res) => {
                    assert_eq!(res.cache_key, "key1");
                    assert!(res.tags.is_empty());
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

    /// cancel を立てれば worker は次ループで break し、drop 後に受信は終わる。
    #[test]
    fn cancel_stops_worker_loop() {
        let pending = spawn();
        for i in 0..1000 {
            pending.push_job(
                PathBuf::from(format!("Z:/nope/{i}.jpg")),
                format!("k{i}"),
            );
        }
        pending.cancel();
        drop(pending);
        // パニックせず到達すれば OK (worker は cancel set + rx/job drop で break する)。
    }
}
