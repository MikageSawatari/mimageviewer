//! Small launcher for the distributable `mimageviewer.exe`.
//!
//! The real application binary (`mimageviewer-core.exe`) imports FFmpeg DLLs at
//! process load time. The launcher therefore extracts the core exe and FFmpeg
//! DLLs into `%APPDATA%/mimageviewer/runtime/<version>/` first, then spawns the
//! core from that directory.

#![windows_subsystem = "windows"]

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const VERSION: &str = env!("CARGO_PKG_VERSION");

static CORE_EXE: &[u8] = include_bytes!(env!("MIMV_CORE_EXE"));
static AVCODEC_DLL: &[u8] = include_bytes!(env!("MIMV_AVCODEC_DLL"));
static AVFORMAT_DLL: &[u8] = include_bytes!(env!("MIMV_AVFORMAT_DLL"));
static AVUTIL_DLL: &[u8] = include_bytes!(env!("MIMV_AVUTIL_DLL"));
static AVFILTER_DLL: &[u8] = include_bytes!(env!("MIMV_AVFILTER_DLL"));
static SWSCALE_DLL: &[u8] = include_bytes!(env!("MIMV_SWSCALE_DLL"));
static SWRESAMPLE_DLL: &[u8] = include_bytes!(env!("MIMV_SWRESAMPLE_DLL"));

const ASSETS: &[(&str, &[u8], &str)] = &[
    ("avutil-59.dll", AVUTIL_DLL, env!("MIMV_AVUTIL_DLL_SHA256")),
    (
        "swresample-5.dll",
        SWRESAMPLE_DLL,
        env!("MIMV_SWRESAMPLE_DLL_SHA256"),
    ),
    (
        "swscale-8.dll",
        SWSCALE_DLL,
        env!("MIMV_SWSCALE_DLL_SHA256"),
    ),
    (
        "avfilter-10.dll",
        AVFILTER_DLL,
        env!("MIMV_AVFILTER_DLL_SHA256"),
    ),
    (
        "avcodec-61.dll",
        AVCODEC_DLL,
        env!("MIMV_AVCODEC_DLL_SHA256"),
    ),
    (
        "avformat-61.dll",
        AVFORMAT_DLL,
        env!("MIMV_AVFORMAT_DLL_SHA256"),
    ),
    (
        "mimageviewer-core.exe",
        CORE_EXE,
        env!("MIMV_CORE_EXE_SHA256"),
    ),
];

fn main() {
    if let Err(e) = run() {
        show_error(&e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let user_args: Vec<OsString> = std::env::args_os().skip(1).collect();

    // --version / -V / --help / -h: 版 / usage を表示して終了 (core を spawn しない)。
    // 既存インスタンスの activate より前に処理する。
    if maybe_handle_version_or_help(&user_args) {
        return Ok(());
    }

    #[cfg(windows)]
    if try_activate_existing(&user_args) {
        return Ok(());
    }

    let runtime_dir = appdata_runtime_dir()?;
    std::fs::create_dir_all(&runtime_dir)
        .map_err(|e| format!("create runtime dir failed ({}): {e}", runtime_dir.display()))?;

    for (name, bytes, expected_hash) in ASSETS {
        let path = runtime_dir.join(name);
        ensure_asset(&path, bytes, expected_hash).map_err(|e| {
            format!(
                "extract {} failed: {e}\n(runtime dir: {})",
                name,
                runtime_dir.display()
            )
        })?;
    }

    let core_path = runtime_dir.join("mimageviewer-core.exe");
    let launcher_path = std::env::current_exe().ok();

    let mut cmd = Command::new(&core_path);
    cmd.args(&user_args);
    if let Some(path) = launcher_path {
        cmd.env("MIV_LAUNCHER_EXE_PATH", path);
    }
    cmd.spawn()
        .map_err(|e| format!("spawn core failed ({}): {e}", core_path.display()))?;

    Ok(())
}

/// `--version` / `-V` / `--help` / `-h` を処理する。該当すれば文面を親コンソールへ
/// 出力して `true` を返す (呼び出し側は core を spawn せず終了する)。GUI exe でも
/// `mimageviewer.exe --version` でバージョンを CLI から確認できるようにする。
fn maybe_handle_version_or_help(args: &[OsString]) -> bool {
    let (mut want_version, mut want_help) = (false, false);
    for a in args {
        match a.to_str() {
            Some("--version") | Some("-V") => want_version = true,
            Some("--help") | Some("-h") => want_help = true,
            _ => {}
        }
    }
    if want_help {
        write_to_parent_console(&format!(
            "mImageViewer {VERSION}\n\
             \n\
             Usage: mimageviewer.exe [OPTIONS] [PATH]\n\
             \n\
             Options:\n  \
             -V, --version  Print version and exit\n  \
             -h, --help     Print this help and exit\n\
             \n\
             PATH  Open the given image file or folder on startup.\n"
        ));
        true
    } else if want_version {
        write_to_parent_console(&format!("mImageViewer {VERSION}\n"));
        true
    } else {
        false
    }
}

/// 親プロセス (cmd / PowerShell) のコンソールへ文字列を出力する。GUI exe
/// (`windows_subsystem="windows"`) は既定でコンソールを持たないため、
/// `AttachConsole(ATTACH_PARENT_PROCESS)` で親コンソールに接続してから、
/// `GetStdHandle(STD_OUTPUT_HANDLE)` の handle へ `WriteFile` する。`WriteFile` は
/// console / file / pipe いずれにも書けるので `--version > ver.txt` のリダイレクトでも拾える
/// (出力は ASCII)。親コンソールなし・リダイレクトなしのときは handle 無効で何も出さない。
#[cfg(windows)]
fn write_to_parent_console(msg: &str) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::WriteFile;
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        let handle: HANDLE = match GetStdHandle(STD_OUTPUT_HANDLE) {
            Ok(h) if !h.is_invalid() => h,
            _ => return,
        };
        let mut written = 0u32;
        let _ = WriteFile(
            handle,
            Some(msg.as_bytes()),
            Some(&mut written as *mut u32),
            None,
        );
    }
}

#[cfg(not(windows))]
fn write_to_parent_console(msg: &str) {
    print!("{msg}");
}

fn appdata_runtime_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA")
        .ok_or_else(|| "APPDATA environment variable not set".to_string())?;
    Ok(PathBuf::from(appdata)
        .join("mimageviewer")
        .join("runtime")
        .join(VERSION))
}

fn ensure_asset(path: &Path, bytes: &[u8], expected_hash: &str) -> std::io::Result<()> {
    let hash_path = sidecar_hash_path(path);

    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() == bytes.len() as u64 && asset_hash_matches(path, &hash_path, &expected_hash)?
        {
            return Ok(());
        }
    }

    write_atomic(path, bytes)?;
    write_atomic(&hash_path, expected_hash.as_bytes())?;
    Ok(())
}

fn asset_hash_matches(path: &Path, hash_path: &Path, expected_hash: &str) -> std::io::Result<bool> {
    if let Ok(stored) = std::fs::read_to_string(hash_path) {
        return Ok(stored.trim().eq_ignore_ascii_case(expected_hash));
    }

    let actual_hash = sha256_file_hex(path)?;
    if actual_hash.eq_ignore_ascii_case(expected_hash) {
        write_atomic(hash_path, expected_hash.as_bytes())?;
        return Ok(true);
    }
    Ok(false)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_path = tmp_path_for(path);
    std::fs::write(&tmp_path, bytes)?;

    // T57 (Codex P2 / 2026-05-16): 旧コードは `remove_file(path)` → `rename(tmp, path)` の
    // 2 ステップで、削除と rename の間に他プロセスが path を開くと「ファイル無し」を見る
    // race window があった。`data_dir.rs` は同じ pattern を 65bde65c で削除済。`std::fs::
    // rename` は Windows でも同名既存ファイルを atomic replace するので remove_file は不要
    // (version-scoped runtime dir のおかげで発火確率は低かったが、他所で修正済の旧パターン
    // を残さないために統一する)。
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp_path = path.to_path_buf();
    let mut tmp_name = tmp_path
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| OsString::from("unknown"));
    tmp_name.push(".tmp");
    tmp_path.set_file_name(tmp_name);
    tmp_path
}

fn sidecar_hash_path(path: &Path) -> PathBuf {
    let mut hash_path = path.to_path_buf();
    let mut name = hash_path
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| OsString::from("unknown"));
    name.push(".sha256");
    hash_path.set_file_name(name);
    hash_path
}

fn sha256_file_hex(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(windows)]
fn try_activate_existing(user_args: &[OsString]) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        EVENT_MODIFY_STATE, OpenEventW, OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS, SetEvent,
    };
    use windows::core::PCWSTR;

    const MUTEX_NAME: &str = env!("MIMV_MUTEX_NAME");
    const ACTIVATE_EVENT_NAME: &str = env!("MIMV_ACTIVATE_EVENT_NAME");
    const SYNCHRONIZE: SYNCHRONIZATION_ACCESS_RIGHTS = SYNCHRONIZATION_ACCESS_RIGHTS(0x00100000);

    let mutex_wide = wide_nul(MUTEX_NAME);
    let Ok(mutex_handle) = (unsafe { OpenMutexW(SYNCHRONIZE, false, PCWSTR(mutex_wide.as_ptr())) })
    else {
        return false;
    };
    unsafe {
        let _ = CloseHandle(mutex_handle);
    }

    if let Some(path) =
        parse_startup_open_path_arg_from(user_args).map(absolutize_startup_open_path)
    {
        let _ = send_open_path_to_existing(&path);
    }

    let event_wide = wide_nul(ACTIVATE_EVENT_NAME);
    if let Ok(event_handle) =
        unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_wide.as_ptr())) }
    {
        unsafe {
            let _ = SetEvent(event_handle);
            let _ = CloseHandle(event_handle);
        }
    }
    true
}

#[cfg(windows)]
fn parse_startup_open_path_arg_from(args: &[OsString]) -> Option<PathBuf> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.as_os_str() == std::ffi::OsStr::new("--") {
            return args.get(i + 1).map(|arg| PathBuf::from(arg.as_os_str()));
        }

        if let Some(flag) = arg.to_str()
            && flag.starts_with("--")
        {
            if flag == "--perf-log"
                && args
                    .get(i + 1)
                    .and_then(|next| next.to_str())
                    .is_some_and(|next| !next.starts_with("--"))
            {
                i += 2;
                continue;
            }
            i += if cli_flag_takes_value(flag) { 2 } else { 1 };
            continue;
        }

        return Some(PathBuf::from(arg.as_os_str()));
    }
    None
}

#[cfg(windows)]
fn absolutize_startup_open_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

#[cfg(windows)]
fn cli_flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--data-dir"
            | "--window-size"
            | "--perf-log-path"
            | "--play-test"
            | "--play-duration"
            | "--play-test-start"
            | "--dcomp-presenter-test"
            | "--dcomp-duration"
            | "--dcomp-window-size"
            | "--dcomp-sync-interval"
            | "--dcomp-start"
    )
}

#[cfg(windows)]
fn send_open_path_to_existing(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, GetLastError};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING, WriteFile,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::core::PCWSTR;

    const OPEN_PATH_MAX_U16: usize = 32_767;
    const OPEN_PATH_PIPE_NAME: &str = env!("MIMV_OPEN_PATH_PIPE_NAME");

    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.len() > OPEN_PATH_MAX_U16 {
        return false;
    }
    let mut message = Vec::with_capacity(4 + wide.len() * 2);
    message.extend_from_slice(&(wide.len() as u32).to_le_bytes());
    for unit in wide {
        message.extend_from_slice(&unit.to_le_bytes());
    }

    let name_wide = wide_nul(OPEN_PATH_PIPE_NAME);
    for _ in 0..60 {
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name_wide.as_ptr()),
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match handle {
            Ok(handle) => {
                let mut bytes = message.as_slice();
                let mut ok = true;
                while !bytes.is_empty() {
                    let mut written = 0_u32;
                    if unsafe { WriteFile(handle, Some(bytes), Some(&mut written), None) }.is_err()
                        || written == 0
                    {
                        ok = false;
                        break;
                    }
                    bytes = &bytes[written as usize..];
                }
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return ok;
            }
            Err(_) => {
                if unsafe { GetLastError() } == ERROR_PIPE_BUSY {
                    unsafe {
                        let _ = WaitNamedPipeW(PCWSTR(name_wide.as_ptr()), 100);
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }
    false
}

#[cfg(windows)]
fn wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn show_error(msg: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::PCWSTR;

    let title = wide_nul("mImageViewer ランチャーエラー");
    let body = wide_nul(msg);
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_error(msg: &str) {
    eprintln!("{msg}");
}
