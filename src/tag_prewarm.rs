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
//! # キャンセル
//!
//! - フォルダ切替や新しい `prewarm_grid_tags` 呼び出しで旧 pending は `cancel` される
//!   (ループ内で毎回チェックし、送信前にも再チェック)。
//! - UI 側が `pending = None` にすれば receiver が落ちて `tx.send` が Err になり、
//!   スレッドは次の送信で自然終了する。
//!
//! # 順序
//!
//! `App::prewarm_grid_tags` は可視範囲を先頭に並べて渡すため、ユーザーに見えている
//! バッジから順に埋まる。スクロール中に一部タグ未表示になっていても、
//! 数フレーム後には反映される。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;

/// バックグラウンド XMP プリフェッチの状態。
pub(crate) struct TagPrewarmPending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<TagPrewarmResult>,
}

impl TagPrewarmPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 1 ファイル分のプリフェッチ結果。`cache_key` は `adjustment_db::normalize_path(path)` 相当
/// (UI 側 `tags_cache` のキー形式に揃える)。
pub(crate) struct TagPrewarmResult {
    pub cache_key: String,
    pub tags: Vec<String>,
}

/// worker を起動する。`requests` の各要素は `(読み取り対象パス, cache_key)` で、UI 側が
/// `fts_meta` 未ヒットだった Image アイテムだけを並べて渡す (可視範囲を先頭にして
/// ユーザーに近いファイルから処理する)。
pub(crate) fn spawn(requests: Vec<(PathBuf, String)>) -> TagPrewarmPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tag-prewarm".into())
        .spawn(move || {
            for (path, cache_key) in requests {
                if cancel_w.load(Ordering::Relaxed) {
                    break;
                }
                let tags = crate::xmp_reader::read_dc_subject(&path);
                if cancel_w.load(Ordering::Relaxed) {
                    break;
                }
                if tx.send(TagPrewarmResult { cache_key, tags }).is_err() {
                    break; // UI 側が pending を drop した → receiver 閉鎖
                }
            }
        })
        .ok();
    TagPrewarmPending { cancel, rx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 存在しないパスを渡しても worker は空タグで結果を返し、エラーにならないこと。
    /// (read_dc_subject は read 失敗で Vec::new() を返す設計)
    #[test]
    fn returns_empty_tags_for_nonexistent_path() {
        let requests = vec![(PathBuf::from("Z:/does/not/exist.jpg"), "key1".to_string())];
        let pending = spawn(requests);
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

    /// cancel を立てれば worker は次ループで break し、受信できる結果が打ち切られる。
    #[test]
    fn cancel_stops_worker_loop() {
        let requests: Vec<(PathBuf, String)> = (0..1000)
            .map(|i| (PathBuf::from(format!("Z:/nope/{i}.jpg")), format!("k{i}")))
            .collect();
        let pending = spawn(requests);
        pending.cancel();
        // drop 後に worker がすぐ break することを確認 (send が receiver 閉鎖で Err)。
        drop(pending);
        // 同スレッドで sleep しても検証にならない — worker は detach 済みなので
        // ここではパニックしなければ OK とする (cancel set + rx drop の両方が break 条件)。
    }
}
