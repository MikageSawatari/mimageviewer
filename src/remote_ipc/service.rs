use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const REMOTE_EXE_NAME: &str = "mimageviewer-remote.exe";
const MANAGED_BY_CORE_ARG: &str = "--managed-by-core";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteServiceDiagnostic {
    Stopped,
    Starting,
    VersionMismatch,
    Error(String),
}

fn remote_executable_path(current_exe: &Path) -> Result<PathBuf, String> {
    let directory = current_exe
        .parent()
        .ok_or("本体のディレクトリを確認できません")?;
    Ok(directory.join(REMOTE_EXE_NAME))
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteServiceStatus {
    inner: Arc<Mutex<RemoteServiceDiagnostic>>,
}

impl RemoteServiceStatus {
    pub(crate) fn stopped() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RemoteServiceDiagnostic::Stopped)),
        }
    }

    pub(crate) fn snapshot(&self) -> RemoteServiceDiagnostic {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(crate) fn set_error(&self, error: impl Into<String>) {
        self.set(classify_remote_service_stderr(&error.into()));
    }

    fn set(&self, diagnostic: RemoteServiceDiagnostic) {
        *self.inner.lock().unwrap_or_else(|error| error.into_inner()) = diagnostic;
    }
}

enum RemoteServiceCommand {
    SetEnabled(bool),
    Shutdown,
}

#[derive(Clone)]
pub(crate) struct RemoteServiceControl {
    tx: mpsc::Sender<RemoteServiceCommand>,
    status: RemoteServiceStatus,
}

impl RemoteServiceControl {
    pub(crate) fn set_enabled(&self, enabled: bool) {
        if self
            .tx
            .send(RemoteServiceCommand::SetEnabled(enabled))
            .is_err()
        {
            self.status
                .set_error("リモート接続の切り替えを開始できませんでした");
        }
    }
}

pub(crate) struct RemoteServiceManager {
    control: RemoteServiceControl,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RemoteServiceManager {
    pub(crate) fn start(
        data_dir: PathBuf,
        initially_enabled: bool,
        status: RemoteServiceStatus,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let worker_status = status.clone();
        let thread = std::thread::Builder::new()
            .name("remote-service-owner".to_owned())
            .spawn(move || run_service_manager(rx, data_dir, initially_enabled, worker_status))
            .map_err(|error| format!("リモート接続の管理を開始できません: {error}"))?;
        Ok(Self {
            control: RemoteServiceControl { tx, status },
            thread: Some(thread),
        })
    }

    pub(crate) fn control(&self) -> RemoteServiceControl {
        self.control.clone()
    }
}

impl Drop for RemoteServiceManager {
    fn drop(&mut self) {
        let _ = self.control.tx.send(RemoteServiceCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_service_manager(
    rx: mpsc::Receiver<RemoteServiceCommand>,
    data_dir: PathBuf,
    initially_enabled: bool,
    status: RemoteServiceStatus,
) {
    let mut process = None;
    if initially_enabled {
        start_owned_process(&data_dir, &status, &mut process);
    } else {
        status.set(RemoteServiceDiagnostic::Stopped);
    }

    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(RemoteServiceCommand::SetEnabled(true)) => {
                if process.is_none() {
                    start_owned_process(&data_dir, &status, &mut process);
                }
            }
            Ok(RemoteServiceCommand::SetEnabled(false)) => {
                drop(process.take());
                status.set(RemoteServiceDiagnostic::Stopped);
            }
            Ok(RemoteServiceCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if process
            .as_mut()
            .is_some_and(RemoteServiceProcess::has_exited)
        {
            drop(process.take());
            if status.snapshot() == RemoteServiceDiagnostic::Starting {
                status.set_error("リモート接続が予期せず終了しました");
            }
        }
    }
    drop(process);
}

fn start_owned_process(
    data_dir: &Path,
    status: &RemoteServiceStatus,
    process: &mut Option<RemoteServiceProcess>,
) {
    status.set(RemoteServiceDiagnostic::Starting);
    match RemoteServiceProcess::start(data_dir, status.clone()) {
        Ok(child) => *process = Some(child),
        Err(error) => {
            crate::logger::log(format!("remote_service: startup failed: {error}"));
            status.set_error(error);
        }
    }
}

pub(crate) struct RemoteServiceProcess {
    child: Child,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl RemoteServiceProcess {
    pub(crate) fn start(data_dir: &Path, status: RemoteServiceStatus) -> Result<Self, String> {
        let current_exe = std::env::current_exe()
            .map_err(|error| format!("本体の場所を確認できません: {error}"))?;
        let remote_exe = remote_executable_path(&current_exe)?;
        start_command(data_dir, status, remote_exe)
    }

    fn has_exited(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(error) => {
                crate::logger::log(format!("remote_service: status check failed: {error}"));
                true
            }
        }
    }
}

fn start_command(
    data_dir: &Path,
    status: RemoteServiceStatus,
    remote_exe: PathBuf,
) -> Result<RemoteServiceProcess, String> {
    if !remote_exe.is_file() {
        return Err(format!(
            "リモート接続に必要な実行ファイルが見つかりません: {}",
            remote_exe.display()
        ));
    }
    let command = remote_command(&remote_exe, data_dir);
    spawn_process(command, remote_exe, status)
}

fn remote_command(remote_exe: &Path, data_dir: &Path) -> Command {
    let mut command = Command::new(remote_exe);
    command
        .arg("--data-dir")
        .arg(data_dir)
        .arg(MANAGED_BY_CORE_ARG)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    configure_process(&mut command);
    command
}

#[cfg(windows)]
fn configure_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_process(_command: &mut Command) {}

fn spawn_process(
    mut command: Command,
    remote_exe: PathBuf,
    status: RemoteServiceStatus,
) -> Result<RemoteServiceProcess, String> {
    let mut child = command.spawn().map_err(|error| {
        format!(
            "リモート接続を開始できません ({}): {error}",
            remote_exe.display()
        )
    })?;
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("リモート接続の状態を確認できません".to_owned());
    };
    let stderr_thread = match std::thread::Builder::new()
        .name("remote-service-stderr".to_owned())
        .spawn(move || monitor_stderr(stderr, status))
    {
        Ok(thread) => thread,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("リモート接続の状態を確認できません: {error}"));
        }
    };
    Ok(RemoteServiceProcess {
        child,
        stderr_thread: Some(stderr_thread),
    })
}

fn monitor_stderr(stderr: impl std::io::Read, status: RemoteServiceStatus) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if !line.is_empty() {
            crate::logger::log(format!("remote_service: {line}"));
            status.set_error(line.to_owned());
        }
    }
}

impl Drop for RemoteServiceProcess {
    fn drop(&mut self) {
        // core 自身が spawn した Child だけを終了し、外部プロセスは所有しない。
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}

fn classify_remote_service_stderr(line: &str) -> RemoteServiceDiagnostic {
    if line.contains("プロトコル版が一致しません")
        || line.contains("不明な引数です: --managed-by-core")
    {
        RemoteServiceDiagnostic::VersionMismatch
    } else if line.contains("IPC へ接続できません") {
        RemoteServiceDiagnostic::Starting
    } else if line.contains("HTTP bind に失敗しました") {
        RemoteServiceDiagnostic::Error(
            "接続の受け付けを開始できません。別のリモート接続が有効になっていないか確認してください。"
                .to_owned(),
        )
    } else {
        RemoteServiceDiagnostic::Error(
            "リモート接続を開始できません。詳しくはログを確認してください。".to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_executable_is_resolved_next_to_core() {
        assert_eq!(
            remote_executable_path(Path::new(r"C:\miv\mimageviewer-core.exe")).unwrap(),
            PathBuf::from(r"C:\miv\mimageviewer-remote.exe")
        );
    }

    #[test]
    fn control_only_sends_desired_state_to_the_owner_thread() {
        let (tx, rx) = mpsc::channel();
        let control = RemoteServiceControl {
            tx,
            status: RemoteServiceStatus::stopped(),
        };
        control.set_enabled(true);
        assert!(matches!(
            rx.recv().unwrap(),
            RemoteServiceCommand::SetEnabled(true)
        ));
    }

    #[test]
    fn protocol_mismatch_is_a_typed_diagnostic() {
        assert_eq!(
            classify_remote_service_stderr(
                "remote-web: IPC プロトコル版が一致しません (remote-web=24, mIV=25)"
            ),
            RemoteServiceDiagnostic::VersionMismatch
        );
        assert_eq!(
            classify_remote_service_stderr("不明な引数です: --managed-by-core"),
            RemoteServiceDiagnostic::VersionMismatch
        );
    }

    #[test]
    fn unknown_stderr_is_not_exposed_as_internal_ui_text() {
        assert_eq!(
            classify_remote_service_stderr("IPC worker handshake failed"),
            RemoteServiceDiagnostic::Error(
                "リモート接続を開始できません。詳しくはログを確認してください。".to_owned()
            )
        );
    }

    #[test]
    fn managed_remote_receives_the_core_data_directory() {
        let command = remote_command(
            Path::new(r"C:\miv\mimageviewer-remote.exe"),
            Path::new(r"C:\isolated data"),
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--data-dir", r"C:\isolated data", MANAGED_BY_CORE_ARG]
        );
    }
}
