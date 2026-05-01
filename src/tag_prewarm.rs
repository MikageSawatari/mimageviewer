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
//! # インクリメンタル投入
//!
//! `spawn()` は空キューで worker を起動し、UI が毎フレーム
//! `App::enqueue_visible_tag_prewarms` から `keep_range` (可視範囲 + prev/next ページ)
//! 分だけを `push_job` でキューに積む。スクロールに追従して必要な分だけ読むため、
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
/// ための送信端。`rx` は worker が XMP 読みの結果を返す経路 (`App::poll_tag_prewarm_results`
/// が drain する)。`in_flight` は push 済みだが UI が drain していないジョブ数。
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
    /// `read_rating = true` のとき、worker は同じ XMP パケットから xmp:Rating も抽出して
    /// `TagPrewarmResult::rating` に載せて返す。
    pub(crate) fn push_job(&self, path: PathBuf, cache_key: String, read_rating: bool) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let _ = self.job_tx.send(PrewarmJob {
            path,
            cache_key,
            read_rating,
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
    cache_key: String,
    read_rating: bool,
}

/// 1 ファイル分のプリフェッチ結果。`cache_key` は `adjustment_db::normalize_path(path)` 相当
/// (UI 側 `tags_cache` のキー形式に揃える)。
/// `rating` は設定 ON かつ XMP に xmp:Rating が存在した場合のみ `Some` になる。
pub(crate) struct TagPrewarmResult {
    pub cache_key: String,
    pub path: PathBuf,
    pub tags: Vec<String>,
    pub rating: Option<u8>,
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

/// ファイルを 1 回だけ read して、XMP パケットから dc:subject と xmp:Rating の
/// 両方を抜き出す。`read_dc_subject` と同じ冒頭のマジックバイト判定を踏襲する。
/// XMP パケット抽出も 1 回で済ませる (各 `read_*_from_bytes` を個別に呼ぶと
/// JPEG APP1 / PNG iTXt 走査が 2 回走り、10 MB JPEG などで CPU が倍になる)。
fn read_xmp_tags_and_rating(path: &std::path::Path) -> (Vec<String>, Option<u8>) {
    let Ok(bytes) = std::fs::read(path) else {
        return (Vec::new(), None);
    };
    if !crate::xmp_reader::has_xmp_capable_magic(&bytes) {
        return (Vec::new(), None);
    }
    let Some(xmp) = crate::xmp_reader::extract_xmp_packet(&bytes) else {
        return (Vec::new(), None);
    };
    let tags = crate::xmp_reader::parse_dc_subject(&xmp);
    let rating = crate::xmp_reader::parse_xmp_rating(&xmp);
    (tags, rating)
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
                // 設定 ON のときはファイルを 1 回だけ open して dc:subject と xmp:Rating を
                // 同じ XMP パケットから抜く。I/O コストは従来のタグ読み 1 回と変わらない。
                let (tags, rating) = if job.read_rating {
                    read_xmp_tags_and_rating(&job.path)
                } else {
                    (crate::xmp_reader::read_dc_subject(&job.path), None)
                };
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if result_tx
                    .send(TagPrewarmResult {
                        cache_key: job.cache_key,
                        path: job.path,
                        tags,
                        rating,
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
        pending.push_job(
            PathBuf::from("Z:/does/not/exist.jpg"),
            "key1".to_string(),
            false,
        );
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

    /// idle 時 (push していない) は `is_busy()` が false を返し、push すると true、
    /// drain 後に再び false に戻ること。`App::update` の repaint ゲート用シグナル。
    #[test]
    fn is_busy_reflects_in_flight() {
        let pending = spawn();
        assert!(!pending.is_busy(), "初期状態は idle");

        pending.push_job(PathBuf::from("Z:/nope/a.jpg"), "ka".to_string(), false);
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
            pending.push_job(
                PathBuf::from(format!("Z:/nope/{i}.jpg")),
                format!("k{i}"),
                false,
            );
        }
        pending.cancel();
        drop(pending);
        // パニックせず到達すれば OK (worker は cancel set + rx/job drop で break する)。
    }
}
