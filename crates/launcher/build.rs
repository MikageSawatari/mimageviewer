//! ランチャークレートのビルドスクリプト。
//!
//! - 内包する `mimageviewer-core.exe`、`mimageviewer-remote.exe` と FFmpeg DLL のパスを `MIMV_*` 環境変数
//!   経由でソースに渡す (`include_bytes!(env!("..."))` で参照)。
//! - core と remote が先にビルドされていることを確認し、未ビルドなら明確なエラーで止める。
//! - exe アイコンを埋め込む (本体と同じアイコン)。

use std::path::PathBuf;

use sha2::{Digest, Sha256};

#[path = "src/build_const_parser.rs"]
mod build_const_parser;

use build_const_parser::extract_const;

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
    let remote_exe = target_dir.join("release").join("mimageviewer-remote.exe");

    let missing = [
        (
            &core_exe,
            "mimageviewer-core.exe",
            "cargo build --release --bin mimageviewer-core",
        ),
        (
            &remote_exe,
            "mimageviewer-remote.exe",
            "cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets",
        ),
    ]
    .into_iter()
    .filter(|(path, _, _)| !path.exists())
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        eprintln!();
        eprintln!("================================================================");
        eprintln!(" Launcher input executable(s) not found:");
        for (path, name, command) in missing {
            eprintln!("   {name}: {}", path.display());
            eprintln!("     build with: {command}");
        }
        eprintln!();
        eprintln!(" Build core and remote before the launcher, or use the wrapper:");
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
        ("MIMV_AVFILTER_DLL", "avfilter-10.dll"),
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
        println!("cargo:rustc-env={var}_SHA256={}", sha256_file_hex(&p));
        println!("cargo:rerun-if-changed={}", p.display());
    }

    println!("cargo:rustc-env=MIMV_CORE_EXE={}", core_exe.display());
    println!(
        "cargo:rustc-env=MIMV_CORE_EXE_SHA256={}",
        sha256_file_hex(&core_exe)
    );
    println!("cargo:rerun-if-changed={}", core_exe.display());
    println!("cargo:rustc-env=MIMV_REMOTE_EXE={}", remote_exe.display());
    println!(
        "cargo:rustc-env=MIMV_REMOTE_EXE_SHA256={}",
        sha256_file_hex(&remote_exe)
    );
    println!("cargo:rerun-if-changed={}", remote_exe.display());

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
    let open_path_pipe_name = extract_const(&src, "OPEN_PATH_PIPE_NAME").unwrap_or_else(|| {
        eprintln!("could not extract OPEN_PATH_PIPE_NAME from src/single_instance.rs");
        std::process::exit(1);
    });
    println!("cargo:rustc-env=MIMV_MUTEX_NAME={mutex_name}");
    println!("cargo:rustc-env=MIMV_ACTIVATE_EVENT_NAME={activate_name}");
    println!("cargo:rustc-env=MIMV_OPEN_PATH_PIPE_NAME={open_path_pipe_name}");
    println!("cargo:rerun-if-changed={}", single_instance_rs.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir
            .join("src")
            .join("build_const_parser.rs")
            .display()
    );

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

fn sha256_file_hex(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("read {} failed for sha256: {e}", path.display());
        std::process::exit(1);
    });
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
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
