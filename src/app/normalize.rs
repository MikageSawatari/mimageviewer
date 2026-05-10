//! 動画音量ノーマライズの App 制御専用構造体 + handler 群。
//!
//! 共有 POD 型 (`NormalizeUiState` / `NormalizeOverlayState` 等) は
//! `crate::video::normalize_types` に置く (= video → app の依存逆転を防ぐため)。
//! 本ファイルは App が単独で持つ worker handle / mpsc rx / cancel atomic 等の制御構造のみ。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use crate::video::normalize_scanner::{NormalizeScanError, NormalizeScanProgress};
use crate::video::normalize_types::NormalizeResult;

/// スキャン worker と App を繋ぐ進行中 state。`App.normalize_state: Option<Self>` で持つ。
///
/// 同時スキャンは禁止 (= 全 App で 1 つだけ)。新規スキャンを始める前に古い state を
/// take() して `cancel.store(true)` で worker を停止させる。`_join` を drop することで
/// JoinHandle も解放され、worker thread は cancel atomic を見て早期 return する。
pub struct NormalizeScanState {
    pub fs_idx: usize,
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<NormalizeScanProgress>,
    pub rx: mpsc::Receiver<NormalizeMessage>,
    /// スキャン開始時点の再生状態 (= スキャン後に再開すべきかどうか)。
    pub was_playing: bool,
    /// スキャン対象ファイルの絶対パス。stale fs_idx 復活防止に使う。
    pub file_path: PathBuf,
    /// スキャン開始時点の Settings.target_lufs_milli (= clamp 済み)。Settings の変更で
    /// 別キー保存になるのを防ぐため、開始時の値を固定保持する。
    pub target_lufs_milli: i32,
    /// worker thread。drop されると detached になるが、worker は cancel atomic を
    /// 見て早期 return するので問題ない。
    pub _join: JoinHandle<()>,
}

/// worker → App の完了通知。
#[derive(Debug)]
pub enum NormalizeMessage {
    /// スキャン成功。`NormalizeResult` を DB に保存し gain を適用する。
    Done(NormalizeResult),
    /// FFmpeg / 計算エラー (詳細メッセージ付き)。DB 保存しない、UI 状態は OnUnmeasured に戻す。
    Error(String),
    /// ユーザーキャンセル。DB 保存しない、UI 状態は OnUnmeasured に戻す。
    Cancelled,
}

impl From<Result<NormalizeResult, NormalizeScanError>> for NormalizeMessage {
    fn from(r: Result<NormalizeResult, NormalizeScanError>) -> Self {
        match r {
            Ok(result) => Self::Done(result),
            Err(NormalizeScanError::Cancelled) => Self::Cancelled,
            Err(e) => Self::Error(e.to_string()),
        }
    }
}

impl NormalizeScanState {
    /// cancel atomic を立てる (worker は次回 atomic 確認で early return)。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
