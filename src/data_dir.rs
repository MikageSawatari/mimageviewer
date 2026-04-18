//! データディレクトリの管理。
//!
//! **bootstrap ディレクトリ** (`%APPDATA%\mimageviewer` or `--data-dir`) は
//! settings.json / logs / models / pdfium.dll など「起動時に必要」な資産を置く。
//!
//! **effective ディレクトリ** は DB / キャッシュ / アーカイブ変換など「容量が増え
//! やすい可動データ」を置く。既定では bootstrap と同一だが、settings.json の
//! `data_root` で任意のフォルダに切り替えられる (v0.7.0〜)。
//!
//! 呼び出し規約:
//! - `data_dir::init()` — プロセス起動直後に 1 回。`--data-dir` 引数を解釈。
//! - `data_dir::bootstrap()` — settings / logs / models / pdfium.dll の基点。
//!   settings が未ロードの時点から使える。
//! - `data_dir::get()` — DB / キャッシュの基点。settings ロード後は
//!   `apply_effective()` で切り替えられる。
//! - `data_dir::apply_effective(path)` — settings 読み込み後に呼ぶと、
//!   effective ディレクトリを更新する。`None` なら bootstrap に戻す。

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

static BOOTSTRAP: OnceLock<PathBuf> = OnceLock::new();
static EFFECTIVE: OnceLock<RwLock<PathBuf>> = OnceLock::new();

/// 起動引数を解析して bootstrap / effective を初期化する。
/// `main()` の先頭で一度だけ呼ぶこと。
pub fn init() {
    let cli = parse_cli_data_dir();
    let bootstrap = cli.clone().unwrap_or_else(default_bootstrap);
    let _ = BOOTSTRAP.set(bootstrap.clone());
    EFFECTIVE.get_or_init(|| RwLock::new(bootstrap));
}

/// Bootstrap ディレクトリを返す。settings / logs / models / pdfium が参照する。
pub fn bootstrap() -> PathBuf {
    BOOTSTRAP.get().cloned().unwrap_or_else(default_bootstrap)
}

/// Effective ディレクトリを返す。DB / キャッシュ / アーカイブ変換が参照する。
pub fn get() -> PathBuf {
    match EFFECTIVE.get() {
        Some(lock) => lock.read().map(|p| p.clone()).unwrap_or_else(|_| bootstrap()),
        None => bootstrap(),
    }
}

/// ログ用サブディレクトリ `<bootstrap>/logs` を返す。
/// ロガーは起動直後に初期化されるため、常に bootstrap 側に置く
/// (settings 読み込み前の panic なども記録できるように)。
pub fn logs_dir() -> PathBuf {
    bootstrap().join("logs")
}

/// Settings 読み込み後に effective ディレクトリを設定する。
///
/// `path` が `Some` のとき、フォルダが存在するか検証してから切り替える。
/// 存在しない場合は bootstrap にフォールバックし、警告を返す。
/// `path` が `None` の場合は bootstrap に戻す。
///
/// Returns `Err(msg)` on fallback, otherwise `Ok(())`.
pub fn apply_effective(path: Option<&Path>) -> Result<(), String> {
    let Some(lock) = EFFECTIVE.get() else {
        return Err("data_dir not initialized".to_string());
    };
    match path {
        None => {
            *lock.write().unwrap() = bootstrap();
            Ok(())
        }
        Some(p) => {
            if !p.exists() {
                *lock.write().unwrap() = bootstrap();
                return Err(format!(
                    "データ保存場所 {} が存在しません。既定の場所に戻します。",
                    p.display()
                ));
            }
            if !p.is_dir() {
                *lock.write().unwrap() = bootstrap();
                return Err(format!(
                    "データ保存場所 {} はフォルダではありません。既定の場所に戻します。",
                    p.display()
                ));
            }
            *lock.write().unwrap() = p.to_path_buf();
            Ok(())
        }
    }
}

fn parse_cli_data_dir() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|w| w[0] == "--data-dir")
        .map(|w| PathBuf::from(&w[1]))
}

fn default_bootstrap() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join("mimageviewer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_effective_none_falls_back_to_bootstrap() {
        init();
        let bootstrap = bootstrap();
        // None にすると effective は bootstrap と同じになる
        apply_effective(None).expect("apply_effective(None) should succeed");
        assert_eq!(get(), bootstrap);
    }

    #[test]
    fn apply_effective_rejects_nonexistent_path() {
        init();
        let ghost = std::env::temp_dir().join("mimageviewer-nonexistent-ABCXYZ-12345");
        let r = apply_effective(Some(&ghost));
        assert!(r.is_err());
        // フォールバックで bootstrap を指していること
        assert_eq!(get(), bootstrap());
    }

    #[test]
    fn apply_effective_accepts_existing_dir() {
        init();
        let tmp = std::env::temp_dir();
        apply_effective(Some(&tmp)).expect("temp dir should be valid");
        assert_eq!(get(), tmp);
        // クリーンアップ
        let _ = apply_effective(None);
    }
}
