//! `src/xmp_reader` の簡易スモークテスト用 bin。
//! 実ファイルで XMP 抽出 → パース結果を dump する。CI 向けではない。
//!
//! 実行例: `cargo run --bin xmp_smoke -- "C:\path\to\mxd-saved.jpg"`

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: xmp_smoke <image-file>");
        return ExitCode::from(2);
    }
    let path = std::path::Path::new(&args[1]);
    match mimageviewer::xmp_reader::read_tweet_info(path) {
        Some(info) => {
            println!("{info:#?}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("no xtw:* metadata found in {}", path.display());
            ExitCode::from(1)
        }
    }
}
