//! 編集用追加パックの導入フローをヘッドレスで実行する検証 probe (開発専用)。
//!
//! 実際の `editing_addon_download::start_install` (= download → sha256 検証 → 展開 →
//! manifest 検証 → 配置 → active.json) を回し、公開予定の pack 成果物が実コードで
//! 正しく導入できるかを GUI 無しで確認する。
//!
//! debug ビルド限定で `MIV_EDITING_PACK_BASE_URL` により配信元を上書きできるので、
//! ローカル HTTP サーバへ向けて使う:
//!
//!   # ターミナル A: 成果物ディレクトリを配信
//!   (cd dist/editing-pack-publish && python -m http.server 8099)
//!   # ターミナル B:
//!   MIV_EDITING_PACK_BASE_URL=http://127.0.0.1:8099 cargo run --bin probe_editing_install
//!
//! release ビルドでは override が無視される (security) ので必ず debug で実行する。

use std::time::Duration;

use mimageviewer::editing_addon;
use mimageviewer::editing_addon_download::{InstallProgress, start_install};

fn main() {
    println!(
        "base url override (debug only): {:?}",
        std::env::var("MIV_EDITING_PACK_BASE_URL").ok()
    );
    println!("install 前 status: {:?}", editing_addon::addon_status());

    let mut handle = start_install();
    let mut terminal: Option<InstallProgress> = None;
    loop {
        if let Some(p) = handle.poll() {
            // Downloading は進捗が多いので bytes だけ簡潔に。
            match &p {
                InstallProgress::Downloading {
                    bytes_done,
                    bytes_total,
                } => {
                    if *bytes_total > 0 {
                        println!(
                            "  Downloading {:.1}%",
                            *bytes_done as f64 / *bytes_total as f64 * 100.0
                        );
                    }
                }
                other => println!("  {other:?}"),
            }
            if p.is_terminal() {
                terminal = Some(p);
                break;
            }
        } else if handle.is_finished() {
            break;
        } else {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    println!("terminal: {terminal:?}");
    println!("install 後 status: {:?}", editing_addon::addon_status());
    println!(
        "subject_matte_model_path: {:?}",
        editing_addon::subject_matte_model_path()
    );
    println!(
        "installed_fonts: {} 書体",
        editing_addon::installed_fonts().len()
    );

    match terminal {
        Some(InstallProgress::Done { .. })
            if editing_addon::subject_matte_model_path().is_some()
                && !editing_addon::installed_fonts().is_empty() =>
        {
            println!("RESULT: OK (導入成功 + 被写体モデル/フォント解決)");
        }
        _ => {
            eprintln!("RESULT: FAILED");
            std::process::exit(1);
        }
    }
}
