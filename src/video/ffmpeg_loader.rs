//! FFmpeg DLL の検索パス検証 (実体は exe と同じディレクトリに同梱)。
//!
//! ## 配布方針
//! `ffmpeg-the-third` は MSVC import library 経由でリンクされるため、
//! Windows ローダが exe をプロセスにロードする時点 (= Rust コードが走るより前) に
//! `avcodec-61.dll` などを解決する必要がある。PDFium / ONNX Runtime のように
//! 「include_bytes! → 起動後に APPDATA 展開 → 動的ロード」する方式は本体 (core)
//! では使えない (Rust コードが起動する前にローダが走るため間に合わない)。
//! `/DELAYLOAD` (delay-load DLL) も検証したが、rustc 経由の link.exe 呼び出しでは
//! Delay Import Directory が空のまま生成され、警告も出ないため動作しない
//! (詳細は `build.rs` 上部のコメントと CLAUDE.md「FFmpeg LGPL DLL 管理」節)。
//!
//! そこでランチャー + 本体の 2 段構成を採る:
//!
//! - 配布する `mimageviewer.exe` (= launcher、`crates/launcher/`) が FFmpeg 6 DLL
//!   と本体 `mimageviewer-core.exe` を `include_bytes!` で内包する。
//! - 起動時に launcher が `%APPDATA%/mimageviewer/runtime/<version>/` へ全ファイルを
//!   展開し、その場所から core を spawn する。
//! - core は exe と同じディレクトリに DLL が並んでいるので Windows ローダの
//!   標準検索順 (exe 同居が最優先) で確実に解決される。
//!
//! 開発時に直接 `target/release/mimageviewer-core.exe` を起動したい場合は
//! `Copy-Item vendor/ffmpeg/bin/*.dll target/release/` で同じ配置を再現する。
//!
//! 本モジュールは「DLL が exe と同じ場所にあるか」を確認してログに出すだけ
//! (実体ロードは Windows ローダが既に行っている)。

use std::path::PathBuf;

const REQUIRED_DLLS: &[&str] = &[
    "avcodec-61.dll",
    "avformat-61.dll",
    "avutil-59.dll",
    "avfilter-10.dll",
    "swscale-8.dll",
    "swresample-5.dll",
];

/// 動画再生プレイヤー作成時に呼ぶ。冪等。
///
/// ※ Windows ローダが exe ロード時点で既に DLL を解決済みなので、ここでの作業は
/// ログ出力のみ (DLL が見つからなければ exe ロード自体に失敗してこの関数は呼ばれない)。
pub fn init() -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .map_err(|e| format!("current_exe failed: {e}"))?
        .parent()
        .ok_or_else(|| "exe path has no parent".to_string())?
        .to_path_buf();
    for name in REQUIRED_DLLS {
        let p = exe_dir.join(name);
        if !p.exists() {
            crate::logger::log(format!(
                "warning: FFmpeg DLL {} not found alongside exe (DLL search may have used PATH)",
                p.display()
            ));
        }
    }
    crate::logger::log(format!("ffmpeg DLLs expected at {}", exe_dir.display()));
    Ok(exe_dir)
}
