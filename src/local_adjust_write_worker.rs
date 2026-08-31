//! 補正レイヤー保存のバックグラウンド worker。
//!
//! `App::set_local_adjust_layers_for_idx` は UI スレッドで直列化 (q8 量子化 → deflate →
//! base64) と SQLite 書き込みを行っていた。24MP のマスクを持つページで 70.6ms、しかも
//! マスク系スライダーのドラッグ中は**毎フレーム**走る (`Slider::changed()` はドラッグ中
//! 毎フレーム真)。ここはその処理を別スレッドへ出す。
//!
//! ## 合体 (generation coalescing)
//!
//! 要求 1 件が画像原寸のマスクを丸ごと抱えるので、ドラッグ中に積まれた要求をすべて
//! 書くとキューにメモリが積み上がる。同じ key について**最新の generation だけ**を書き、
//! 追い越されたものは [`LocalAdjustWriteCompletion::Superseded`] として捨てる。
//! 後続の要求が同じ行を上書きするので、捨てても正本の内容は変わらない。
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
//! ## 終了時
//!
//! drain-on-shutdown。**送信端を落とすことが停止の合図**で、`Receiver::recv` は
//! キューが空になって初めて `Disconnected` を返すので、在庫は必ず書き切ってから止まる。
//! 停止フラグを別に持って周期的に見る形 ([`crate::rating_write_worker`]) と違い、
//! 待ち時間もポーリングも要らない。`shutdown_and_collect` は join してから結果をすべて
//! 返すので、呼び出し側は取りこぼしなく sidecar ミラーを書ける。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::app::EditStoreOutcome;
use crate::local_adjust_db::LocalAdjustDb;

/// 1 ページ分の保存要求。
pub(crate) struct LocalAdjustWriteJob {
    pub key: String,
    pub generation: u64,
    pub layers: local_adjust_core::LocalAdjustmentLayers,
}

/// worker が 1 件の要求について返す答え。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LocalAdjustWriteCompletion {
    /// 正本へ書きに行った。`outcome` が `Committed` のときだけ sidecar ミラーを書ける。
    ///
    /// `layers` は**実際に書いた文書そのもの**。呼び出し側が別途メモリから取り直すと、
    /// その間に入った編集をミラーしてしまう (書いた内容とミラーがずれる)。
    Settled {
        key: String,
        generation: u64,
        layers: local_adjust_core::LocalAdjustmentLayers,
        outcome: EditStoreOutcome,
    },
    /// 同じ key の新しい要求に追い越されたので書いていない。
    ///
    /// **成功でも失敗でもない。**ミラーもトーストも出さないこと。
    Superseded { key: String, generation: u64 },
}

impl LocalAdjustWriteCompletion {
    pub(crate) fn key(&self) -> &str {
        match self {
            Self::Settled { key, .. } | Self::Superseded { key, .. } => key,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        match self {
            Self::Settled { generation, .. } | Self::Superseded { generation, .. } => *generation,
        }
    }
}

pub(crate) struct LocalAdjustWriteHandle {
    /// `None` は「停止を伝えた後」。落とすことが worker への停止の合図なので、
    /// join より**前**に落とすこと。
    job_tx: Option<Sender<LocalAdjustWriteJob>>,
    result_rx: Receiver<LocalAdjustWriteCompletion>,
    /// key ごとに最後に submit した generation。worker が追い越し判定に読む。
    latest: Arc<Mutex<HashMap<String, u64>>>,
    next_generation: Arc<AtomicU64>,
    submitted: Arc<AtomicUsize>,
    done: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LocalAdjustWriteHandle {
    pub(crate) fn spawn(db_path: PathBuf) -> Self {
        let (job_tx, job_rx) = unbounded::<LocalAdjustWriteJob>();
        let (result_tx, result_rx) = unbounded::<LocalAdjustWriteCompletion>();
        let latest: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        let done = Arc::new(AtomicUsize::new(0));

        let w_latest = Arc::clone(&latest);
        let w_done = Arc::clone(&done);
        let thread = std::thread::Builder::new()
            .name("local-adjust-write-worker".into())
            .spawn(move || {
                run_worker(&db_path, &job_rx, &result_tx, &w_latest, &w_done);
            })
            .expect("local-adjust-write-worker spawn");

        Self {
            job_tx: Some(job_tx),
            result_rx,
            latest,
            next_generation: Arc::new(AtomicU64::new(0)),
            submitted: Arc::new(AtomicUsize::new(0)),
            done,
            thread: Some(thread),
        }
    }

    /// 保存を積む。返り値はこの要求の generation で、完了を照合するのに使う。
    pub(crate) fn submit(
        &self,
        key: String,
        layers: local_adjust_core::LocalAdjustmentLayers,
    ) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut latest) = self.latest.lock() {
            latest.insert(key.clone(), generation);
        }
        self.submitted.fetch_add(1, Ordering::Relaxed);
        if let Some(job_tx) = self.job_tx.as_ref() {
            let _ = job_tx.send(LocalAdjustWriteJob {
                key,
                generation,
                layers,
            });
        }
        generation
    }

    /// 実行待ち / 実行中の要求が残っているか。
    fn is_busy(&self) -> bool {
        self.submitted.load(Ordering::Relaxed) != self.done.load(Ordering::Relaxed)
    }

    /// 積んである保存がすべて着地するまで待ち、結果をまとめて返す。**worker は止めない。**
    ///
    /// 時間で待たずに「残り件数が 0 になるまで受け取る」形にしてある。worker は結果を
    /// 送ってから件数を数えるので、`is_busy` が偽になった時点で結果はすべてキューに
    /// 入っている。worker が死んで送信端が落ちた場合は `recv` が `Disconnected` を
    /// 返すので、待ち続けることはない。
    pub(crate) fn drain_blocking(&self) -> Vec<LocalAdjustWriteCompletion> {
        let mut collected = Vec::new();
        while self.is_busy() {
            match self.result_rx.recv() {
                Ok(completion) => collected.push(completion),
                Err(_) => break,
            }
        }
        collected.extend(self.result_rx.try_iter());
        collected
    }

    pub(crate) fn try_recv(&self) -> Option<LocalAdjustWriteCompletion> {
        self.result_rx.try_recv().ok()
    }

    /// worker を止め、**在庫を書き切らせてから**結果をすべて返す。
    ///
    /// 終了経路用。時間で諦める形にすると「間に合わなかった保存」が環境ごとに変わるので、
    /// join してから受け取り切る形にしてある。
    pub(crate) fn shutdown_and_collect(mut self) -> Vec<LocalAdjustWriteCompletion> {
        self.stop_and_join();
        self.result_rx.try_iter().collect()
    }

    /// 送信端を落として worker に停止を伝え、書き切るまで待つ。
    fn stop_and_join(&mut self) {
        // **join より先に落とすこと。**送信端が生きている限り `recv` は待ち続けるので、
        // 順序を逆にすると join が返らない。
        self.job_tx = None;
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

fn run_worker(
    db_path: &Path,
    job_rx: &Receiver<LocalAdjustWriteJob>,
    result_tx: &Sender<LocalAdjustWriteCompletion>,
    latest: &Mutex<HashMap<String, u64>>,
    done: &AtomicUsize,
) {
    let mut db: Option<LocalAdjustDb> = None;
    // `recv` はキューが空になって初めて `Disconnected` を返す。送信端が落ちても
    // 在庫は必ず先に返ってくるので、これがそのまま drain-on-shutdown になる。
    while let Ok(job) = job_rx.recv() {
        let completion = process_job(db_path, &mut db, latest, job);
        // **送ってから数える。**逆にすると「busy ではない」のに結果がまだキューに
        // 入っていない窓ができ、`drain_blocking` が取りこぼす。
        let _ = result_tx.send(completion);
        done.fetch_add(1, Ordering::Relaxed);
    }
}

fn process_job(
    db_path: &Path,
    db: &mut Option<LocalAdjustDb>,
    latest: &Mutex<HashMap<String, u64>>,
    job: LocalAdjustWriteJob,
) -> LocalAdjustWriteCompletion {
    let superseded = latest
        .lock()
        .ok()
        .and_then(|latest| latest.get(&job.key).copied())
        .is_some_and(|newest| newest > job.generation);
    if superseded {
        crate::perf::event(
            "local_adjust",
            "save_superseded",
            Some(&job.key),
            job.generation,
            &[],
        );
        return LocalAdjustWriteCompletion::Superseded {
            key: job.key,
            generation: job.generation,
        };
    }

    let t0 = std::time::Instant::now();
    let outcome = write_layers(db_path, db, &job.key, &job.layers);
    crate::perf::event(
        "local_adjust",
        "save_done",
        Some(&job.key),
        job.generation,
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
            ("layers", serde_json::Value::from(job.layers.len())),
        ],
    );
    LocalAdjustWriteCompletion::Settled {
        key: job.key,
        generation: job.generation,
        layers: job.layers,
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
        match LocalAdjustDb::open_at(db_path) {
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

    /// 結果を 1 件ずつ受け取る。worker は別スレッドなので、テストは
    /// `shutdown_and_collect` で join してから読む (時間で待たない)。
    fn run_to_completion(handle: LocalAdjustWriteHandle) -> Vec<LocalAdjustWriteCompletion> {
        handle.shutdown_and_collect()
    }

    #[test]
    fn a_write_that_the_store_accepts_reports_committed_with_the_document_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        let document = doc("saved");
        handle.submit("key".to_string(), document.clone());

        let results = run_to_completion(handle);

        assert_eq!(results.len(), 1);
        let LocalAdjustWriteCompletion::Settled {
            outcome, layers, ..
        } = &results[0]
        else {
            panic!("expected Settled, got {:?}", results[0]);
        };
        assert_eq!(*outcome, EditStoreOutcome::Committed);
        assert_eq!(layers.as_slice(), document.as_slice());
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
        handle.submit("key".to_string(), doc("saved"));

        let results = run_to_completion(handle);

        assert_eq!(results.len(), 1);
        let LocalAdjustWriteCompletion::Settled { outcome, .. } = &results[0] else {
            panic!("expected Settled, got {:?}", results[0]);
        };
        assert_eq!(*outcome, EditStoreOutcome::Unavailable);
    }

    /// 同じ key の古い要求は書かずに捨てる。
    ///
    /// 要求 1 件が画像原寸のマスクを抱えるので、ドラッグ中に積まれた分をすべて書くと
    /// キューにメモリが積み上がる。最後の 1 件だけが正本に残ればよい。
    #[test]
    fn only_the_newest_request_for_a_key_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        // worker が 1 件目を取り出す前に 3 件積む。取り出せてしまった場合でも
        // 「最後の 1 件が正本に残る」ことは変わらない。
        for name in ["first", "second", "newest"] {
            handle.submit("key".to_string(), doc(name));
        }

        let results = run_to_completion(handle);

        assert_eq!(results.len(), 3);
        let newest = results.iter().map(|r| r.generation()).max().unwrap();
        for result in &results {
            if result.generation() < newest {
                continue;
            }
            assert!(
                matches!(
                    result,
                    LocalAdjustWriteCompletion::Settled {
                        outcome: EditStoreOutcome::Committed,
                        ..
                    }
                ),
                "最新の要求は書かれる: {result:?}"
            );
        }
        assert_eq!(
            LocalAdjustDb::open_at(&path)
                .unwrap()
                .get_layers("key")
                .unwrap()
                .as_slice(),
            doc("newest").as_slice()
        );
    }

    /// 追い越し判定そのもの。
    ///
    /// 上のテストは「最新が正本に残る」ことを見るが、worker が速ければ 3 件とも
    /// 書いてしまっても通る。追い越しを**捨てている**ことはここで直接確かめる。
    #[test]
    fn a_request_that_a_newer_one_overtook_is_not_written_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let latest = Mutex::new(HashMap::from([("key".to_string(), 7)]));
        let mut db = None;

        let completion = process_job(
            &path,
            &mut db,
            &latest,
            LocalAdjustWriteJob {
                key: "key".to_string(),
                generation: 6,
                layers: doc("stale"),
            },
        );

        assert_eq!(
            completion,
            LocalAdjustWriteCompletion::Superseded {
                key: "key".to_string(),
                generation: 6,
            }
        );
        assert!(db.is_none(), "書きに行っていないので接続も開かない");
        assert!(
            !path.exists(),
            "追い越された要求は正本に触れない: {}",
            path.display()
        );
    }

    /// 同じ generation は追い越しではない (自分自身に負けない)。
    #[test]
    fn a_request_is_written_when_it_is_the_newest_for_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let latest = Mutex::new(HashMap::from([("key".to_string(), 6)]));
        let mut db = None;

        let completion = process_job(
            &path,
            &mut db,
            &latest,
            LocalAdjustWriteJob {
                key: "key".to_string(),
                generation: 6,
                layers: doc("newest"),
            },
        );

        assert!(matches!(
            completion,
            LocalAdjustWriteCompletion::Settled {
                outcome: EditStoreOutcome::Committed,
                ..
            }
        ));
    }

    /// key が違えば追い越しではない。
    #[test]
    fn requests_for_different_keys_do_not_supersede_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        handle.submit("a".to_string(), doc("for a"));
        handle.submit("b".to_string(), doc("for b"));

        let results = run_to_completion(handle);

        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|r| matches!(r, LocalAdjustWriteCompletion::Settled { .. })),
            "別 key は追い越さない: {results:?}"
        );
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
        handle.submit(
            "key".to_string(),
            local_adjust_core::LocalAdjustmentLayers::default(),
        );
        let results = run_to_completion(handle);

        assert!(matches!(
            results.as_slice(),
            [LocalAdjustWriteCompletion::Settled {
                outcome: EditStoreOutcome::Committed,
                ..
            }]
        ));
        assert!(
            LocalAdjustDb::open_at(&path)
                .unwrap()
                .get_layers("key")
                .is_none()
        );
    }

    /// 保存が着地するまで待てること。
    ///
    /// key の付け替え (製本ページの rename) の直前に使う。付け替えの後に古い key の
    /// 保存が着地すると、移したはずの行が古い key で書き戻る。
    #[test]
    fn draining_waits_until_every_queued_write_has_landed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        for i in 0..32 {
            handle.submit(format!("key{i}"), doc("saved"));
        }

        let collected = handle.drain_blocking();

        assert_eq!(collected.len(), 32, "着地する前に待つのをやめている");
        assert!(!handle.is_busy());
        let db = LocalAdjustDb::open_at(&path).unwrap();
        for i in 0..32 {
            assert!(
                db.get_layers(&format!("key{i}")).is_some(),
                "key{i} が書かれていない"
            );
        }
    }

    /// 積んでいなければ待たない。
    #[test]
    fn draining_an_idle_worker_returns_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let handle = LocalAdjustWriteHandle::spawn(dir.path().join("local_adjust.db"));
        assert!(handle.drain_blocking().is_empty());
    }

    /// 終了時に在庫を捨てない。
    ///
    /// `shutdown` を見た瞬間に抜ける実装だと、直前の編集が「メモリにはあるが正本には
    /// 無い」まま終わる。
    #[test]
    fn shutdown_drains_the_queue_before_stopping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local_adjust.db");
        let handle = LocalAdjustWriteHandle::spawn(path.clone());
        for i in 0..32 {
            handle.submit(format!("key{i}"), doc("saved"));
        }

        let results = run_to_completion(handle);

        assert_eq!(results.len(), 32, "積んだ分はすべて処理される");
        let db = LocalAdjustDb::open_at(&path).unwrap();
        for i in 0..32 {
            assert!(
                db.get_layers(&format!("key{i}")).is_some(),
                "key{i} が書かれていない"
            );
        }
    }
}
