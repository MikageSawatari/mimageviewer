//! TRT pack オンラインインストールの E2E スモーク (CLI 版)。
//!
//! `MIV_TRT_PACK_BASE_URL` で指している HTTP サーバーから pack を取得し、
//! `tensorrt_installer::start_install` のフルパスを通す。GUI を使わない検証用。
//!
//! ## 使い方
//!
//! ```bash
//! # 別ターミナルで
//! cd dist/trt-pack-v2
//! python -m http.server 8000
//!
//! # 本ターミナルで (空の tensorrt/ ディレクトリから始める想定)
//! MIV_TRT_PACK_BASE_URL=http://127.0.0.1:8000 \
//!   cargo run --release --bin test_trt_install
//! ```
//!
//! exit 0 = インストール成功 (= INSTALL_OK 書かれた、is_pack_installed() == true)。
//! 進捗は stdout に逐次出力。一時的なテスト用、リリース版インストーラには
//! 含めない (リリースビルドからは feature ゲートで除外しても OK だが、現状は
//! 単に「出荷物に紛れて使われない」前提で残す)。

use std::time::Instant;

use mimageviewer::ai::tensorrt_installer::{InstallProgress, start_install};
use mimageviewer::ai::tensorrt_pack;

fn main() {
    println!(
        "[test_trt_install] base url: {}",
        std::env::var("MIV_TRT_PACK_BASE_URL")
            .unwrap_or_else(|_| "<default GitHub Releases>".to_string())
    );
    println!(
        "[test_trt_install] pack_dir: {}",
        tensorrt_pack::pack_dir().display()
    );

    let target_sm = mimageviewer::gpu_info::query_primary_gpu_sm();
    println!("[test_trt_install] target SM: {:?}", target_sm);

    let start = Instant::now();
    let mut handle = start_install(target_sm);

    let mut last_overall_print = Instant::now();
    let mut current_file = String::new();
    let mut total_bytes: u64 = 0;
    let mut bytes_done: u64 = 0;
    let mut last_file_done: u64 = 0;
    let mut last_file_name = String::new();

    loop {
        if let Some(ev) = handle.poll() {
            match ev {
                InstallProgress::FetchingManifest => {
                    println!("[event] fetching manifest");
                }
                InstallProgress::ManifestFetched {
                    pack_version,
                    total_files,
                    total_bytes: tb,
                    engine_pack_label,
                } => {
                    total_bytes = tb;
                    println!(
                        "[event] manifest fetched: pack_version={}, files={}, total={:.1} MiB, engine_pack={}",
                        pack_version,
                        total_files,
                        tb as f64 / 1024.0 / 1024.0,
                        engine_pack_label
                    );
                }
                InstallProgress::StartingFile {
                    name,
                    file_index,
                    total_files,
                    bytes_total,
                } => {
                    println!(
                        "[event] starting [{}/{}] {} ({:.1} MiB)",
                        file_index + 1,
                        total_files,
                        name,
                        bytes_total as f64 / 1024.0 / 1024.0
                    );
                    current_file = name.clone();
                    last_file_name = name;
                    last_file_done = 0;
                }
                InstallProgress::FileProgress {
                    name,
                    bytes_done: bd,
                    bytes_total,
                } => {
                    if name == last_file_name {
                        let delta = bd.saturating_sub(last_file_done);
                        bytes_done = bytes_done.saturating_add(delta);
                        last_file_done = bd;
                    } else {
                        last_file_name = name.clone();
                        last_file_done = bd;
                        bytes_done = bytes_done.saturating_add(bd);
                    }
                    let _ = bytes_total;
                    if last_overall_print.elapsed().as_millis() >= 250 {
                        let pct = if total_bytes > 0 {
                            100.0 * bytes_done as f64 / total_bytes as f64
                        } else {
                            0.0
                        };
                        println!(
                            "[progress] {} ({:.1} / {:.1} MiB, overall {:.1}%)",
                            current_file,
                            bd as f64 / 1024.0 / 1024.0,
                            bytes_total as f64 / 1024.0 / 1024.0,
                            pct
                        );
                        last_overall_print = Instant::now();
                    }
                }
                InstallProgress::VerifyingFile { name } => {
                    println!("[event] verifying {}", name);
                }
                InstallProgress::ExtractingEngine { entry_index, total } => {
                    if (entry_index + 1) % 5 == 0 || entry_index + 1 == total {
                        println!(
                            "[event] extracting engine entry {}/{}",
                            entry_index + 1,
                            total
                        );
                    }
                }
                InstallProgress::Done => {
                    println!("[event] DONE in {:.1}s", start.elapsed().as_secs_f64());
                    break;
                }
                InstallProgress::Error { message } => {
                    eprintln!("[event] ERROR: {}", message);
                    std::process::exit(1);
                }
            }
        } else if handle.is_finished() {
            // チャネル空 + thread 死亡 = 異常終了
            eprintln!("[error] worker terminated without Done/Error event");
            std::process::exit(1);
        } else {
            // ちょっと寝てから再ポーリング
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    if !tensorrt_pack::is_pack_installed() {
        eprintln!("[error] is_pack_installed() == false after Done event");
        std::process::exit(1);
    }
    println!("[test_trt_install] OK (is_pack_installed=true)");
}
