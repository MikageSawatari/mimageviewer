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

    // portable ビルド: `--data-dir` 未指定で既定の `<exe_dir>/data` が書込不可
    // (read-only メディア / Program Files 等の保護先) の場合は、APPDATA へフォールバック
    // **せず** 明確なエラーで起動を中止する。ポータブルを選ぶユーザーは「APPDATA を触らない
    // 自己完結動作」を期待しており、黙ってフォールバックすると最も嫌われる挙動になるため。
    // `--data-dir <書込可パス>` を明示した場合は read-only メディア上でも起動できる抜け道として
    // チェックしない。詳細: docs/portable-build-plan.md §4.4。
    #[cfg(all(feature = "portable", not(test)))]
    {
        let explicit_data_dir = args.windows(2).any(|w| w[0] == "--data-dir");
        if !explicit_data_dir {
            if let Err(e) = ensure_writable(&dir) {
                portable_fatal_unwritable(&dir, &e);
                std::process::exit(1);
            }
        }
    }

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

/// 軽量な data_dir override 用 RAII ガード (test only)。`settings_db` を触らない
/// テスト (e.g. `archive_cache::cache_zip_path_uses_basename`) から呼ぶ。
///
/// 2026-05-17 事故ガード対応 (`data_dir::default()` の `cfg(test)` panic) を回避するため、
/// `data_dir::get()` を呼ぶ全ての unit test は入り口でこのガードを取って TEST_OVERRIDE
/// を設定すること。Drop で override 解除 + lock 解放。
///
/// `settings_db` の `DataDirOverrideGuard` (= GLOBAL_DB / SAVE_SUPPRESSED もリセット)
/// と違って **TEST_OVERRIDE と lock しか触らない** ので、settings DB を使うテストには
/// 機能不足。settings DB に触るテストは `settings_db` 側のガードを使うこと。
#[cfg(test)]
pub struct TestDataDirGuard {
    _tmp: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl TestDataDirGuard {
    pub fn new() -> Self {
        let lock = test_override_lock();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        set_test_override(Some(tmp.path().to_path_buf()));
        Self {
            _tmp: tmp,
            _lock: lock,
        }
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        self._tmp.path()
    }
}

#[cfg(test)]
impl Drop for TestDataDirGuard {
    fn drop(&mut self) {
        set_test_override(None);
    }
}

/// ログ用サブディレクトリ `<data_dir>/logs` を返す。
pub fn logs_dir() -> PathBuf {
    get().join("logs")
}

fn default() -> PathBuf {
    // 2026-05-17 事故ガード (cfg(test)): テストで `set_test_override(Some(temp))` を
    // 呼び忘れたまま `data_dir::get()` に落ちると、本物の %APPDATA%\mimageviewer に
    // SettingsDb の open/save や `log_diag` の append が流れ込み、ユーザーの本番
    // settings.db / settings.log を汚染する経路に入る (実害: 2026-05-16 夜の
    // cargo test 中に発生)。
    //
    // 主防御は `settings_db::with_db` の data_dir 不一致検知で `SaveSuppressed` に
    // 倒すこと。default() 側はそれと別の二重防御として、**cfg(test) ではプロセス毎の
    // sandbox temp dir を返す**。これで:
    //   - 本物の %APPDATA% を絶対に触らない (= 汚染ゼロ)
    //   - panic で test を片っ端から落とさない (= log_diag のような副次的な
    //     get() 呼び出しを許容)
    //   - sandbox 内で何が書かれてもプロセス終了時に自動消滅
    //
    // 厳格に書き忘れを検出したい場合は、test 側で明示的に `TestDataDirGuard` または
    // `DataDirOverrideGuard` を取って override をセットすること。
    #[cfg(test)]
    {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "mimageviewer-cfg-test-sandbox-{}",
            std::process::id()
        ));
        p
    }
    // portable: 既定の data_dir は exe と同じ場所の `data/` サブディレクトリ。
    // これにより `data_dir::init()` を呼ぶ前に `get()` が呼ばれても (= worker など)
    // APPDATA に逃げず、常にポータブル配下を指す。
    #[cfg(all(not(test), feature = "portable"))]
    {
        flavor_default()
    }
    #[cfg(all(not(test), not(feature = "portable")))]
    {
        flavor_default()
    }
}

/// 現在の build flavor が `--data-dir` 無指定時に使う data directory。
/// single-instance 名前空間は、解決済みの `get()` がこの path と異なる場合だけ分離する。
pub(crate) fn flavor_default() -> PathBuf {
    #[cfg(feature = "portable")]
    {
        crate::native_assets::bundled_root().join("data")
    }
    #[cfg(not(feature = "portable"))]
    {
        let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(appdata).join("mimageviewer")
    }
}

/// portable: data_dir が実際に書き込み可能かを確認する。ACL を推測せず、ディレクトリ作成 +
/// プローブファイルの write/remove を実際に試す (Windows の ACL は事前予測が不確実なため)。
#[cfg(all(feature = "portable", not(test)))]
fn ensure_writable(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("ディレクトリ作成に失敗: {e}"))?;
    let probe = dir.join(".mimv_write_probe.tmp");
    std::fs::write(&probe, b"ok").map_err(|e| format!("書き込みテストに失敗: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// portable: data_dir 書込不可時に、原因と対処を示すネイティブダイアログを出す。
/// `logger::init()` 前に呼ばれ得るので stderr 併用 (ログには頼らない)。
#[cfg(all(feature = "portable", not(test)))]
fn portable_fatal_unwritable(dir: &Path, detail: &str) {
    let msg = format!(
        "ポータブル版はこのフォルダにデータを書き込めません:\n  {}\n\n\
         書き込み可能な場所 (デスクトップ / D ドライブ / USB メモリ等) に\n\
         展開し直してから起動してください。\n\n詳細: {}",
        dir.display(),
        detail
    );
    eprintln!("{msg}");
    show_fatal_messagebox(&msg);
}

#[cfg(all(feature = "portable", not(test), windows))]
fn show_fatal_messagebox(msg: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::PCWSTR;

    let title: Vec<u16> = "mImageViewer (ポータブル版)"
        .encode_utf16()
        .chain([0])
        .collect();
    let body: Vec<u16> = msg.encode_utf16().chain([0]).collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(all(feature = "portable", not(test), not(windows)))]
fn show_fatal_messagebox(_msg: &str) {}

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

/// `bytes` を `path` にアトミックに書き出す。
///
/// 旧実装は `remove_file(path)` → `rename(tmp, path)` だったが、その間 `path` の実体が
/// 物理的に存在しない race window があった (spec §11.1)。Rust の `std::fs::rename` は
/// Windows でも `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` を使うので、既存ファイルへの
/// 上書きは OS レベルでアトミックに行われる。よって `remove_file` は不要。
///
/// `rename` が失敗した場合は tmp ファイルだけ掃除してエラーを返す
/// (= 元の `path` は手付かずで残る)。
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_path = tmp_path_for(path);
    std::fs::write(&tmp_path, bytes)?;
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        // rename 失敗時は中途半端な tmp を残さない。remove 失敗は無視 (元エラーを優先)。
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
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
