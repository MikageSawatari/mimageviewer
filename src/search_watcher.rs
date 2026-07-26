//! ファイルシステム監視 + debounce (docs/archive/search-metadata/search-expansion-design.md §7.1〜7.3)。
//!
//! お気に入りルートを再帰的に watch し、create/modify/remove/rename イベントを
//! 500ms デバウンスして「変更された path 集合」を Diff Applier に送る。
//!
//! ## プラットフォーム
//!
//! Windows: `ReadDirectoryChangesW` (notify-rs が内部で使用)。
//! ネットワーク共有 (SMB, NAS) では発火しないケースがあるため、上位レイヤーで
//! 定期ポーリング走査 (§7.2) を併走させる想定。
//!
//! ## デザイン
//!
//! - `FsWatcher` 構造体が notify::Watcher を保持、crossbeam-channel でイベント受信
//! - 独立スレッドでイベントを集約・デバウンスし、`DebouncedChange` を送信
//! - 500ms ウィンドウ内の同一 path イベントはまとめる
//! - 下流の Diff Applier は `DebouncedChange` を受け取って fts_meta.db と照合
//!
//! ## キャンセル / 停止
//!
//! `FsWatcher` を drop すると notify::Watcher も drop され、内部スレッドも終了する
//! (`notify_stop: AtomicBool` で明示停止)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use uuid::Uuid;

/// debounce 後に確定した変更イベント。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebouncedChange {
    pub favorite_id: Uuid,
    pub path: PathBuf,
    pub kind: ChangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    /// 作成 or 変更 (どちらかは mtime/size 比較で後段が判定)
    Upsert,
    /// 削除 or rename 元
    Remove,
}

/// debounce ウィンドウ (§7.2 "デバウンス必須 (notify::event::Event が短時間に数百件")。
const DEBOUNCE_MS: u64 = 500;
/// バッファオーバーフロー時のフル再スキャン通知用。典型は notify-rs が `Any` kind で来る。
pub const OVERFLOW_MARKER_PATH: &str = "<overflow>";

/// 1 お気に入り分の watcher。複数お気に入りを同時 watch するなら複数 `FsWatcher` を持つ。
/// `RecursiveMode::Recursive` なのでサブディレクトリも自動的に監視対象に入る。
pub struct FsWatcher {
    favorite_id: Uuid,
    _watcher: RecommendedWatcher, // drop で監視停止
    stop_flag: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl FsWatcher {
    /// `root` を watch してデバウンス済み変更を `out_tx` に送る。
    ///
    /// `favorite_id` は下流の Diff Applier が「どのお気に入りスコープの変更か」を
    /// 判別するためにイベントに載せる。
    pub fn start(
        favorite_id: Uuid,
        root: &Path,
        out_tx: Sender<DebouncedChange>,
    ) -> notify::Result<Self> {
        let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<notify::Result<Event>>();

        // notify-rs イベントハンドラ: crossbeam-channel に転送するだけ
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = raw_tx.send(res);
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_cl = Arc::clone(&stop_flag);

        let handle = std::thread::Builder::new()
            .name(format!("fs-watcher-{}", favorite_id.as_simple()))
            .spawn(move || debounce_loop(favorite_id, raw_rx, out_tx, stop_flag_cl))
            .map_err(|e| notify::Error::generic(&format!("spawn debounce thread: {e}")))?;

        Ok(Self {
            favorite_id,
            _watcher: watcher,
            stop_flag,
            thread_handle: Some(handle),
        })
    }

    pub fn favorite_id(&self) -> Uuid {
        self.favorite_id
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        // 停止シグナル: debounce_loop が次の recv_timeout (最長 DEBOUNCE_MS/2 = 250ms) で
        // stop_flag をチェックし、break して終了する。
        // 実装は通常の join で、タイムアウト付き join はしていない (Codex 6 回目指摘 #7):
        //   - 通常ケース: debounce_loop が 250ms 以内に stop_flag を見て終わる → 即 join
        //   - notify 側の Sender が drop される経路: raw_rx が Disconnected → break
        // 多数 favorite を UI スレッドから一斉 drop すると ~250ms × N の累積になる可能性あり。
        // App 統合時は Supervisor drop を UI スレッド外 (別スレッド) に寄せる方針で設計すること。
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(h) = self.thread_handle.take() {
            let _ = h.join();
        }
        // _watcher は self が drop されたときに自動で drop される (DropGuard 同様)
    }
}

/// notify イベントを集約 → 500ms debounce → 確定した変更を送出する。
fn debounce_loop(
    favorite_id: Uuid,
    raw_rx: Receiver<notify::Result<Event>>,
    out_tx: Sender<DebouncedChange>,
    stop_flag: Arc<AtomicBool>,
) {
    // 同一 path の最新イベントだけを覚える (map で上書きしていく)
    let pending: Mutex<HashMap<PathBuf, PendingEntry>> = Mutex::new(HashMap::new());

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }
        // 直近で受信できるイベントがあれば貪欲に取り込む
        // タイムアウトで flush の機会を作る
        match raw_rx.recv_timeout(Duration::from_millis(DEBOUNCE_MS / 2)) {
            Ok(Ok(event)) => {
                let mut guard = pending.lock().unwrap();
                absorb_event(&event, &mut guard);
            }
            Ok(Err(e)) => {
                // notify 内部エラー (バッファオーバーフロー等)。
                // 上位レイヤーでは overflow として全再走査をトリガすべき。
                let _ = out_tx.send(DebouncedChange {
                    favorite_id,
                    path: PathBuf::from(OVERFLOW_MARKER_PATH),
                    kind: ChangeKind::Upsert,
                });
                crate::logger::log(format!("fs-watcher[{favorite_id}]: notify error: {e}"));
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // flush チェック
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }

        // flush: 最後のイベントから DEBOUNCE_MS 以上経過した entry を送出
        let now = Instant::now();
        let mut to_emit = Vec::new();
        {
            let mut guard = pending.lock().unwrap();
            guard.retain(|path, entry| {
                if now.saturating_duration_since(entry.last_seen).as_millis() as u64 >= DEBOUNCE_MS
                {
                    to_emit.push((path.clone(), entry.kind));
                    false
                } else {
                    true
                }
            });
        }
        for (path, kind) in to_emit {
            let _ = out_tx.send(DebouncedChange {
                favorite_id,
                path,
                kind,
            });
        }
    }

    // 終了時に pending に残っているものを全部吐き出す
    let residuals: Vec<_> = {
        let mut guard = pending.lock().unwrap();
        guard.drain().map(|(p, e)| (p, e.kind)).collect()
    };
    for (path, kind) in residuals {
        let _ = out_tx.send(DebouncedChange {
            favorite_id,
            path,
            kind,
        });
    }
}

struct PendingEntry {
    kind: ChangeKind,
    last_seen: Instant,
}

/// 1 件の notify::Event を pending map に吸収する。
fn absorb_event(event: &Event, pending: &mut HashMap<PathBuf, PendingEntry>) {
    use notify::event::{ModifyKind, RenameMode};
    let kind = match event.kind {
        // **Windows rename の正確な分解** (docs/search-test-plan.md rename バグ):
        // `ReadDirectoryChangesW` は rename を
        //   1. Modify(Name(From)) 旧パス
        //   2. Modify(Name(To))   新パス
        //   3. (or)  Modify(Name(Both)) で両パスを 1 イベントにまとめる場合あり
        // として届ける。旧パスを Upsert で扱うと「存在しないファイル」扱いで
        // silently no-op → 旧パスが fts_meta / Tantivy に残り続けるバグがあった。
        // From は Remove、To / Both は Upsert に割り当てる。
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => ChangeKind::Remove,
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => ChangeKind::Upsert,
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => ChangeKind::Upsert,
        // RenameMode::Any / Other はプラットフォーム依存で情報不足。Upsert 扱いにし、
        // supervisor 側のフォールバック (build_candidate_from_path 失敗 → Remove) で
        // 救う (indexer_supervisor.rs の apply_single_change::Upsert 分岐参照)。
        EventKind::Create(_) | EventKind::Modify(_) => ChangeKind::Upsert,
        EventKind::Remove(_) => ChangeKind::Remove,
        EventKind::Access(_) => return, // 読み取りアクセスは無視
        EventKind::Any | EventKind::Other => {
            // バッファオーバーフロー等の可能性 — 呼び出し側ではエラーとして
            // OVERFLOW_MARKER_PATH が飛ぶが、ここは upsert として扱うのが安全
            ChangeKind::Upsert
        }
    };
    let now = Instant::now();
    for path in &event.paths {
        // rename は (Remove old + Create new) の 2 イベントで届くので、
        // 同じ path に Remove + Upsert が連続したら Upsert 側を優先 (最新状態を反映)。
        pending.insert(
            path.clone(),
            PendingEntry {
                kind,
                last_seen: now,
            },
        );
    }
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------
//
// ReadDirectoryChangesW を実際に起動するテストは CI で flakey になりやすいので、
// `absorb_event` など純粋ロジックに限定した単体テストに留める。
// E2E のテストは統合テスト (tests/) 側で env var ガード付きで追加する予定 (v0.8.x)。

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RemoveKind};

    fn make_event(kind: EventKind, paths: Vec<PathBuf>) -> Event {
        Event {
            kind,
            paths,
            attrs: Default::default(),
        }
    }

    #[test]
    fn create_event_upsert() {
        let mut pending = HashMap::new();
        absorb_event(
            &make_event(
                EventKind::Create(CreateKind::File),
                vec![PathBuf::from("C:/a/b.jpg")],
            ),
            &mut pending,
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending.get(&PathBuf::from("C:/a/b.jpg")).unwrap().kind,
            ChangeKind::Upsert
        );
    }

    #[test]
    fn remove_event_remove() {
        let mut pending = HashMap::new();
        absorb_event(
            &make_event(
                EventKind::Remove(RemoveKind::File),
                vec![PathBuf::from("C:/a/b.jpg")],
            ),
            &mut pending,
        );
        assert_eq!(
            pending.get(&PathBuf::from("C:/a/b.jpg")).unwrap().kind,
            ChangeKind::Remove
        );
    }

    #[test]
    fn modify_after_remove_becomes_upsert() {
        // rename で (Remove + Create) が連続したら最新状態 (Upsert) で上書きされる
        let p = PathBuf::from("C:/a/b.jpg");
        let mut pending = HashMap::new();
        absorb_event(
            &make_event(EventKind::Remove(RemoveKind::File), vec![p.clone()]),
            &mut pending,
        );
        absorb_event(
            &make_event(EventKind::Create(CreateKind::File), vec![p.clone()]),
            &mut pending,
        );
        assert_eq!(pending.get(&p).unwrap().kind, ChangeKind::Upsert);
    }

    #[test]
    fn access_event_ignored() {
        let mut pending = HashMap::new();
        absorb_event(
            &make_event(
                EventKind::Access(notify::event::AccessKind::Read),
                vec![PathBuf::from("C:/a/b.jpg")],
            ),
            &mut pending,
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn multiple_modifies_same_path_collapse() {
        let p = PathBuf::from("C:/a/b.jpg");
        let mut pending = HashMap::new();
        for _ in 0..10 {
            absorb_event(
                &make_event(EventKind::Modify(ModifyKind::Any), vec![p.clone()]),
                &mut pending,
            );
        }
        assert_eq!(pending.len(), 1, "同じ path は 1 エントリに集約");
    }
}
