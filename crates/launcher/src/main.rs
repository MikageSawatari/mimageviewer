//! `mimageviewer.exe` ランチャー。配布する単体 exe はこれ。
//!
//! 役割:
//! 1. `%APPDATA%\mimageviewer\runtime\<version>\` に本体 (`mimageviewer-core.exe`)
//!    と FFmpeg LGPL DLL 5 つを展開する (サイズ一致チェックでスキップ、不一致なら
//!    `.tmp` → atomic rename で更新)。
//! 2. 展開した core を `std::process::Command::spawn` で起動 (引数を forward)。
//!    `std::process::Command` が Windows の `CreateProcessW` 引数引用符付けを
//!    内部で正しく行うので、自分で escape する必要は無い。
//! 3. ランチャーは即座に終了する (GUI アプリなので exit code は待たない)。
//!
//! ### なぜランチャー方式か
//! 本体は `ffmpeg-the-third` を MSVC import library 経由で使うため、Windows ローダ
//! が exe ロード時 (Rust コードが走るより前) に `avcodec-61.dll` 等を解決しようと
//! する。`include_bytes!` → APPDATA 展開方式は間に合わない。`/DELAYLOAD` も
//! rustc 経由の link.exe では機能しなかった (Delay Import Directory が空のまま)。
//!
//! ランチャーは **FFmpeg API を一切呼ばない** ので、Windows ローダの DLL 解決問題に
//! 直撃しない。core を spawn する時点で展開済み DLL が core と同じディレクトリに
//! あるので、Windows の DLL 検索順 (exe 同居が最優先) で確実に解決される。
//!
//! ### バージョン別 runtime ディレクトリ
//! 古いバージョンの core が走行中に新ランチャーが上書きしようとして file lock で
//! 失敗するのを避けるため、`%APPDATA%\mimageviewer\runtime\<version>\` の
//! バージョン別フォルダに展開する (Codex レビュー助言)。

#![windows_subsystem = "windows"]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// build.rs が `MIMV_*` 環境変数で展開先パスを設定済み。include_bytes! は
// 文字列リテラル要求だが env!() は文字列リテラル相当に展開されるので OK。
static CORE_EXE: &[u8] = include_bytes!(env!("MIMV_CORE_EXE"));
static AVCODEC_DLL: &[u8] = include_bytes!(env!("MIMV_AVCODEC_DLL"));
static AVFORMAT_DLL: &[u8] = include_bytes!(env!("MIMV_AVFORMAT_DLL"));
static AVUTIL_DLL: &[u8] = include_bytes!(env!("MIMV_AVUTIL_DLL"));
static SWSCALE_DLL: &[u8] = include_bytes!(env!("MIMV_SWSCALE_DLL"));
static SWRESAMPLE_DLL: &[u8] = include_bytes!(env!("MIMV_SWRESAMPLE_DLL"));

const ASSETS: &[(&str, &[u8])] = &[
    // core を最初に展開する (DLL が無い瞬間に core が走る隙を最小化)
    // ※ どのみち spawn は全展開後なので順序は厳密ではないが、見やすさ優先。
    ("avutil-59.dll", AVUTIL_DLL),
    ("swresample-5.dll", SWRESAMPLE_DLL),
    ("swscale-8.dll", SWSCALE_DLL),
    ("avcodec-61.dll", AVCODEC_DLL),
    ("avformat-61.dll", AVFORMAT_DLL),
    ("mimageviewer-core.exe", CORE_EXE),
];

fn main() {
    if let Err(e) = run() {
        show_error(&e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // ── シングルインスタンス検査 (DLL 展開より **前** に行う) ──
    //
    // 既存インスタンスが走っていれば runtime ディレクトリの core / DLL を握っており、
    // ここで `ensure_asset` の rename が ERROR_ACCESS_DENIED で失敗する。
    // 既存に「ウィンドウ復帰せよ」の activate event を投げて、ランチャーは即終了する。
    //
    // 失敗 (mutex なし) なら通常の起動経路へ。core 起動後に core 本体が
    // `SingleInstanceGuard::acquire` で mutex を作る。
    #[cfg(windows)]
    if try_activate_existing() {
        return Ok(());
    }

    let runtime_dir = appdata_runtime_dir()?;
    std::fs::create_dir_all(&runtime_dir)
        .map_err(|e| format!("create runtime dir failed ({}): {e}", runtime_dir.display()))?;

    for (name, bytes) in ASSETS {
        let path = runtime_dir.join(name);
        ensure_asset(&path, bytes).map_err(|e| {
            format!(
                "extract {} failed: {e}\n(runtime dir: {})",
                name,
                runtime_dir.display()
            )
        })?;
    }

    let core_path = runtime_dir.join("mimageviewer-core.exe");

    // ユーザーが渡した引数をそのまま forward。
    // std::process::Command が Windows の CreateProcessW 用に
    // 引用符付け (https://learn.microsoft.com/en-us/cpp/cpp/main-function-command-line-args)
    // を正しく行ってくれる。
    let user_args: Vec<OsString> = std::env::args_os().skip(1).collect();

    Command::new(&core_path)
        .args(&user_args)
        .spawn()
        .map_err(|e| format!("spawn core failed ({}): {e}", core_path.display()))?;

    // GUI アプリなので exit code を待たない。ランチャーは即座に終了し、
    // core はバックグラウンドで自身のウィンドウを開いて動き続ける。
    Ok(())
}

fn appdata_runtime_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA")
        .ok_or_else(|| "APPDATA environment variable not set".to_string())?;
    Ok(PathBuf::from(appdata)
        .join("mimageviewer")
        .join("runtime")
        .join(VERSION))
}

/// 既に同サイズのファイルがあれば skip。なければ `.tmp` 経由で atomic rename する。
///
/// Windows の `std::fs::rename` は宛先ファイルがあると失敗するので、先に
/// `remove_file` を試みる (file lock で失敗してもベスト努力で続行)。
fn ensure_asset(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() == bytes.len() as u64 {
            return Ok(());
        }
    }

    let mut tmp_path = path.to_path_buf();
    let mut tmp_name = tmp_path
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| OsString::from("unknown"));
    tmp_name.push(".tmp");
    tmp_path.set_file_name(tmp_name);

    std::fs::write(&tmp_path, bytes)?;

    // Windows の rename は overwrite 不可なので先に削除。NotFound は無視される。
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// 既に core が起動していたら activate event を SetEvent で叩いて起動済みウィンドウを
/// 前面に戻す。`true` を返したら呼び出し側はランチャーを即終了する。
#[cfg(windows)]
fn try_activate_existing() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        EVENT_MODIFY_STATE, OpenEventW, OpenMutexW, SetEvent, SYNCHRONIZATION_ACCESS_RIGHTS,
    };

    // build.rs が src/single_instance.rs から抜き出して注入する。
    // core 側の定数を変えれば次のビルドで自動的に反映される。
    const MUTEX_NAME: &str = env!("MIMV_MUTEX_NAME");
    const ACTIVATE_EVENT_NAME: &str = env!("MIMV_ACTIVATE_EVENT_NAME");

    // SYNCHRONIZE = 0x00100000。OpenMutexW は最低限 SYNCHRONIZE があれば成功する。
    // 存在テストだけが目的で取得はしない。
    const SYNCHRONIZE: SYNCHRONIZATION_ACCESS_RIGHTS = SYNCHRONIZATION_ACCESS_RIGHTS(0x00100000);

    let mutex_wide = wide_nul(MUTEX_NAME);
    let Ok(mutex_handle) =
        (unsafe { OpenMutexW(SYNCHRONIZE, false, PCWSTR(mutex_wide.as_ptr())) })
    else {
        // Mutex 不在 → 既存 core なし → 通常の起動経路へ
        return false;
    };
    unsafe {
        let _ = CloseHandle(mutex_handle);
    }

    // 既存 core あり → activate event を叩いてウィンドウを前面に戻す
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

/// `&str` を NUL 終端 UTF-16 に変換 (Win32 PCWSTR 用)。
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
