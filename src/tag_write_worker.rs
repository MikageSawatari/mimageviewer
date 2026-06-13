//! タグ更新のバックグラウンド worker。
//!
//! UI からの「タグ X を付与/削除」「すべてクリア」要求を受け取り、1 item ずつ
//! シリアルに `tags.db` を更新する。メディア本体 / XMP サイドカー / Tantivy
//! には通常タグ操作から書き込まない。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSidecarTarget {
    pub folder: PathBuf,
    pub rel_key: String,
}

pub fn sidecar_target_for_real_file(path: &std::path::Path) -> Option<TagSidecarTarget> {
    Some(TagSidecarTarget {
        folder: path.parent()?.to_path_buf(),
        // rel_key の導出式は sidecar.rs の正本を共有する (式が割れると同じ .dat 内で
        // タグだけキーが食い違う)。
        rel_key: crate::sidecar::real_file_rel_key(path)?,
    })
}

/// UI が worker に渡す操作。
#[derive(Debug, Clone)]
pub enum TagJobKind {
    /// 現在のタグ状態を worker が tags.db から読み出し、含まれていれば Remove、
    /// 含まれていなければ Add を実行する。単体操作や旧テスト用。
    Toggle(String),
    /// 指定タグを付与する。
    Add(String),
    /// 指定タグを削除する。
    Remove(String),
    /// mIV タグをすべて削除。
    ClearMiv,
    /// mIV タグを指定リストで完全置換する。Undo/Redo で「操作直前の状態」に
    /// 戻すために使う。Toggle の逆操作だと外部ツールでの書き換え後にズレるが、
    /// この置換ジョブなら mIV が記録した状態へ確実に戻せる。
    SetTags(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct TagWriteJob {
    pub path: PathBuf,
    pub kind: TagJobKind,
    /// タグ用 sidecar のミラー先。フォルダタグや仮想ページなど、sidecar 対象外なら None。
    pub tag_sidecar: Option<TagSidecarTarget>,
    /// Undo entry を最終確定するための取引 ID。同じ user 操作 (1 トグル / 1 クリア) で
    /// 投入された全ジョブは同じ tx_id を共有し、UI 側の `pending_tag_undos` で
    /// 集計される。0 なら Undo 確定不要 (例: Undo/Redo 由来の SetTags ジョブ自体)。
    pub tx_id: u64,
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
    /// SetTags: Undo/Redo による状態復元で tags.db のタグ一覧を置き換えた。
    Restored,
    /// 実質変化なし (clear した時に元々空だったケース等)。
    NoOp,
}

#[derive(Debug, Clone)]
pub struct TagWriteResult {
    pub path: PathBuf,
    /// 成功時に UI スレッドで sidecar へミラーするため、投入時の座標をそのまま返す。
    pub tag_sidecar: Option<TagSidecarTarget>,
    pub result: Result<TagAction, String>,
    /// 更新**直前**の mIV タグ一覧。
    /// Undo entry の `before` を確定させるために使う — UI 側の予測 (= tags_cache)
    /// が stale になっていた場合でも、worker が読んだ DB 状態から Undo を組み立てる。
    /// 失敗時も worker が op を解決するために読み取った値をそのまま入れる。
    pub tags_before: Vec<String>,
    /// 更新後の mIV タグ一覧 (成功時のみ意味あり、失敗時は空)。
    /// UI 側はこれを `tags_cache` に直接書き戻すことでグリッドバッジを即時反映する。
    pub tags_after: Vec<String>,
    /// 投入時のトランザクション ID をエコーバック。`TagWriteJob::tx_id` 参照。
    pub tx_id: u64,
}

pub struct TagWriteHandle {
    job_tx: Sender<TagWriteJob>,
    result_rx: Receiver<TagWriteResult>,
    pub total: Arc<AtomicUsize>,
    pub done: Arc<AtomicUsize>,
    pub failures: Arc<AtomicUsize>,
    /// 旧 FTS 反映時代の互換フィールド。tags.db 専用化後は常に 0。
    pub pending_in_writer: Arc<AtomicUsize>,
    _thread: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl TagWriteHandle {
    /// worker スレッドを起動する。
    pub fn spawn() -> Self {
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

    /// tags.db 更新が完了するまで busy を維持する。
    /// `pending_in_writer` は旧 FTS 反映時代の互換値で、通常は 0。
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
        // on_exit → App::drop → ここの流れで、worker が在庫ジョブを drain し切るのを待つ。
        // 終了直前にキューされた tags.db 更新を失わないため、rating worker と同じく join する。
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self._thread.take() {
            let _ = thread.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    job_rx: &Receiver<TagWriteJob>,
    result_tx: &Sender<TagWriteResult>,
    done: &Arc<AtomicUsize>,
    failures: &Arc<AtomicUsize>,
    pending_in_writer: &Arc<AtomicUsize>,
    shutdown: &Arc<AtomicBool>,
) {
    let mut db = match crate::tags_db::TagsDb::open() {
        Ok(db) => Some(db),
        Err(e) => {
            crate::logger::log(format!("tag_write_worker: tags.db open failed: {e}"));
            None
        }
    };

    loop {
        if shutdown.load(Ordering::Relaxed) && job_rx.is_empty() {
            break;
        }
        let job = match job_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(j) => j,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        // spawn 時の open が一時要因 (AV スキャン / 他プロセスのファイルロック) で
        // 失敗していても、セッション中ずっと全タグ操作を失敗させない —
        // ジョブごとに再 open を試みる (worker は初回タグ操作時に 1 度だけ spawn
        // されるため、ここで回復しないと再起動以外に復旧手段が無い)。
        if db.is_none() {
            match crate::tags_db::TagsDb::open() {
                Ok(reopened) => {
                    crate::logger::log("tag_write_worker: tags.db reopened after earlier failure");
                    db = Some(reopened);
                }
                Err(e) => {
                    crate::logger::log(format!("tag_write_worker: tags.db reopen failed: {e}"));
                }
            }
        }
        let (res, tags_before, tags_after) = match db.as_mut() {
            Some(db) => process_job(&job, db),
            None => (
                Err("tags.db を開けませんでした".to_string()),
                Vec::new(),
                Vec::new(),
            ),
        };
        if res.is_err() {
            failures.fetch_add(1, Ordering::Relaxed);
        }
        pending_in_writer.store(0, Ordering::Relaxed);

        done.fetch_add(1, Ordering::Relaxed);
        let _ = result_tx.send(TagWriteResult {
            path: job.path.clone(),
            tag_sidecar: job.tag_sidecar.clone(),
            result: res.map_err(|e| e.to_string()),
            tags_before,
            tags_after,
            tx_id: job.tx_id,
        });
    }
    pending_in_writer.store(0, Ordering::Relaxed);
}

/// ジョブを 1 件処理する。戻り値は:
/// - `Result<TagAction, String>`: UI 側トースト用の結果ラベル
/// - `Vec<String>` tags_before: 更新直前の mIV タグ一覧 (Undo entry の `before` に使う)
/// - `Vec<String>` tags_after: 更新後の mIV タグ一覧 (エラー時は空)。
fn process_job(
    job: &TagWriteJob,
    db: &mut crate::tags_db::TagsDb,
) -> (Result<TagAction, String>, Vec<String>, Vec<String>) {
    let path_disp = job.path.display();
    let item_key = crate::tags_db::item_key_for_path(&job.path);
    let tags_before = db.display_tags_for_item(&item_key);

    let result = match &job.kind {
        TagJobKind::Toggle(name) => {
            let with_hash = crate::tags_db::format_display_tag(name);
            crate::logger::log(format!("[TAG] worker: toggle {with_hash:?} | {path_disp}"));
            db.toggle_item_tag(&item_key, name)
                .map(|(outcome, _before, after)| {
                    let action = match outcome {
                        crate::tags_db::TagToggleOutcome::Added => TagAction::Added,
                        crate::tags_db::TagToggleOutcome::Removed => TagAction::Removed,
                        crate::tags_db::TagToggleOutcome::NoOp => TagAction::NoOp,
                    };
                    (action, after)
                })
        }
        TagJobKind::Add(name) => {
            let wanted = crate::tags_db::format_display_tag(name);
            let mut next = tags_before.clone();
            if !next.iter().any(|tag| tag == &wanted) {
                next.push(wanted);
            }
            db.set_item_tags(
                &item_key,
                next.iter()
                    .map(|tag| crate::tags_db::strip_display_hash(tag)),
                crate::tags_db::source::EDIT,
            )
            .map(|after| {
                let action = if after == tags_before {
                    TagAction::NoOp
                } else {
                    TagAction::Added
                };
                (action, after)
            })
        }
        TagJobKind::Remove(name) => {
            let wanted_key = crate::tags_db::normalize_tag_key(name);
            let next: Vec<String> = tags_before
                .iter()
                .filter(|tag| crate::tags_db::normalize_tag_key(tag) != wanted_key)
                .cloned()
                .collect();
            db.set_item_tags(
                &item_key,
                next.iter()
                    .map(|tag| crate::tags_db::strip_display_hash(tag)),
                crate::tags_db::source::EDIT,
            )
            .map(|after| {
                let action = if after == tags_before {
                    TagAction::NoOp
                } else {
                    TagAction::Removed
                };
                (action, after)
            })
        }
        TagJobKind::ClearMiv => {
            crate::logger::log(format!("[TAG] worker: clear mIV tags | {path_disp}"));
            db.clear_item_tags(&item_key)
                .map(|(changed, _before, after)| {
                    (
                        if changed {
                            TagAction::Cleared
                        } else {
                            TagAction::NoOp
                        },
                        after,
                    )
                })
        }
        TagJobKind::SetTags(target) => {
            // SetTags は Undo/Redo 経路でしか使わないので、DB が既に target に
            // 一致していても `Restored` を返す。`NoOp` を返すと
            // tag_ops の `format_completion_toast` で「mIV タグをクリア」誤表示になるため。
            crate::logger::log(format!(
                "[TAG] worker: SetTags current={tags_before:?} target={target:?} | {path_disp}"
            ));
            db.set_item_tags(
                &item_key,
                target
                    .iter()
                    .map(|tag| crate::tags_db::strip_display_hash(tag)),
                crate::tags_db::source::EDIT,
            )
            .map(|after| (TagAction::Restored, after))
        }
    };

    match result {
        Ok((action, tags_after)) => {
            crate::logger::log(format!(
                "[TAG] worker: tags.db update OK, tags_after={tags_after:?} | {path_disp}"
            ));
            (Ok(action), tags_before, tags_after)
        }
        Err(e) => {
            crate::logger::log(format!(
                "[TAG] worker: tags.db update FAILED ({e}) | {path_disp}"
            ));
            (Err(e.to_string()), tags_before, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kinds_clone() {
        let j1 = TagJobKind::Toggle("tag".into());
        let j2 = TagJobKind::ClearMiv;
        let j3 = TagJobKind::SetTags(vec!["#a".into(), "#b".into()]);
        assert!(matches!(j1.clone(), TagJobKind::Toggle(_)));
        assert!(matches!(j2.clone(), TagJobKind::ClearMiv));
        assert!(matches!(j3.clone(), TagJobKind::SetTags(_)));
    }

    /// `is_busy()` は互換フィールド `pending_in_writer > 0` も busy 扱いにする。
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
        assert!(handle.is_busy(), "pending が残る間は busy を維持する");

        // pending クリア後に busy が下がる。
        pending_in_writer.store(0, Ordering::Relaxed);
        assert!(!handle.is_busy(), "pending クリア後は busy=false");
    }
}
