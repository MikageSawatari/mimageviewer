//! ランチャークレートのビルドスクリプト。
//!
//! - 内包する `mimageviewer-core.exe` と FFmpeg DLL のパスを `MIMV_*` 環境変数
//!   経由でソースに渡す (`include_bytes!(env!("..."))` で参照)。
//! - core が先にビルドされていることを確認し、未ビルドなら明確なエラーで止める。
//! - exe アイコンを埋め込む (本体と同じアイコン)。

use std::path::PathBuf;

/// `pub const NAME: &str = "value";` 形式の文字列リテラル定数を抽出する。
/// `src/single_instance.rs` から MUTEX_NAME / ACTIVATE_EVENT_NAME を拾うために使用。
fn extract_const(src: &str, name: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        let prefix = format!("pub const {name}");
        if !line.starts_with(&prefix) {
            continue;
        }
        let start = line.find('"')?;
        let end = line[start + 1..].find('"')?;
        let raw = &line[start + 1..start + 1 + end];
        // Rust ソース上の `\\` を実際の `\` に展開
        return Some(raw.replace("\\\\", "\\"));
    }
    None
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/launcher/ → crates/ → workspace root
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root inferable from CARGO_MANIFEST_DIR")
        .to_path_buf();

    // CARGO_TARGET_DIR を尊重 (環境変数が無ければ workspace_root/target)
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"));

    let core_exe = target_dir.join("release").join("mimageviewer-core.exe");

    if !core_exe.exists() {
        eprintln!();
        eprintln!("================================================================");
        eprintln!(" mimageviewer-core.exe not found:");
        eprintln!("   {}", core_exe.display());
        eprintln!();
        eprintln!(" 先に core をビルドしてください:");
        eprintln!("   cargo build --release --bin mimageviewer-core");
        eprintln!();
        eprintln!(" もしくは 2 段階ビルドのラッパースクリプト:");
        eprintln!("   bash scripts/build-release.sh        (Git Bash)");
        eprintln!("   .\\scripts\\build-release.ps1         (PowerShell)");
        eprintln!("================================================================");
        eprintln!();
        std::process::exit(1);
    }

    let dll_dir = workspace_root.join("vendor").join("ffmpeg").join("bin");
    let dlls = [
        ("MIMV_AVCODEC_DLL", "avcodec-61.dll"),
        ("MIMV_AVFORMAT_DLL", "avformat-61.dll"),
        ("MIMV_AVUTIL_DLL", "avutil-59.dll"),
        ("MIMV_SWSCALE_DLL", "swscale-8.dll"),
        ("MIMV_SWRESAMPLE_DLL", "swresample-5.dll"),
    ];

    for (var, name) in &dlls {
        let p = dll_dir.join(name);
        if !p.exists() {
            eprintln!("FFmpeg DLL not found: {}", p.display());
            eprintln!("Run: bash scripts/setup-ffmpeg.sh");
            std::process::exit(1);
        }
        println!("cargo:rustc-env={var}={}", p.display());
        println!("cargo:rerun-if-changed={}", p.display());
    }

    println!("cargo:rustc-env=MIMV_CORE_EXE={}", core_exe.display());
    println!("cargo:rerun-if-changed={}", core_exe.display());

    // Single-instance 用の Mutex / Event 名を core 側のソースから取り出して
    // 環境変数経由で渡す。core 側を変えると次のビルドで自動反映される。
    let single_instance_rs = workspace_root.join("src").join("single_instance.rs");
    let src = std::fs::read_to_string(&single_instance_rs).unwrap_or_else(|e| {
        eprintln!("read {} failed: {e}", single_instance_rs.display());
        std::process::exit(1);
    });
    let mutex_name = extract_const(&src, "MUTEX_NAME").unwrap_or_else(|| {
        eprintln!("could not extract MUTEX_NAME from src/single_instance.rs");
        std::process::exit(1);
    });
    let activate_name = extract_const(&src, "ACTIVATE_EVENT_NAME").unwrap_or_else(|| {
        eprintln!("could not extract ACTIVATE_EVENT_NAME from src/single_instance.rs");
        std::process::exit(1);
    });
    println!("cargo:rustc-env=MIMV_MUTEX_NAME={mutex_name}");
    println!("cargo:rustc-env=MIMV_ACTIVATE_EVENT_NAME={activate_name}");
    println!("cargo:rerun-if-changed={}", single_instance_rs.display());

    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set("FileDescription", "mImageViewer");
        res.set("ProductName", "mImageViewer");
        res.set("FileVersion", &format!("{}.0", env!("CARGO_PKG_VERSION")));
        res.set(
            "ProductVersion",
            &format!("{}.0", env!("CARGO_PKG_VERSION")),
        );
        res.set("LegalCopyright", "Copyright (C) 2026 Mikage Sawatari");
        res.set("OriginalFilename", "mimageviewer.exe");

        let icon = workspace_root.join("assets").join("icon.ico");
        if icon.exists() {
            res.set_icon(icon.to_str().unwrap());
        }

        if let Err(e) = res.compile() {
            eprintln!("winresource compile error: {e}");
        }
    }
}
