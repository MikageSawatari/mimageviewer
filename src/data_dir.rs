//! データディレクトリの管理。
//!
//! `%APPDATA%\mimageviewer` をデフォルトとし、
//! 起動引数 `--data-dir <path>` で上書きできる。
//! 設定・キャッシュ・回転DB が全てここを参照する。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// テスト用の data_dir 上書き。`None` なら本番の OnceLock を参照する。
///
/// Phase C で `App::new_for_test()` が TempDir を割り当てるために使う。
/// プロセス全体で 1 箇所のグローバル状態 (`OnceLock` は再代入不可 + App の
/// 子スレッドが同じ dir を参照する必要があるため thread-local 化できない)
/// なので、Phase C テストは `#[serial]` 相当で同時 1 本に絞って動かす前提。
static TEST_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 起動引数を解析してデータディレクトリを初期化する。
/// `main()` の先頭で一度だけ呼ぶこと。
pub fn init() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .windows(2)
        .find(|w| w[0] == "--data-dir")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(default);
    DATA_DIR.set(dir).ok();
}

/// データディレクトリを返す。
///
/// 優先順位: `TEST_OVERRIDE` (テスト時) → `DATA_DIR` (本番 OnceLock) →
/// `default()` (%APPDATA% + mimageviewer)。
pub fn get() -> PathBuf {
    if let Ok(guard) = TEST_OVERRIDE.lock() {
        if let Some(p) = guard.as_ref() {
            return p.clone();
        }
    }
    DATA_DIR.get().cloned().unwrap_or_else(default)
}

/// テスト用: data_dir をテンポラリディレクトリに差し替える (None で解除)。
///
/// Phase C の `App::new_for_test` から呼ぶ。`TempDir` の `close()` が先に走ると
/// テスト中の supervisor スレッドが生き残って disk を触りに行くので、呼び出し側は
/// `TempDir` を App より長生きさせること。
pub fn set_test_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = TEST_OVERRIDE.lock() {
        *guard = path;
    }
}

/// ログ用サブディレクトリ `<data_dir>/logs` を返す。
pub fn logs_dir() -> PathBuf {
    get().join("logs")
}

fn default() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join("mimageviewer")
}

/// `include_bytes!` で埋め込んだファイルを指定パスに展開する。
///
/// サイズ一致チェックで、既に同じ長さのファイルが存在すれば書き込みをスキップする
/// (例: アプリ再起動時の無駄な書き戻し抑制)。新バージョンの exe でバイトサイズが
/// 変われば自動的に更新される。書き込みが発生したら `logger::log` に記録する。
///
/// 同サイズ別内容の破損ファイルには気付けないが、これは PDFium / ONNX Runtime DLL 等
/// 公式配布ファイルを埋め込んでいる前提で許容する (Susie ワーカーのみ別途内容比較)。
pub fn extract_embedded_file(path: &Path, bytes: &[u8], label: &str) -> std::io::Result<()> {
    let needs_write = match std::fs::metadata(path) {
        Ok(meta) => meta.len() != bytes.len() as u64,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(path, bytes)?;
        crate::logger::log(format!(
            "{} extracted to {} ({} bytes)",
            label,
            path.display(),
            bytes.len()
        ));
    }
    Ok(())
}
