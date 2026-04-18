//! 複数インスタンス間の協調用の名前付き Mutex と、mimageviewer プロセス数の取得。
//!
//! 目的 (v0.7.0 データ移動機能):
//! - **データ移動ガード**: 移動実行中のインスタンスが `Local\mimageviewer_data_move`
//!   を保持し、他インスタンスの起動時にこれを検出して拒否する。
//! - **他インスタンス検知**: 移動予約を受け付ける前に他の mimageviewer プロセスが
//!   いないかを Toolhelp32 で数える。
//!
//! 通常起動では何も制約しない (複数インスタンス可)。

#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, WIN32_ERROR};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};

/// データ移動ガード用の Mutex 名。セッションローカル。
pub const DATA_MOVE_MUTEX_NAME: &str = "Local\\mimageviewer_data_move";

/// 所有している Mutex ハンドル (Drop で解放される)。
pub struct NamedMutexGuard {
    handle: HANDLE,
}

impl Drop for NamedMutexGuard {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

// HANDLE は送信可能だが Sync ではない。自前で保護するので Send のみ実装。
unsafe impl Send for NamedMutexGuard {}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// データ移動ガード Mutex を取得する。既に他インスタンスが保持していれば `Err`。
/// 成功時は返値 (Drop まで保持) の生存期間中、他インスタンスは起動を拒否される。
pub fn acquire_data_move_guard() -> Result<NamedMutexGuard, String> {
    let name = to_wide(DATA_MOVE_MUTEX_NAME);
    unsafe {
        let handle = CreateMutexW(None, true, PCWSTR(name.as_ptr()))
            .map_err(|e| format!("CreateMutexW failed: {e}"))?;
        let err = windows::Win32::Foundation::GetLastError();
        if err == ERROR_ALREADY_EXISTS {
            // 既に他プロセスが保持している。こちらの handle も閉じる。
            let _ = CloseHandle(handle);
            return Err("他のプロセスでデータ移動処理が進行中です。".to_string());
        }
        Ok(NamedMutexGuard { handle })
    }
}

/// データ移動ガード Mutex が他プロセスで保持されているか。
/// 存在確認のみで取得はしない。
pub fn is_data_move_in_progress() -> bool {
    let name = to_wide(DATA_MOVE_MUTEX_NAME);
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

/// 自プロセスを除く mimageviewer.exe の実行数を返す。
///
/// pdf worker (`--pdf-worker`) も `mimageviewer.exe` なので含まれてしまう点に注意。
/// pdf worker は親プロセスが終了すれば追随して終了する (kill-on-close job) ため、
/// 呼び出し側は「親インスタンス数 ≒ mimageviewer プロセス数 - pdf worker 数」
/// として扱うか、本関数を呼ぶ前提の運用 (移動予約時は PDF ワーカー起動前など) を
/// 確立すること。
///
/// v0.7.0 では「移動予約時の他インスタンス検知」のみに使う。pdf worker は
/// 実メインウィンドウを持つインスタンスと同等に扱って 1 としてカウントする
/// (過大検出は安全側に倒れる: 実際に問題ないケースで user に追加確認を促すだけ)。
pub fn count_other_mimageviewer_processes() -> u32 {
    let my_pid = std::process::id();
    let mut count: u32 = 0;
    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return 0,
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32ProcessID != my_pid
                    && wide_exe_matches(&entry.szExeFile, "mimageviewer.exe")
                {
                    count = count.saturating_add(1);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    count
}

fn wide_exe_matches(wide: &[u16], expected_lower: &str) -> bool {
    // NUL 終端までの u16 を String に変換し、ベース名を小文字比較。
    let nul = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    let s = String::from_utf16_lossy(&wide[..nul]);
    let base = std::path::Path::new(&s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    base == expected_lower
}

// WIN32_ERROR をそのまま比較できるようにする別名 (clippy 回避 + 可読性)。
#[allow(dead_code)]
fn last_error() -> WIN32_ERROR {
    unsafe { windows::Win32::Foundation::GetLastError() }
}
