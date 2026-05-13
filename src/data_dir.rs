//! データディレクトリの管理。
//!
//! `%APPDATA%\mimageviewer` をデフォルトとし、
//! 起動引数 `--data-dir <path>` で上書きできる。
//! 設定・キャッシュ・回転DB が全てここを参照する。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use sha2::{Digest, Sha256};

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

/// data_dir override を使うテスト群全体を直列化するための共有ロック (Codex P2 v8b
/// 2026-05-14)。`set_test_override` は process-global なので、同時に使う複数の
/// テストモジュールがファイル別の Mutex を持つと cross-file race が起きる。
/// `set_test_override` を使うすべてのテストは入り口で `test_override_lock()` を
/// 取って guard を持ち続けること。poison は無視 (= panic 後も後続テストを進める)。
pub fn test_override_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    let mu = LOCK.get_or_init(|| Mutex::new(()));
    mu.lock().unwrap_or_else(|e| e.into_inner())
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
    let expected_hash = sha256_hex(bytes);
    let hash_path = sidecar_hash_path(path);
    let needs_write = match std::fs::metadata(path) {
        Ok(meta) => {
            meta.len() != bytes.len() as u64
                || !embedded_file_hash_matches(path, &hash_path, &expected_hash)?
        }
        Err(_) => true,
    };
    if needs_write {
        write_atomic(path, bytes)?;
        write_atomic(&hash_path, expected_hash.as_bytes())?;
        crate::logger::log(format!(
            "{} extracted to {} ({} bytes)",
            label,
            path.display(),
            bytes.len()
        ));
    }
    Ok(())
}

fn embedded_file_hash_matches(
    path: &Path,
    hash_path: &Path,
    expected_hash: &str,
) -> std::io::Result<bool> {
    if let Ok(stored) = std::fs::read_to_string(hash_path) {
        return Ok(stored.trim().eq_ignore_ascii_case(expected_hash));
    }

    let actual_hash = sha256_file_hex(path)?;
    if actual_hash.eq_ignore_ascii_case(expected_hash) {
        write_atomic(hash_path, expected_hash.as_bytes())?;
        return Ok(true);
    }
    Ok(false)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_path = tmp_path_for(path);
    std::fs::write(&tmp_path, bytes)?;
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp_path = path.to_path_buf();
    let mut name = tmp_path
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("unknown"));
    name.push(".tmp");
    tmp_path.set_file_name(name);
    tmp_path
}

fn sidecar_hash_path(path: &Path) -> PathBuf {
    let mut hash_path = path.to_path_buf();
    let mut name = hash_path
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("unknown"));
    name.push(".sha256");
    hash_path.set_file_name(name);
    hash_path
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

fn sha256_file_hex(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
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
