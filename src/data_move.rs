//! アプリデータの保存場所を移動するユーティリティ。
//!
//! v0.7.0 の「アプリデータ保存場所の変更機能」で使用。
//! DB / キャッシュ / アーカイブ変換キャッシュなど可動データを、
//! `%APPDATA%\mimageviewer` から任意フォルダ (別ドライブ) へ移す。
//!
//! 基本方針:
//! - **コピー → 検証 → 削除** を 1 ファイルずつ行う (破棄中の中断に耐える)
//! - 途中でエラーや cancel を受け取ったら、移動済みファイルを元に戻す
//! - 進捗は `mpsc::Sender<MoveProgress>` で UI に通知
//!
//! 対象ファイル (bootstrap 側に残すものは除く):
//! - `catalog.db`
//! - `rotation.db`
//! - `rating.db`
//! - `adjustment.db`
//! - `mask.db`
//! - `spread.db`
//! - `search_index.db`
//! - `pdf_passwords.json`
//! - `archive_cache.db` / `archive_cache/` (v0.7.0 新規)
//! - `susie_plugins/` (v0.7.0 新規)
//!
//! bootstrap 側に残す:
//! - `settings.json` (ブートストラップに必要)
//! - `logs/` (起動直後からログを取るため)
//! - `models/` (exe から再展開される)
//! - `pdfium.dll` (exe から再展開される)

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// 移動対象のエントリ (ファイル or ディレクトリ)。
#[derive(Debug, Clone)]
pub struct MoveEntry {
    pub name: &'static str,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntryKind {
    File,
    Dir,
}

/// 移動対象エントリの一覧 (bootstrap 側に残るものは含まない)。
pub const MOVE_TARGETS: &[MoveEntry] = &[
    MoveEntry { name: "catalog.db", kind: EntryKind::File },
    MoveEntry { name: "rotation.db", kind: EntryKind::File },
    MoveEntry { name: "rating.db", kind: EntryKind::File },
    MoveEntry { name: "adjustment.db", kind: EntryKind::File },
    MoveEntry { name: "mask.db", kind: EntryKind::File },
    MoveEntry { name: "spread.db", kind: EntryKind::File },
    MoveEntry { name: "search_index.db", kind: EntryKind::File },
    MoveEntry { name: "pdf_passwords.json", kind: EntryKind::File },
    MoveEntry { name: "archive_cache.db", kind: EntryKind::File },
    MoveEntry { name: "archive_cache", kind: EntryKind::Dir },
    MoveEntry { name: "susie_plugins", kind: EntryKind::Dir },
];

/// UI への進捗通知。
#[derive(Debug, Clone)]
pub enum MoveProgress {
    /// 計測完了: 総バイト数 / 総ファイル数。
    Prepared { total_bytes: u64, total_files: u64 },
    /// ファイル 1 つのコピー完了。
    FileCopied { bytes_done: u64, files_done: u64, current: String },
    /// 全件完了。
    Completed,
    /// キャンセルまたは失敗 → ロールバック完了。
    Failed { message: String },
}

/// 移動計画。
pub struct MovePlan {
    pub from: PathBuf,
    pub to: PathBuf,
    /// (src_path, dst_path, kind, bytes) のリスト。存在するエントリのみ。
    pub items: Vec<(PathBuf, PathBuf, EntryKind, u64)>,
    pub total_bytes: u64,
    pub total_files: u64,
}

/// 移動計画を立てる (I/O 発生するがファイル操作はしない)。
pub fn plan_move(from: &Path, to: &Path) -> std::io::Result<MovePlan> {
    let mut items = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut total_files: u64 = 0;

    for entry in MOVE_TARGETS {
        let src = from.join(entry.name);
        if !src.exists() {
            continue;
        }
        let dst = to.join(entry.name);
        let (bytes, files) = match entry.kind {
            EntryKind::File => {
                let size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
                (size, 1)
            }
            EntryKind::Dir => dir_size_and_count(&src)?,
        };
        total_bytes = total_bytes.saturating_add(bytes);
        total_files = total_files.saturating_add(files);
        items.push((src, dst, entry.kind, bytes));
    }

    Ok(MovePlan { from: from.to_path_buf(), to: to.to_path_buf(), items, total_bytes, total_files })
}

/// 計画を実行する。エラー時はコピー済みを元に戻す。
///
/// `progress` に進捗イベントを送る (UI ビジー表示用)。
/// `cancel` が true になったら移動を中断してロールバックする。
pub fn execute_move(
    plan: MovePlan,
    progress: Sender<MoveProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let _ = progress.send(MoveProgress::Prepared {
        total_bytes: plan.total_bytes,
        total_files: plan.total_files,
    });

    std::fs::create_dir_all(&plan.to)
        .map_err(|e| format!("移動先を作成できません: {} ({e})", plan.to.display()))?;

    let mut copied: Vec<(PathBuf, PathBuf, EntryKind)> = Vec::new();
    let mut bytes_done: u64 = 0;
    let mut files_done: u64 = 0;

    for (src, dst, kind, _bytes) in plan.items.iter() {
        if cancel.load(Ordering::Relaxed) {
            rollback(&copied, &progress);
            return Err("移動がキャンセルされました。".to_string());
        }

        // 移動先に既存があれば失敗 (上書きはユーザーの意図しない破損を招くため)
        if dst.exists() {
            rollback(&copied, &progress);
            let msg = format!("移動先に既存のファイルがあります: {}", dst.display());
            let _ = progress.send(MoveProgress::Failed { message: msg.clone() });
            return Err(msg);
        }

        let result = match kind {
            EntryKind::File => copy_and_verify_file(src, dst),
            EntryKind::Dir => copy_dir_recursive(src, dst, &cancel),
        };

        if let Err(e) = result {
            rollback(&copied, &progress);
            let msg = format!("{} のコピーに失敗: {e}", src.display());
            let _ = progress.send(MoveProgress::Failed { message: msg.clone() });
            return Err(msg);
        }

        copied.push((src.clone(), dst.clone(), *kind));

        match kind {
            EntryKind::File => {
                bytes_done = bytes_done.saturating_add(
                    std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0),
                );
                files_done += 1;
            }
            EntryKind::Dir => {
                let (b, f) = dir_size_and_count(dst).unwrap_or((0, 0));
                bytes_done = bytes_done.saturating_add(b);
                files_done = files_done.saturating_add(f);
            }
        }

        let _ = progress.send(MoveProgress::FileCopied {
            bytes_done,
            files_done,
            current: src.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string(),
        });
    }

    // ここまで来れば全エントリがコピー済み & 検証済み。元を削除する。
    // 削除失敗は警告するが全体を失敗扱いにはしない (新しい場所は完成済み)。
    for (src, _dst, kind) in copied.iter() {
        let res = match kind {
            EntryKind::File => std::fs::remove_file(src),
            EntryKind::Dir => std::fs::remove_dir_all(src),
        };
        if let Err(e) = res {
            crate::logger::log(format!(
                "[data_move] 旧データ削除失敗 {} ({e})", src.display()
            ));
        }
    }

    let _ = progress.send(MoveProgress::Completed);
    Ok(())
}

fn rollback(copied: &[(PathBuf, PathBuf, EntryKind)], progress: &Sender<MoveProgress>) {
    for (_src, dst, kind) in copied.iter() {
        let res = match kind {
            EntryKind::File => std::fs::remove_file(dst),
            EntryKind::Dir => std::fs::remove_dir_all(dst),
        };
        if let Err(e) = res {
            crate::logger::log(format!(
                "[data_move] ロールバック削除失敗 {} ({e})", dst.display()
            ));
        }
    }
    let _ = progress.send(MoveProgress::Failed {
        message: "移動を中止し元に戻しました。".to_string(),
    });
}

fn copy_and_verify_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    // 検証: サイズが一致するか
    let src_len = std::fs::metadata(src)?.len();
    let dst_len = std::fs::metadata(dst)?.len();
    if src_len != dst_len {
        let _ = std::fs::remove_file(dst);
        return Err(std::io::Error::other(format!(
            "size mismatch: src={src_len} dst={dst_len}"
        )));
    }
    // 小さいファイル (<= 4MB) は内容ハッシュでも検証 (DB は壊れると致命的)
    if src_len <= 4 * 1024 * 1024 {
        let src_hash = hash_file(src)?;
        let dst_hash = hash_file(dst)?;
        if src_hash != dst_hash {
            let _ = std::fs::remove_file(dst);
            return Err(std::io::Error::other("hash mismatch"));
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path, cancel: &Arc<AtomicBool>) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("canceled"));
        }
        let entry = entry?;
        let kind = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if kind.is_dir() {
            copy_dir_recursive(&src_path, &dst_path, cancel)?;
        } else if kind.is_file() {
            copy_and_verify_file(&src_path, &dst_path)?;
        }
        // シンボリックリンクやデバイスファイルは無視 (app data には出現しない想定)
    }
    Ok(())
}

fn dir_size_and_count(dir: &Path) -> std::io::Result<(u64, u64)> {
    let mut bytes: u64 = 0;
    let mut files: u64 = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let (b, f) = dir_size_and_count(&entry.path())?;
            bytes = bytes.saturating_add(b);
            files = files.saturating_add(f);
        } else if kind.is_file() {
            bytes = bytes.saturating_add(entry.metadata()?.len());
            files += 1;
        }
    }
    Ok((bytes, files))
}

fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let out = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    Ok(arr)
}

// -----------------------------------------------------------------------
// ランタイム状態 (起動時 move を UI から監視するため)
// -----------------------------------------------------------------------

/// 起動時データ移動の実行状態。App がメインループで参照する。
///
/// Windows Named Mutex (DataMove ガード) はワーカースレッド内で保持し続け、
/// スレッド終了時に Drop → 他インスタンスの起動禁止を解除する。
/// MoveRunState 自身は HANDLE を持たないので Send + Sync が成立する。
pub struct MoveRunState {
    pub from: PathBuf,
    pub to: PathBuf,
    pub total_bytes: AtomicU64,
    pub total_files: AtomicU64,
    pub bytes_done: AtomicU64,
    pub files_done: AtomicU64,
    pub current: Mutex<String>,
    pub finished: AtomicBool,
    pub result: Mutex<Option<Result<(), String>>>,
    pub cancel: Arc<AtomicBool>,
}

impl MoveRunState {
    /// 移動スレッドを起動してランタイム状態を返す。
    pub fn spawn(
        from: PathBuf,
        to: PathBuf,
        #[cfg(windows)] guard: Option<crate::instance_lock::NamedMutexGuard>,
    ) -> Arc<Self> {
        let state = Arc::new(MoveRunState {
            from: from.clone(),
            to: to.clone(),
            total_bytes: AtomicU64::new(0),
            total_files: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            current: Mutex::new(String::new()),
            finished: AtomicBool::new(false),
            result: Mutex::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
        });

        let (tx, rx) = std::sync::mpsc::channel::<MoveProgress>();
        let state_for_pump = Arc::clone(&state);
        std::thread::spawn(move || pump_progress(rx, state_for_pump));

        let cancel = Arc::clone(&state.cancel);
        let state_for_worker = Arc::clone(&state);
        let new_root = to.clone();
        #[cfg(windows)]
        let guard_for_worker = guard;
        std::thread::spawn(move || {
            // ガードをスレッド寿命の末尾まで保持 (Drop で Mutex 解放)
            #[cfg(windows)]
            let _hold_guard = guard_for_worker;

            let plan = match plan_move(&from, &to) {
                Ok(p) => p,
                Err(e) => {
                    *state_for_worker.result.lock().unwrap() =
                        Some(Err(format!("移動計画を作成できません: {e}")));
                    state_for_worker.finished.store(true, Ordering::Release);
                    return;
                }
            };
            let result = execute_move(plan, tx, cancel);
            // 成功なら settings.json を更新 (pending_move クリア、data_root 設定)。
            // DataMove ガードを保持している間に書き込むので他インスタンスとの競合はない。
            if result.is_ok() {
                update_settings_after_move(&new_root);
            } else {
                // 失敗時は pending_move を残さない (ユーザーが手動でやり直せる)。
                clear_pending_move_in_settings();
            }
            *state_for_worker.result.lock().unwrap() = Some(result);
            state_for_worker.finished.store(true, Ordering::Release);
        });

        state
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

fn update_settings_after_move(new_root: &Path) {
    let mut s = crate::settings::Settings::load();
    s.pending_move = None;
    s.data_root = Some(new_root.to_path_buf());
    s.save();
}

fn clear_pending_move_in_settings() {
    let mut s = crate::settings::Settings::load();
    s.pending_move = None;
    s.save();
}

fn pump_progress(rx: Receiver<MoveProgress>, state: Arc<MoveRunState>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            MoveProgress::Prepared { total_bytes, total_files } => {
                state.total_bytes.store(total_bytes, Ordering::Relaxed);
                state.total_files.store(total_files, Ordering::Relaxed);
            }
            MoveProgress::FileCopied { bytes_done, files_done, current } => {
                state.bytes_done.store(bytes_done, Ordering::Relaxed);
                state.files_done.store(files_done, Ordering::Relaxed);
                *state.current.lock().unwrap() = current;
            }
            MoveProgress::Completed | MoveProgress::Failed { .. } => {
                // finished flag は worker 側で立てるのでここでは何もしない
            }
        }
    }
}

/// 指定パスが実用上書き込み可能か (テストファイルを作って消す)。
/// フォルダ選択時の事前チェックに使う。
pub fn probe_writable(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Err(format!("{} が存在しません。", dir.display()));
    }
    if !dir.is_dir() {
        return Err(format!("{} はフォルダではありません。", dir.display()));
    }
    let probe = dir.join(".mimageviewer_probe");
    match std::fs::File::create(&probe).and_then(|mut f| f.write_all(b"probe")) {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(format!("書き込みテストに失敗: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn plan_move_skips_missing_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("from");
        let to = tmp.path().join("to");
        std::fs::create_dir_all(&from).unwrap();
        // from に 1 つだけターゲットを配置
        std::fs::write(from.join("rotation.db"), b"x").unwrap();

        let plan = plan_move(&from, &to).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.total_files, 1);
        assert_eq!(plan.total_bytes, 1);
    }

    #[test]
    fn execute_move_copies_and_deletes_source() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("from");
        let to = tmp.path().join("to");
        std::fs::create_dir_all(&from).unwrap();
        let payload = b"hello, world\x00\x01\x02";
        std::fs::write(from.join("rotation.db"), payload).unwrap();
        std::fs::write(from.join("rating.db"), b"rate").unwrap();
        std::fs::create_dir_all(from.join("archive_cache/sub")).unwrap();
        std::fs::write(from.join("archive_cache/sub/a.zip"), b"zip").unwrap();

        let plan = plan_move(&from, &to).unwrap();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let result = execute_move(plan, tx, cancel);

        assert!(result.is_ok(), "move should succeed: {:?}", result);

        // 移動先に存在
        assert!(to.join("rotation.db").exists());
        assert!(to.join("rating.db").exists());
        assert!(to.join("archive_cache/sub/a.zip").exists());
        assert_eq!(std::fs::read(to.join("rotation.db")).unwrap(), payload);

        // 移動元から削除
        assert!(!from.join("rotation.db").exists());
        assert!(!from.join("rating.db").exists());
        assert!(!from.join("archive_cache").exists());

        // 進捗イベントが届いていること (Prepared + FileCopied + Completed)
        let msgs: Vec<_> = rx.try_iter().collect();
        assert!(msgs.iter().any(|m| matches!(m, MoveProgress::Prepared { .. })));
        assert!(msgs.iter().any(|m| matches!(m, MoveProgress::FileCopied { .. })));
        assert!(msgs.iter().any(|m| matches!(m, MoveProgress::Completed)));
    }

    #[test]
    fn execute_move_aborts_when_dst_has_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("from");
        let to = tmp.path().join("to");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join("rotation.db"), b"src").unwrap();
        // 前回のキルで残った部分状態を再現
        std::fs::write(to.join("rotation.db"), b"leftover").unwrap();

        let plan = plan_move(&from, &to).unwrap();
        let (tx, _rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let result = execute_move(plan, tx, cancel);

        assert!(result.is_err(), "should refuse to overwrite");
        // 元ファイルは消されていないこと (rollback)
        assert!(from.join("rotation.db").exists());
    }
}
