//! データディレクトリの管理。
//!
//! `%APPDATA%\mimageviewer` をデフォルトとし、
//! 起動引数 `--data-dir <path>` で上書きできる。
//! 設定・キャッシュ・回転DB が全てここを参照する。

use std::path::PathBuf;
use std::sync::OnceLock;

pub static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 起動引数を解析してデータディレクトリを初期化する。
/// `main()` の先頭で一度だけ呼ぶこと。
pub fn init() {
    let dir = parse_from_args();
    DATA_DIR.set(dir).ok();
}

/// データディレクトリを返す。`init()` 未呼び出しの場合はデフォルト値を返す。
pub fn get() -> PathBuf {
    DATA_DIR.get().cloned().unwrap_or_else(default)
}

fn parse_from_args() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == "--data-dir" {
            return PathBuf::from(&args[i + 1]);
        }
    }
    default()
}

fn default() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join("mimageviewer")
}
