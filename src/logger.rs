/// シンプルなファイルロガー（パフォーマンス分析用）
///
/// ログは mimageviewer.log に出力される。
/// 書式: [経過秒数][スレッドID] メッセージ
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static FILE: OnceLock<Mutex<LogFile>> = OnceLock::new();
const DEFAULT_MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
const DEBUG_MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_ROTATED_GENERATIONS: usize = 1;
const DEBUG_ROTATED_GENERATIONS: usize = 4;
const DEBUG_RETENTION_ENV_VARS: &[&str] = &[
    "MIV_DETACHED_WINDOW_DEBUG",
    "MIV_DETAILS_LAYOUT_DEBUG",
    "MIV_LOG_RETENTION_DEBUG",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LogRetention {
    max_bytes: u64,
    rotated_generations: usize,
    debug_env: Option<&'static str>,
}

impl LogRetention {
    fn from_environment() -> Self {
        let debug_env = DEBUG_RETENTION_ENV_VARS
            .iter()
            .copied()
            .find(|name| std::env::var(name).ok().as_deref() == Some("1"));
        if let Some(debug_env) = debug_env {
            Self {
                max_bytes: DEBUG_MAX_LOG_BYTES,
                rotated_generations: DEBUG_ROTATED_GENERATIONS,
                debug_env: Some(debug_env),
            }
        } else {
            Self {
                max_bytes: DEFAULT_MAX_LOG_BYTES,
                rotated_generations: DEFAULT_ROTATED_GENERATIONS,
                debug_env: None,
            }
        }
    }

    fn description(self) -> String {
        if let Some(env) = self.debug_env {
            format!(
                "logger: debug retention mode ({}MB x{} generations) via {env}",
                self.max_bytes / 1024 / 1024,
                self.rotated_generations
            )
        } else {
            format!(
                "logger: normal retention mode ({}MB x{} generation)",
                self.max_bytes / 1024 / 1024,
                self.rotated_generations
            )
        }
    }
}

struct LogFile {
    path: PathBuf,
    file: Option<std::fs::File>,
    bytes_written: u64,
    retention: LogRetention,
}

impl LogFile {
    fn rotate_if_needed(&mut self, incoming_bytes: usize) {
        if self.bytes_written + incoming_bytes as u64 <= self.retention.max_bytes {
            return;
        }
        if let Some(mut f) = self.file.take() {
            let _ = f.flush();
        }
        rotate_log_files(&self.path, self.retention.rotated_generations);
        match open_log_path(&self.path, true) {
            Ok(f) => {
                self.file = Some(f);
                self.bytes_written = 0;
            }
            Err(_) => {
                self.file = None;
            }
        }
    }
}

/// メインプロセス用 logger 初期化。`mimageviewer.log` を truncate して書き始める。
pub fn init() {
    init_inner("mimageviewer.log", /* truncate = */ true);
}

/// ワーカープロセス用 logger 初期化。**メインの mimageviewer.log を truncate せず**、
/// ワーカー専用ファイル `<worker_kind>.log` に append する。
///
/// 以前は worker も `init()` を呼んでいたため、parent が書いた直近の mimageviewer.log
/// を truncate で消し飛ばしてしまい、起動失敗時のデバッグ情報が消えていた
/// (Codex P3 指摘 / Apr 28-29 の trt_worker 死亡解析でも実害)。
///
/// `worker_kind` は識別用の短い名前 (例: "trt-worker", "pdf-worker")。複数のワーカーが
/// 並走しても別ファイルに書き分けられる。
pub fn init_for_worker(worker_kind: &str) {
    init_inner(&format!("{worker_kind}.log"), /* truncate = */ false);
}

fn init_inner(file_name: &str, truncate: bool) {
    START.set(Instant::now()).ok();
    let retention = LogRetention::from_environment();
    let log_dir = crate::data_dir::logs_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(file_name);
    // truncate モード (= メインプロセス) の場合、**前回セッションのログを `.prev` に**
    // 退避してから truncate する。クラッシュ再現で「次の起動」をする前に
    // `<log>.prev` を見れば直前の死亡時の最終出力が観察できる。
    // 退避は best-effort (失敗しても通常起動を続ける)。
    if truncate && log_path.exists() {
        let mut prev_path = log_path.clone();
        let mut prev_name = log_path
            .file_name()
            .map(|n| n.to_owned())
            .unwrap_or_else(|| std::ffi::OsString::from(file_name));
        prev_name.push(".prev");
        prev_path.set_file_name(prev_name);
        let _ = std::fs::copy(&log_path, &prev_path);
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true);
    allow_live_log_reading(&mut opts);
    if truncate {
        opts.truncate(true);
    } else {
        opts.append(true);
    }
    match opts.open(&log_path) {
        Ok(f) => {
            let bytes_written = if truncate {
                0
            } else {
                f.metadata().map(|m| m.len()).unwrap_or(0)
            };
            FILE.set(Mutex::new(LogFile {
                path: log_path,
                file: Some(f),
                bytes_written,
                retention,
            }))
            .ok();
            log(retention.description());
        }
        Err(e) => eprintln!("ログファイル作成失敗: {e} (path: {})", log_path.display()),
    }
}

fn rotated_log_path(path: &std::path::Path, generation: usize) -> PathBuf {
    let mut rotated = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("mimageviewer.log"));
    name.push(format!(".bak{generation}"));
    rotated.set_file_name(name);
    rotated
}

fn rotate_log_files(path: &std::path::Path, generations: usize) {
    if generations == 0 {
        return;
    }
    if generations == 1 {
        let backup_path = path.with_extension("log.bak");
        let _ = std::fs::remove_file(&backup_path);
        if path.exists() {
            let _ = std::fs::rename(path, backup_path);
        }
        return;
    }

    let oldest = rotated_log_path(path, generations);
    let _ = std::fs::remove_file(oldest);
    for generation in (1..generations).rev() {
        let from = rotated_log_path(path, generation);
        let to = rotated_log_path(path, generation + 1);
        if from.exists() {
            let _ = std::fs::rename(from, to);
        }
    }
    if path.exists() {
        let _ = std::fs::rename(path, rotated_log_path(path, 1));
    }
}

fn open_log_path(path: &std::path::Path, truncate: bool) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true);
    allow_live_log_reading(&mut opts);
    if truncate {
        opts.truncate(true);
    } else {
        opts.append(true);
    }
    opts.open(path)
}

fn allow_live_log_reading(opts: &mut std::fs::OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;

        opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    #[cfg(not(windows))]
    let _ = opts;
}

pub fn log(msg: impl AsRef<str>) {
    let elapsed = START
        .get()
        .map(|s| s.elapsed().as_secs_f64())
        .unwrap_or(0.0);

    let tid_num = current_thread_id_num()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".to_owned());

    if let Some(file) = FILE.get() {
        if let Ok(mut log_file) = file.lock() {
            let line = format!("[{elapsed:>8.3}s][t{tid_num:>3}] {}\n", msg.as_ref());
            log_file.rotate_if_needed(line.len());
            let wrote = if let Some(f) = log_file.file.as_mut() {
                let ok = f.write_all(line.as_bytes()).is_ok();
                if ok {
                    let _ = f.flush();
                }
                ok
            } else {
                false
            };
            if wrote {
                log_file.bytes_written += line.len() as u64;
            }
        }
    }
}

/// Buffered log data is normally flushed by [`log`] after every line. Keep an
/// explicit flush operation for shutdown paths that may terminate the process
/// without running Rust destructors.
pub fn flush() {
    if let Some(file) = FILE.get()
        && let Ok(mut log_file) = file.lock()
        && let Some(f) = log_file.file.as_mut()
    {
        let _ = f.flush();
    }
}

/// ログ行に添える現在スレッドの識別子。`logger` と `perf` の両方から参照される。
///
/// Windows では **OS の TID を使い、`std::thread::current()` を経由しない**。
/// この関数は native 例外ハンドラからも呼ばれ、そのハンドラは
/// **スレッド終了処理 (`LdrShutdownThread`) の内側でも走る**。そこでは TLS が既に
/// 破棄されており、`std::thread::current()` は thread-local を読んで落ちる。
/// 実際に `RtlFreeHeap` の一次例外を受けたハンドラがここで二次 AV を起こし、
/// **panic.log に 1 行も残らないまま一次例外の情報を失った** (backlog §1.123)。
/// `GetCurrentThreadId` は syscall だけで TLS に触れず、値はダンプや `cdb` の
/// スレッド表示とも一致するので、事後解析にもそのまま使える。
pub fn current_thread_id_num() -> Option<u64> {
    #[cfg(windows)]
    {
        Some(u64::from(unsafe {
            windows::Win32::System::Threading::GetCurrentThreadId()
        }))
    }
    #[cfg(not(windows))]
    {
        let tid = format!("{:?}", std::thread::current().id());
        tid.trim_start_matches("ThreadId(")
            .trim_end_matches(')')
            .parse::<u64>()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "miv_logger_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn debug_rotation_keeps_four_generations_with_latest_at_bak1() {
        let dir = unique_test_dir("debug_rotation");
        let log_path = dir.join("mimageviewer.log");

        for generation in 1..=5 {
            std::fs::write(&log_path, format!("current-{generation}")).unwrap();
            rotate_log_files(&log_path, DEBUG_ROTATED_GENERATIONS);
        }

        assert_eq!(
            std::fs::read_to_string(rotated_log_path(&log_path, 1)).unwrap(),
            "current-5",
            "bak1 must be the latest rotated generation"
        );
        assert_eq!(
            std::fs::read_to_string(rotated_log_path(&log_path, 2)).unwrap(),
            "current-4"
        );
        assert_eq!(
            std::fs::read_to_string(rotated_log_path(&log_path, 3)).unwrap(),
            "current-3"
        );
        assert_eq!(
            std::fs::read_to_string(rotated_log_path(&log_path, 4)).unwrap(),
            "current-2",
            "the fifth rotation must discard the oldest generation"
        );
        assert!(
            !log_path.exists(),
            "rotation only moves the full file; the caller opens a fresh log afterwards"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn normal_rotation_keeps_legacy_single_bak_name() {
        let dir = unique_test_dir("normal_rotation");
        let log_path = dir.join("mimageviewer.log");
        std::fs::write(&log_path, "normal-current").unwrap();

        rotate_log_files(&log_path, DEFAULT_ROTATED_GENERATIONS);

        assert_eq!(
            std::fs::read_to_string(log_path.with_extension("log.bak")).unwrap(),
            "normal-current"
        );
        assert!(
            !rotated_log_path(&log_path, 1).exists(),
            "normal mode must keep the historical .log.bak name instead of .bak1"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
