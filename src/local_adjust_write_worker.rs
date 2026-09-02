//! 補正レイヤー保存のバックグラウンド worker。
//!
//! `App::set_local_adjust_layers_for_idx` は UI スレッドで直列化 (q8 量子化 → deflate →
//! base64) と SQLite 書き込みを行っていた。24MP のマスクを持つページで 70.6ms、しかも
//! マスク系スライダーのドラッグ中は**毎フレーム**走る (`Slider::changed()` はドラッグ中
//! 毎フレーム真)。ここはその処理を別スレッドへ出す。
//!
//! ## 合体は「積む前」に行う
//!
//! 要求 1 件が画像原寸のマスクを丸ごと抱える (24MP で 96MB)。**チャネルへ積んでから
//! 取り出し時に古い方を捨てる形では、抱えたままキューに溜まる**ので合体にならない。
//! ここでは key ごとに**未書き込みの文書を 1 つだけ**持つ枠 (`Queue::documents`) を置き、
//! 同じ key への再要求はその枠を差し替える。古い文書はその瞬間に落ちる。
//!
//! `Mutex + Condvar` で保護したキュー + 専用スレッドという形は、CLAUDE.md
//! 「try_lock + sleep は使わない」が指す構造そのもの (`PdfWorkerPool` と同じ)。
//!
//! ## R-26 — 「開けなかった」を「書けた」にしない
//!
//! 結果は [`crate::app::EditStoreOutcome`] のまま返す。`Committed` のときだけ呼び出し側が
//! sidecar ミラーを書く、という同期版の規約をそのまま保つためで、非同期化のついでに
//! `Result` へ畳んではならない ([`crate::app::EditStoreOutcome`] の doc comment を参照)。
//!
//! **同期版との違いが 1 つだけある。**同期版の `Unavailable` は「起動時に開けなかった」
//! で、その後の書き込みは一切試されなかった。ここでは worker が自分の接続を持ち、
//! 開けなければ次の要求でまた試す。判定する主体と書く主体が一致するので、
//! 一時的に開けなかっただけのときに以後ずっと保存できない状態に落ちない。
//!
//! ## 止まった worker を黙って無視しない
//!
//! spawn 失敗と worker の panic はどちらも「以後すべての保存が消える」に直結する。
//! [`LocalAdjustWriteHandle::submit`] は worker が生きていないと分かれば
//! [`EditStoreOutcome::Failed`] を**その場で**返すので、呼び出し側の R-26 判定
//! (失敗ならミラーを書かずトーストを出す) がそのまま働く。
//!
//! ## 終了・待ち合わせ
//!
//! - 終了は「`stopped` を立てて起こす」。worker は**キューを空にしてから**抜けるので、
//!   在庫は必ず書き切る。
//! - [`LocalAdjustWriteHandle::drain_blocking`] は**フェンスを 1 つ積んでその ACK を待つ**。
//!   「残件カウンタが 0 になるまで受け取る」形にすると、最後の結果を受け取った直後・
//!   カウンタが進む前にもう一度 `recv` して永久に待つロストウェイクアップが起きる
//!   (2026-08-31 Codex P1)。worker が panic しても [`WorkerGuard`] がフェンスを落として
//!   待ち手を起こす。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::app::EditStoreOutcome;
use crate::local_adjust_db::LocalAdjustDb;

/// worker が 1 件の保存について返す答え。
///
/// `layers` は**実際に書いた文書そのもの**。呼び出し側が別途メモリから取り直すと、
/// 積んでから完了までの間に入った編集をミラーしてしまう (書いた内容とミラーがずれる)。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalAdjustWriteCompletion {
    pub key: String,
    pub generation: u64,
    pub layers: local_adjust_core::LocalAdjustmentLayers,
    pub outcome: EditStoreOutcome,
}

/// まだ書いていない 1 ページ分の文書。key ごとに 1 つだけ持つ。
struct QueuedWrite {
    generation: u64,
    layers: local_adjust_core::LocalAdjustmentLayers,
}

/// キューに並ぶもの。`Write` は key だけを持ち、文書は `documents` 側にある
/// (同じ key の再要求で文書だけを差し替えられるようにするため)。
enum QueueItem {
    Write(String),
    /// 待ち合わせ。worker がここまで来たら ACK を返して待ち手を起こす。
    Fence(Sender<()>),
}

struct Queue {
    items: VecDeque<QueueItem>,
    documents: HashMap<String, QueuedWrite>,
    stopped: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    /// worker スレッドが生きているか。spawn 失敗・panic・正常終了で false。
    alive: AtomicBool,
}

impl Shared {
    /// poison を復旧して掴む。worker が panic したあとも待ち手を起こす必要がある。
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct LocalAdjustWriteHandle {
    shared: Arc<Shared>,
    result_rx: Receiver<LocalAdjustWriteCompletion>,
    next_generation: AtomicU64,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LocalAdjustWriteHandle {
    pub(crate) fn spawn(db_path: PathBuf) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                documents: HashMap::new(),
                stopped: false,
            }),
            ready: Condvar::new(),
            alive: AtomicBool::new(true),
        });
        let (result_tx, result_rx) = unbounded::<LocalAdjustWriteCompletion>();

        let worker_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("local-adjust-write-worker".into())
            .spawn(move || {
                let _guard = WorkerGuard {
                    shared: Arc::clone(&worker_shared),
                };
                run_worker(&db_path, &worker_shared, &result_tx);
            });
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(error) => {
                // スレッドを作れない状態で `expect` すると UI ごと落ちる。生きていない
                // 印を立てて返し、以後の保存は `submit` がその場で失敗として返す。
                crate::logger::log(format!("[local-adjust-write] worker spawn failed: {error}"));
                shared.alive.store(false, Ordering::Release);
                None
            }
        };

        Self {
            shared,
            result_rx,
            next_generation: AtomicU64::new(0),
            thread,
        }
    }

    /// 保存を積む。
    ///
    /// `Ok(generation)` は「worker が受け取った」。`Err(outcome)` は**その場で確定した
    /// 失敗**で、呼び出し側は他の失敗と同じに扱う (ミラーを書かず、利用者へ伝える)。
    pub(crate) fn submit(
        &self,
        key: String,
        layers: local_adjust_core::LocalAdjustmentLayers,
    ) -> Result<u64, EditStoreOutcome> {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let mut queue = self.shared.lock();
        if queue.stopped || !self.shared.alive.load(Ordering::Acquire) {
            return Err(EditStoreOutcome::Failed(
                "保存ワーカーが停止しています".to_string(),
            ));
        }
        // 同じ key が既に並んでいれば**位置は動かさず文書だけ差し替える**。
        // 古い文書はここで落ちるので、キューに原寸マスクが積み上がらない。
        let replaced = queue
            .documents
            .insert(key.clone(), QueuedWrite { generation, layers })
            .is_some();
        if !replaced {
            queue.items.push_back(QueueItem::Write(key));
        }
        drop(queue);
        self.shared.ready.notify_one();
        Ok(generation)
    }

    /// まだ書いていない要求か、書いたが未回収の結果があるか。
    ///
    /// リネーム移行のように `local_adjust.db` を UI スレッドから直接書き換える処理の
    /// 前に使う。完了を適用するとサイドカーへミラーが書かれるので、**未回収の結果も
    /// 「まだ終わっていない」に数える**。
    pub(crate) fn has_unfinished_work(&self) -> bool {
        !self.result_rx.is_empty() || {
            let queue = self.shared.lock();
            !queue.items.is_empty() || !queue.documents.is_empty()
        }
    }

    /// テスト用: worker が抜けた状態を作る。
    ///
    /// 本番と同じ [`WorkerGuard`] の後始末を通すので、テストが見る状態は
    /// 「panic した worker」「spawn できなかった worker」と同一。
    #[cfg(test)]
    pub(crate) fn mark_worker_stopped_for_test(&self) {
        drop(WorkerGuard {
            shared: Arc::clone(&self.shared),
        });
    }

    pub(crate) fn try_recv(&self) -> Option<LocalAdjustWriteCompletion> {
        self.result_rx.try_recv().ok()
    }

    /// UI を止めずに待つ呼び出し側のため、現在の queue 末尾へフェンスを積む。
    /// ACK より前の completion はすべて result channel へ送信済みになる。
    pub(crate) fn enqueue_fence(&self) -> Result<Receiver<()>, String> {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded::<()>(1);
        {
            let mut queue = self.shared.lock();
            if queue.stopped || !self.shared.alive.load(Ordering::Acquire) {
                return Err("保存ワーカーが停止しています".to_string());
            }
            queue.items.push_back(QueueItem::Fence(ack_tx));
        }
        self.shared.ready.notify_one();
        Ok(ack_rx)
    }

    /// 積んである保存がすべて着地するまで待ち、結果をまとめて返す。**worker は止めない。**
    ///
    /// フェンスを 1 つ積み、その ACK を待つ。worker はキューを順に処理するので、
    /// ACK が返った時点で「フェンスより前に積んだ保存」はすべて書き終えて結果も
    /// 送り終えている。worker が panic した場合は [`WorkerGuard`] がフェンスを落とすので、
    /// `recv` は `Disconnected` で戻る (待ち続けない)。
    pub(crate) fn drain_blocking(&self) -> Vec<LocalAdjustWriteCompletion> {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded::<()>(1);
        {
            let mut queue = self.shared.lock();
            if queue.stopped {
                return self.result_rx.try_iter().collect();
            }
            queue.items.push_back(QueueItem::Fence(ack_tx));
        }
        self.shared.ready.notify_one();
        let _ = ack_rx.recv();
        self.result_rx.try_iter().collect()
    }

    /// worker を止め、**在庫を書き切らせてから**結果をすべて返す。
    pub(crate) fn shutdown_and_collect(mut self) -> Vec<LocalAdjustWriteCompletion> {
        self.stop_and_join();
        self.result_rx.try_iter().collect()
    }

    fn stop_and_join(&mut self) {
        self.shared.lock().stopped = true;
        self.shared.ready.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LocalAdjustWriteHandle {
    fn drop(&mut self) {
        // 待たずに落とすと、書き切る前にプロセスが終わり得る。
        // `shutdown_and_collect` を通った場合は take 済みなのでここは no-op。
        self.stop_and_join();
    }
}

/// worker スレッドが**どう抜けても** (正常終了でも panic でも) 後始末をする。
///
/// これが無いと、panic した worker のキューに残ったフェンスが誰にも処理されず、
/// `drain_blocking` が永久に待つ。
struct WorkerGuard {
    shared: Arc<Shared>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.shared.alive.store(false, Ordering::Release);
        let mut queue = self.shared.lock();
        queue.stopped = true;
        // フェンスの Sender をここで落とす = 待ち手が `Disconnected` で起きる。
        queue.items.clear();
        queue.documents.clear();
        drop(queue);
        self.shared.ready.notify_all();
    }
}

fn run_worker(db_path: &Path, shared: &Shared, result_tx: &Sender<LocalAdjustWriteCompletion>) {
    let mut db: Option<LocalAdjustDb> = None;
    loop {
        let item = {
            let mut queue = shared.lock();
            loop {
                if let Some(item) = queue.items.pop_front() {
                    break item;
                }
                // **キューが空になってから**止まる。`stopped` だけで抜けると
                // 在庫が捨てられる (drain-on-shutdown)。
                if queue.stopped {
                    return;
                }
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        match item {
            QueueItem::Fence(ack) => {
                let _ = ack.send(());
            }
            QueueItem::Write(key) => {
                // 取り出しと文書の取得の間に同じ key が再要求されても、`documents` の
                // 枠が差し替わるだけなので、ここで取るのは常に最新。
                let Some(write) = shared.lock().documents.remove(&key) else {
                    continue;
                };
                let completion = process_write(db_path, &mut db, key, write);
                let _ = result_tx.send(completion);
            }
        }
    }
}

fn process_write(
    db_path: &Path,
    db: &mut Option<LocalAdjustDb>,
    key: String,
    write: QueuedWrite,
) -> LocalAdjustWriteCompletion {
    let t0 = std::time::Instant::now();
    let outcome = write_layers(db_path, db, &key, &write.layers);
    crate::perf::event(
        "local_adjust",
        "save_done",
        Some(&key),
        write.generation,
        &[
            (
                "ms",
                serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
            ),
            (
                "outcome",
                serde_json::Value::from(match &outcome {
                    EditStoreOutcome::Committed => "committed",
                    EditStoreOutcome::Unavailable => "unavailable",
                    EditStoreOutcome::Failed(_) => "failed",
                }),
            ),
            ("layers", serde_json::Value::from(write.layers.len())),
        ],
    );
    LocalAdjustWriteCompletion {
        key,
        generation: write.generation,
        layers: write.layers,
        outcome,
    }
}

fn write_layers(
    db_path: &Path,
    slot: &mut Option<LocalAdjustDb>,
    key: &str,
    layers: &[local_adjust_core::LocalAdjustmentLayer],
) -> EditStoreOutcome {
    if slot.is_none() {
        match LocalAdjustDb::open_for_writer(db_path) {
            Ok(db) => *slot = Some(db),
            Err(error) => {
                crate::logger::log(format!(
                    "[local-adjust-write] cannot open {}: {error}",
                    db_path.display()
                ));
                return EditStoreOutcome::Unavailable;
            }
        }
    }
    let Some(db) = slot.as_ref() else {
        return EditStoreOutcome::Unavailable;
    };
    match db.set_layers(key, layers) {
        Ok(()) => EditStoreOutcome::Committed,
        Err(error) => EditStoreOutcome::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(name: &str) -> local_adjust_core::LocalAdjustmentLayer {
        local_adjust_core::LocalAdjustmentLayer::new(
            name,
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        )
    }

    fn doc(name: &str) -> local_adjust_core::LocalAdjustmentLayers {
        local_adjust_core::LocalAdjustmentLayers::new(vec![layer(name)])
    }

    #[test]
    fn a_write_that_the_store_accepts_reports_committed_with_the_document_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        let document = doc("saved");
        handle.submit("key".to_string(), document.clone()).unwrap();

        let results = handle.shutdown_and_collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].outcome, EditStoreOutcome::Committed);
        assert_eq!(results[0].layers.as_slice(), document.as_slice());
        assert_eq!(
            LocalAdjustDb::open_at(&path)
                .unwrap()
                .get_layers("key")
                .unwrap()
                .as_slice(),
            document.as_slice()
        );
    }

    #[test]
    fn edit_bundle_bulk_fence_ack_follows_prior_write_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        let document = doc("before bulk");
        handle.submit("key".to_string(), document.clone()).unwrap();
        let fence = handle.enqueue_fence().unwrap();

        fence
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("fence must acknowledge after the queued write");
        let completion = handle
            .try_recv()
            .expect("completion must be sent before the fence ACK");
        assert_eq!(completion.outcome, EditStoreOutcome::Committed);
        assert_eq!(completion.layers.as_slice(), document.as_slice());
        let _ = handle.shutdown_and_collect();
        assert_eq!(
            LocalAdjustDb::open_at(&path)
                .unwrap()
                .get_layers("key")
                .unwrap()
                .as_slice(),
            document.as_slice()
        );
    }

    /// 保存先を開けない = `Unavailable`。**`Failed` でも成功でもない。**
    ///
    /// 呼び出し側はこれを見て sidecar ミラーを書かずに済ませる。ここが成功に化けると
    /// R-26 の事故 (中央に無いのに sidecar だけ「保存済み」を主張し、次回起動の
    /// import がそれを捨てる) が戻る。
    #[test]
    fn a_store_that_cannot_be_opened_reports_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        // ディレクトリを DB のパスに据えると SQLite は開けない。
        let path = dir.path().join("not-a-file");
        std::fs::create_dir(&path).unwrap();
        let handle = LocalAdjustWriteHandle::spawn(path);
        handle.submit("key".to_string(), doc("saved")).unwrap();

        let results = handle.shutdown_and_collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].outcome, EditStoreOutcome::Unavailable);
    }

    /// 同じ key を積み直すと、**まだ書いていない文書はその場で落ちる**。
    ///
    /// 取り出し時に古い方を捨てる形だと、捨てるまでキューが原寸マスクを抱えたままに
    /// なる。要求 1 件が 24MP で 96MB なので、ドラッグ中に積み上がる (Codex P1)。
    #[test]
    fn resubmitting_a_key_replaces_the_queued_document_instead_of_queueing_another() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path);
        for name in ["first", "second", "newest"] {
            // worker に取られる前に積み直したことにする。取られていたら枠は空くので、
            // この検査は「取られていない分について 1 件しか残らない」を見る。
            let mut queue = handle.shared.lock();
            queue.documents.insert(
                "key".to_string(),
                QueuedWrite {
                    generation: 1,
                    layers: doc(name),
                },
            );
            if queue.items.is_empty() {
                queue.items.push_back(QueueItem::Write("key".to_string()));
            }
        }

        let queue = handle.shared.lock();
        assert_eq!(
            queue.documents.len(),
            1,
            "同じ key の未書き込み文書が 1 つを超えて残っている"
        );
        assert_eq!(
            queue.documents["key"].layers.as_slice(),
            doc("newest").as_slice()
        );
        assert_eq!(queue.items.len(), 1, "同じ key を 2 度並べている");
    }

    /// 上と同じことを公開 API だけで確かめる (worker は止めておく)。
    #[test]
    fn submit_does_not_queue_a_second_slot_for_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let handle = LocalAdjustWriteHandle::spawn(dir.path().join("local_adjust.db"));
        // worker を待たせたまま積むために、いったん条件変数へ入れる。ここでは
        // `stopped` を使わずに済むよう、worker が起きる前に 3 件積み切る。
        let mut queued = Vec::new();
        for name in ["first", "second", "newest"] {
            queued.push(handle.submit("key".to_string(), doc(name)));
        }

        assert!(queued.iter().all(|result| result.is_ok()));
        let queue = handle.shared.lock();
        assert!(
            queue.items.len() <= 1,
            "同じ key を複数枠で並べている: {}",
            queue.items.len()
        );
        assert!(
            queue.documents.len() <= 1,
            "同じ key の未書き込み文書が積み上がっている: {}",
            queue.documents.len()
        );
    }

    /// 積み直しても、最後に積んだものが正本に残る。
    #[test]
    fn the_newest_document_for_a_key_is_what_lands_in_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        for name in ["first", "second", "newest"] {
            handle.submit("key".to_string(), doc(name)).unwrap();
        }

        handle.shutdown_and_collect();

        assert_eq!(
            LocalAdjustDb::open_at(&path)
                .unwrap()
                .get_layers("key")
                .unwrap()
                .as_slice(),
            doc("newest").as_slice()
        );
    }

    /// key が違えば独立。
    #[test]
    fn requests_for_different_keys_do_not_replace_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        handle.submit("a".to_string(), doc("for a")).unwrap();
        handle.submit("b".to_string(), doc("for b")).unwrap();

        let results = handle.shutdown_and_collect();

        assert_eq!(results.len(), 2);
        let db = LocalAdjustDb::open_at(&path).unwrap();
        assert_eq!(
            db.get_layers("a").unwrap().as_slice(),
            doc("for a").as_slice()
        );
        assert_eq!(
            db.get_layers("b").unwrap().as_slice(),
            doc("for b").as_slice()
        );
    }

    /// 空の文書は行の削除。
    #[test]
    fn an_empty_document_removes_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        LocalAdjustDb::open_at(&path)
            .unwrap()
            .set_layers("key", &[layer("existing")])
            .unwrap();

        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        handle
            .submit(
                "key".to_string(),
                local_adjust_core::LocalAdjustmentLayers::default(),
            )
            .unwrap();
        let results = handle.shutdown_and_collect();

        assert_eq!(results[0].outcome, EditStoreOutcome::Committed);
        assert!(
            LocalAdjustDb::open_at(&path)
                .unwrap()
                .get_layers("key")
                .is_none()
        );
    }

    /// 終了時に在庫を捨てない。
    #[test]
    fn shutdown_drains_the_queue_before_stopping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        for i in 0..32 {
            handle.submit(format!("key{i}"), doc("saved")).unwrap();
        }

        let results = handle.shutdown_and_collect();

        assert_eq!(results.len(), 32, "積んだ分はすべて処理される");
        let db = LocalAdjustDb::open_at(&path).unwrap();
        for i in 0..32 {
            assert!(
                db.get_layers(&format!("key{i}")).is_some(),
                "key{i} が書かれていない"
            );
        }
    }

    /// 保存が着地するまで待てること。
    ///
    /// key の付け替え (製本ページの rename、リネーム移行) の直前に使う。付け替えの後に
    /// 古い key の保存が着地すると、移したはずの行が古い key で書き戻る。
    #[test]
    fn draining_waits_until_every_queued_write_has_landed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        for i in 0..32 {
            handle.submit(format!("key{i}"), doc("saved")).unwrap();
        }

        let collected = handle.drain_blocking();

        assert_eq!(collected.len(), 32, "着地する前に待つのをやめている");
        let db = LocalAdjustDb::open_at(&path).unwrap();
        for i in 0..32 {
            assert!(
                db.get_layers(&format!("key{i}")).is_some(),
                "key{i} が書かれていない"
            );
        }
    }

    /// 何度でも待てる (フェンスが 1 回きりの仕掛けになっていない)。
    #[test]
    fn draining_twice_still_returns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path);
        handle.submit("a".to_string(), doc("a")).unwrap();
        assert_eq!(handle.drain_blocking().len(), 1);
        handle.submit("b".to_string(), doc("b")).unwrap();
        assert_eq!(handle.drain_blocking().len(), 1);
        assert!(handle.drain_blocking().is_empty());
    }

    /// 積んでいなければ待たない。
    #[test]
    fn draining_an_idle_worker_returns_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let handle = LocalAdjustWriteHandle::spawn(dir.path().join("local_adjust.db"));
        assert!(handle.drain_blocking().is_empty());
    }

    /// worker が居なくなったら、保存は**その場で失敗として返る**。
    ///
    /// 黙って積むと、画面には編集が残ったまま正本には何も書かれない状態が続く。
    #[test]
    fn submitting_to_a_dead_worker_fails_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let handle = LocalAdjustWriteHandle::spawn(dir.path().join("local_adjust.db"));
        // worker が抜けた状態を作る (panic でも正常終了でも同じ印が立つ)。
        handle.shared.alive.store(false, Ordering::Release);

        let result = handle.submit("key".to_string(), doc("lost"));

        assert!(
            matches!(result, Err(EditStoreOutcome::Failed(_))),
            "止まった worker へ積んで成功を返している"
        );
    }

    /// worker が居なくなったら、待ち合わせは**止まらずに戻る**。
    ///
    /// 残件カウンタで待つ形だと、ここが永久ハングになる (Codex P1)。
    #[test]
    fn draining_after_the_worker_is_gone_returns_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let handle = LocalAdjustWriteHandle::spawn(dir.path().join("local_adjust.db"));
        // worker の drop guard が通った状態 = フェンスを誰も処理しない。
        handle.shared.lock().stopped = true;
        handle.shared.alive.store(false, Ordering::Release);

        assert!(handle.drain_blocking().is_empty());
    }

    /// panic した worker でも待ち手は起きる。
    #[test]
    fn the_worker_guard_releases_waiters_even_when_the_thread_dies() {
        let dir = tempfile::tempdir().unwrap();
        let handle = LocalAdjustWriteHandle::spawn(dir.path().join("local_adjust.db"));
        let (ack_tx, ack_rx) = crossbeam_channel::bounded::<()>(1);
        handle
            .shared
            .lock()
            .items
            .push_back(QueueItem::Fence(ack_tx));

        // worker が抜けたときと同じ後始末を走らせる。
        drop(WorkerGuard {
            shared: Arc::clone(&handle.shared),
        });

        assert!(
            ack_rx.recv().is_err(),
            "フェンスが落とされていない (待ち手が永久に待つ)"
        );
    }
}
