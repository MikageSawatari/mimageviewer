//! タグ書き込みのバックグラウンド worker (docs/tag-feature.md §5.6)。
//!
//! UI から複数ファイルへの「タグ追加/削除/クリア」要求を受け取り、
//! 1 ファイルずつシリアルに `xmp_writer::apply_tag_op` を実行する。
//! 完了時は fts_meta.db と Tantivy index の tags フィールドも即時更新する
//! (mtime 監視経由の再 ingest を待たずに次の Ctrl+G で反映)。
//!
//! # なぜ 1 本のシリアル worker か
//!
//! - 並列書き込みは FS ロック競合リスクがあり高速化メリットも小さい
//!   (タグ追加は通常 1 ファイルあたり < 10ms)
//! - UI スレッドとインデックス worker との競合を避けるため別スレッドに逃がす
//!
//! # キャンセル
//!
//! v1.0 では未対応。大量ファイル (>100) で時間がかかるケースが出てきたら
//! Arc<AtomicBool> cancel トークンを追加する。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crossbeam_channel::{Receiver, Sender, unbounded};
use uuid::Uuid;

use crate::fts_index::{self, Container, FtsIndex, IndexDoc};
use crate::fts_meta::FtsMetaDb;
use crate::xmp_writer::{TagOp, WriteError};

/// 1 件の書き込みジョブ。
#[derive(Debug, Clone)]
pub struct TagWriteJob {
    pub path: PathBuf,
    pub op: TagOp,
    /// 検索インデックス更新用の favorite_id。
    /// path が所属するお気に入りが分かっていれば設定する。分からない場合は None
    /// (その場合は再 ingest 経路に任せる)。
    pub favorite_id: Option<Uuid>,
    pub favorite_root: Option<PathBuf>,
}

/// 書き込み 1 件の結果。
#[derive(Debug, Clone)]
pub struct TagWriteResult {
    pub path: PathBuf,
    /// 成功時は編集後のタグ列 (スペース区切り、`#` 接頭辞込み)。
    pub result: Result<String, String>,
}

/// UI 側から worker に接続するためのハンドル。
pub struct TagWriteHandle {
    job_tx: Sender<TagWriteJob>,
    result_rx: Receiver<TagWriteResult>,
    pub total: Arc<AtomicUsize>,
    pub done: Arc<AtomicUsize>,
    pub failures: Arc<AtomicUsize>,
    /// worker スレッドの join handle (drop 時に shutdown)。
    _thread: Option<std::thread::JoinHandle<()>>,
    /// true になると worker はキューを空にした後終了する。
    shutdown: Arc<AtomicBool>,
}

impl TagWriteHandle {
    /// worker スレッドを起動する。
    /// `meta` と `fts` は worker の寿命中ずっと必要なので `Arc` で共有する。
    pub fn spawn(meta: Arc<FtsMetaDb>, fts: Arc<FtsIndex>) -> Self {
        let (job_tx, job_rx) = unbounded::<TagWriteJob>();
        let (result_tx, result_rx) = unbounded::<TagWriteResult>();
        let total = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let w_total = total.clone();
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
                    &w_total,
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

    /// ジョブをキューに積む。
    pub fn submit(&self, job: TagWriteJob) {
        self.total.fetch_add(1, Ordering::Relaxed);
        let _ = self.job_tx.send(job);
    }

    /// 結果を非ブロッキングで取り出す (UI 側で毎フレーム呼ぶ)。
    pub fn try_recv_result(&self) -> Option<TagWriteResult> {
        self.result_rx.try_recv().ok()
    }

    /// アクティブジョブがあるか (total != done)。
    pub fn is_busy(&self) -> bool {
        self.total.load(Ordering::Relaxed) != self.done.load(Ordering::Relaxed)
    }

    /// 全ジョブ完了後にカウンタをリセット (次のバッチ表示のため)。
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
        // job_tx を落として recv を唤起 (shutdown フラグだけでは recv がブロックし続ける)
        // shutdown シグナルは worker のループ条件としても見る。
        // 本当は drop で join したいが、worker が進行中のジョブを完了するまで待つと
        // UI がフリーズするので、バックグラウンドで切れるに任せる (std::thread::spawn は
        // main 終了時に detach される)。
    }
}

fn run_worker(
    job_rx: &Receiver<TagWriteJob>,
    result_tx: &Sender<TagWriteResult>,
    meta: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    _total: &Arc<AtomicUsize>,
    done: &Arc<AtomicUsize>,
    failures: &Arc<AtomicUsize>,
    shutdown: &Arc<AtomicBool>,
) {
    // バッチで Tantivy writer を 1 本だけ確保し、定期 commit する。
    let mut writer = match fts.writer() {
        Ok(w) => Some(w),
        Err(e) => {
            crate::logger::log(format!("tag_write_worker: Tantivy writer 確保失敗: {e}"));
            None
        }
    };
    let mut pending_commits = 0usize;
    const COMMIT_BATCH: usize = 20;

    loop {
        if shutdown.load(Ordering::Relaxed) && job_rx.is_empty() {
            break;
        }
        // タイムアウト付き recv (ポーリング): shutdown を定期的に見るため。
        let job = match job_rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(j) => j,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // 一定時間ジョブが来ない場合は commit を走らせる
                if let Some(w) = writer.as_mut() {
                    if pending_commits > 0 {
                        if let Err(e) = w.commit() {
                            crate::logger::log(format!(
                                "tag_write_worker: Tantivy commit 失敗: {e}"
                            ));
                        } else {
                            let _ = fts.reload_reader();
                        }
                        pending_commits = 0;
                    }
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        let res = process_job(&job, &meta, &fts, writer.as_mut());
        match &res {
            Ok(_) => {
                pending_commits += 1;
            }
            Err(_) => {
                failures.fetch_add(1, Ordering::Relaxed);
            }
        }
        done.fetch_add(1, Ordering::Relaxed);

        let _ = result_tx.send(TagWriteResult {
            path: job.path.clone(),
            result: res.map_err(|e| e.to_string()),
        });

        // バッチ境界で commit
        if pending_commits >= COMMIT_BATCH {
            if let Some(w) = writer.as_mut() {
                if let Err(e) = w.commit() {
                    crate::logger::log(format!(
                        "tag_write_worker: Tantivy commit 失敗: {e}"
                    ));
                } else {
                    let _ = fts.reload_reader();
                }
                pending_commits = 0;
            }
        }
    }

    // 終了時に残りの commit
    if let Some(mut w) = writer {
        if pending_commits > 0 {
            let _ = w.commit();
            let _ = fts.reload_reader();
        }
    }
}

fn process_job(
    job: &TagWriteJob,
    meta: &FtsMetaDb,
    fts: &FtsIndex,
    writer: Option<&mut tantivy::IndexWriter>,
) -> Result<String, WriteError> {
    // 1. XMP 書き込み
    let new_tags = crate::xmp_writer::apply_tag_op(&job.path, &job.op)?;

    // 2. 検索インデックス更新 (favorite_id が分かっているときだけ)
    if let (Some(fav_id), Some(fav_root)) = (job.favorite_id, job.favorite_root.as_ref()) {
        let key = crate::search_index_db::normalize_path(&job.path);
        // fts_meta の tags 列を更新 (all_text_norm はここでは触らない - 次回再 ingest で反映)
        let _ = meta.set_tags(&key, &new_tags);
        // Tantivy 側も tags フィールドだけ upsert
        if let Some(w) = writer {
            // 既存 doc の他フィールド (all_text 等) を維持するには delete→add だと
            // 全部再構築する必要がある。現状は name_only 再投入 (簡易)。
            // 実運用では notify-rs による mtime 変化で再 ingest が走るので、
            // ここは「次の検索で暫定的にヒットさせる」ための quick path として扱う。
            if let Ok(meta_row) = meta.get(&key) {
                if let Some(row) = meta_row {
                    let name = job
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    let doc = IndexDoc {
                        path: key,
                        container: Container::Fs,
                        zip_entry: String::new(),
                        favorite_id: fav_id,
                        mtime: row.mtime,
                        file_size: row.file_size,
                        name,
                        all_text: row.all_text_norm,
                        tags: new_tags.clone(),
                    };
                    if let Err(e) = fts_index::upsert_doc(w, fts.fields(), &doc) {
                        crate::logger::log(format!(
                            "tag_write_worker: upsert_doc 失敗: {e}"
                        ));
                    }
                }
            }
            let _ = fav_root; // 現状未使用、将来のために API に残す
        }
    }

    Ok(new_tags)
}

// ---------------------------------------------------------------------------
// UI 側ヘルパー: トグル判定
// ---------------------------------------------------------------------------

/// 選択ファイル群のタグ状態を見て、次の操作を決定する (docs/tag-feature.md §2.3)。
///
/// - 全ファイルに付与済み → Remove (全削除)
/// - それ以外 (未付与 or 一部のみ) → Add (全付与)
///
/// `tag_with_hash` は `#原神` のように `#` 接頭辞込みの文字列。
/// `file_tags_map` は path → tags (スペース区切り) のマップ。
pub fn decide_toggle_op(
    tag_with_hash: &str,
    paths: &[PathBuf],
    file_tags_map: &std::collections::HashMap<PathBuf, String>,
) -> TagOp {
    let mut all_have = true;
    for p in paths {
        let tags = file_tags_map
            .get(p)
            .map(|s| s.as_str())
            .unwrap_or("");
        if !tags.split_whitespace().any(|t| t == tag_with_hash) {
            all_have = false;
            break;
        }
    }
    if all_have {
        TagOp::Remove(tag_with_hash.to_string())
    } else {
        TagOp::Add(tag_with_hash.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn toggle_all_have_means_remove() {
        let mut map: HashMap<PathBuf, String> = HashMap::new();
        map.insert(PathBuf::from("a.jpg"), "#原神 #風景".into());
        map.insert(PathBuf::from("b.jpg"), "#原神".into());
        let paths = vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")];
        match decide_toggle_op("#原神", &paths, &map) {
            TagOp::Remove(s) => assert_eq!(s, "#原神"),
            _ => panic!("expected Remove"),
        }
    }

    #[test]
    fn toggle_some_missing_means_add() {
        let mut map: HashMap<PathBuf, String> = HashMap::new();
        map.insert(PathBuf::from("a.jpg"), "#原神".into());
        map.insert(PathBuf::from("b.jpg"), "".into());
        let paths = vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")];
        match decide_toggle_op("#原神", &paths, &map) {
            TagOp::Add(s) => assert_eq!(s, "#原神"),
            _ => panic!("expected Add"),
        }
    }

    #[test]
    fn toggle_none_have_means_add() {
        let mut map: HashMap<PathBuf, String> = HashMap::new();
        map.insert(PathBuf::from("a.jpg"), "#風景".into());
        let paths = vec![PathBuf::from("a.jpg")];
        match decide_toggle_op("#原神", &paths, &map) {
            TagOp::Add(_) => {}
            _ => panic!("expected Add"),
        }
    }
}
