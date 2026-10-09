/// シンプルなファイルロガー（パフォーマンス分析用）
///
/// ログは mimageviewer.log に出力される。
/// 書式: [経過秒数][スレッドID] メッセージ
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static CONFIGURED: OnceLock<()> = OnceLock::new();

#[cfg(test)]
pub(crate) fn install_test_clock(start: Instant) {
    assert!(START.set(start).is_ok());
}
const DEFAULT_MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
const DEBUG_MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_ROTATED_GENERATIONS: usize = 1;
const DEBUG_ROTATED_GENERATIONS: usize = 4;
const DEBUG_RETENTION_ENV_VARS: &[&str] = &[
    "MIV_CURSOR_DEBUG",
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

pub(crate) struct LogFile {
    pub(crate) path: PathBuf,
    file: Option<std::fs::File>,
    bytes_written: u64,
    retention: LogRetention,
}

impl LogFile {
    #[cfg(test)]
    pub(crate) fn needs_rotation(&self, incoming: usize) -> bool {
        self.bytes_written.saturating_add(incoming as u64) > self.retention.max_bytes
    }
    #[cfg(test)]
    pub(crate) fn test_limit(&mut self, bytes: u64) {
        self.retention.max_bytes = bytes;
    }
    #[cfg(test)]
    pub(crate) fn set_test_retention(&mut self, bytes: u64, generations: usize) {
        self.retention.max_bytes = bytes;
        self.retention.rotated_generations = generations;
    }
    fn rotate_if_needed(&mut self, incoming_bytes: usize) -> std::io::Result<()> {
        if self.bytes_written + incoming_bytes as u64 <= self.retention.max_bytes {
            return Ok(());
        }
        if let Some(mut f) = self.file.take() {
            f.flush()?;
        }
        rotate_log_files(&self.path, self.retention.rotated_generations)?;
        match open_log_path(&self.path, true) {
            Ok(f) => {
                self.file = Some(f);
                self.bytes_written = 0;
                Ok(())
            }
            Err(error) => {
                self.file = None;
                Err(error)
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
    if CONFIGURED.set(()).is_err() {
        return;
    }
    crate::log_writer::configure_normal(crate::log_writer::NormalConfig {
        path: crate::data_dir::logs_dir().join(file_name),
        truncate,
    });
    log(LogRetention::from_environment().description());
}

impl LogFile {
    pub(crate) fn open(path: &std::path::Path, truncate: bool) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if truncate && path.exists() {
            let mut name = path.as_os_str().to_os_string();
            name.push(".prev");
            let _ = std::fs::copy(path, PathBuf::from(name));
        }
        let file = open_log_path(path, truncate)?;
        let bytes_written = if truncate { 0 } else { file.metadata()?.len() };
        Ok(Self {
            path: path.to_owned(),
            file: Some(file),
            bytes_written,
            retention: LogRetention::from_environment(),
        })
    }
    pub(crate) fn write(&mut self, line: &[u8]) -> std::io::Result<()> {
        self.rotate_if_needed(line.len())?;
        self.file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("rotation failed"))?
            .write_all(line)?;
        self.bytes_written += line.len() as u64;
        Ok(())
    }
    pub(crate) fn flush(&mut self) -> std::io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("log unavailable"))?
            .flush()
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

fn rotate_log_files(path: &std::path::Path, generations: usize) -> std::io::Result<()> {
    if generations == 0 {
        return Ok(());
    }
    if generations == 1 {
        let backup_path = path.with_extension("log.bak");
        remove_old_generation(&backup_path)?;
        if path.exists() {
            std::fs::rename(path, backup_path)?;
        }
        return Ok(());
    }

    let oldest = rotated_log_path(path, generations);
    remove_old_generation(&oldest)?;
    for generation in (1..generations).rev() {
        let from = rotated_log_path(path, generation);
        let to = rotated_log_path(path, generation + 1);
        if from.exists() {
            std::fs::rename(from, to)?;
        }
    }
    if path.exists() {
        std::fs::rename(path, rotated_log_path(path, 1))?;
    }
    Ok(())
}

pub(crate) fn remove_old_generation(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
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

pub(crate) fn allow_live_log_reading(opts: &mut std::fs::OpenOptions) {
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

#[track_caller]
pub fn log(msg: impl AsRef<str>) {
    let Some(publication) = crate::log_writer::publication(crate::log_writer::Sink::Normal) else {
        return;
    };
    let elapsed = START
        .get()
        .map(|s| s.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    let tid = current_thread_id_num().unwrap_or(0);
    let tid_text = if tid == 0 {
        "?".to_owned()
    } else {
        tid.to_string()
    };
    publication.submit(
        format!("[{elapsed:>8.3}s][t{tid_text:>3}] {}\n", msg.as_ref()),
        elapsed,
        tid,
        std::panic::Location::caller(),
    );
}

/// Returns the same process-relative clock used by [`log`], at microsecond resolution.
/// Transition probes include this value because the ordinary log prefix is rounded to millis.
pub(crate) fn elapsed_micros() -> u64 {
    START
        .get()
        .map(|s| s.elapsed().as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Request delivery without waiting. Callers that need durable completion use
/// the worker-only completion barrier or the final shutdown helper.
#[track_caller]
pub fn flush() {
    crate::log_writer::request_flush();
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

    // Isolate the facade's OnceLocks from every other test in this binary.
    #[test]
    fn stalled_sink_does_not_block_facade_producer() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "logger::tests::stalled_sink_facade_producer_child",
                "--ignored",
                "--nocapture",
            ])
            .env("MIV_C1356_LOGGER_TEST_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated producer test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "isolated helper invoked by stalled_sink_does_not_block_facade_producer"]
    fn stalled_sink_facade_producer_child() {
        assert_eq!(std::env::var("MIV_C1356_LOGGER_TEST_CHILD").unwrap(), "1");
        let directory = tempfile::tempdir().unwrap();
        let writer = crate::log_writer::test_support::writer(directory.path(), 32, 65536);
        crate::log_writer::test_support::install_global(&writer);
        let stalled_sink = writer.pause_write();
        log("reach stalled write");
        stalled_sink.entered();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            log("facade producer while sink is stalled");
            crate::perf::event(
                "test",
                "facade",
                None,
                17,
                &[("kind", serde_json::json!("cannot_override_reserved_field"))],
            );
            flush();
            crate::perf::flush();
            finished_tx.send(()).unwrap();
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let completed_while_stalled = finished_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_ok();
        drop(stalled_sink);
        producer.join().unwrap();
        assert!(writer.shutdown());
        assert!(
            completed_while_stalled,
            "logger::log could not finish while the sink ownership guard was held"
        );
    }

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
            rotate_log_files(&log_path, DEBUG_ROTATED_GENERATIONS).unwrap();
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

        rotate_log_files(&log_path, DEFAULT_ROTATED_GENERATIONS).unwrap();

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
