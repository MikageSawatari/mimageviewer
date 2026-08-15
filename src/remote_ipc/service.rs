use std::io::{BufRead, BufReader};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const REMOTE_EXE_NAME: &str = "mimageviewer-remote.exe";
const MANAGED_BY_CORE_ARG: &str = "--managed-by-core";
const AUTH_FILE_NAME: &str = "remote-web-auth.json";
const LOG_FILE_NAME: &str = "remote-web-log.jsonl";

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteServicePaths {
    data_dir: PathBuf,
    auth_file: PathBuf,
    log_file: PathBuf,
    port: u16,
}

impl RemoteServicePaths {
    fn new(data_dir: PathBuf, log_dir: PathBuf, port: u16) -> Self {
        Self {
            auth_file: data_dir.join(AUTH_FILE_NAME),
            data_dir,
            log_file: log_dir.join(LOG_FILE_NAME),
            port,
        }
    }
}

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
    SetPin {
        pin: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
    RotateSessionSecret {
        reply: mpsc::Sender<Result<(), String>>,
    },
    ConfigureTailscaleServe {
        reply: mpsc::Sender<Result<(), mimageviewer_ipc::TailscaleCommandError>>,
    },
    Shutdown,
}

pub(crate) type RemotePinUpdateReceiver = mpsc::Receiver<Result<(), String>>;
pub(crate) type RemoteSessionSecretRotationReceiver = mpsc::Receiver<Result<(), String>>;
pub(crate) type RemoteTailnetProbeReceiver = mpsc::Receiver<mimageviewer_ipc::TailnetProbe>;
pub(crate) type RemoteTailscaleServeReceiver =
    mpsc::Receiver<Result<(), mimageviewer_ipc::TailscaleCommandError>>;

#[derive(Clone)]
pub(crate) struct RemoteServiceControl {
    tx: mpsc::Sender<RemoteServiceCommand>,
    status: RemoteServiceStatus,
    pin_configured: Arc<AtomicBool>,
    /// 実際に子プロセスへ渡している待受ポート。UI が案内する `tailscale serve` の
    /// コマンドは、既定値ではなくこの値から組み立てる (表示と実行を 1 つの出所にする)。
    port: u16,
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

    pub(crate) fn pin_configured(&self) -> bool {
        self.pin_configured.load(Ordering::Acquire)
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn set_pin(&self, pin: String) -> Result<RemotePinUpdateReceiver, String> {
        mimageviewer_ipc::validate_pin(&pin)?;
        let (reply, receiver) = mpsc::channel();
        self.tx
            .send(RemoteServiceCommand::SetPin { pin, reply })
            .map_err(|_| "PIN の設定を開始できませんでした".to_owned())?;
        Ok(receiver)
    }

    pub(crate) fn rotate_session_secret(
        &self,
    ) -> Result<RemoteSessionSecretRotationReceiver, String> {
        let (reply, receiver) = mpsc::channel();
        self.tx
            .send(RemoteServiceCommand::RotateSessionSecret { reply })
            .map_err(|_| "すべての端末のログアウトを開始できませんでした".to_owned())?;
        Ok(receiver)
    }

    pub(crate) fn probe_tailnet(&self) -> Result<RemoteTailnetProbeReceiver, String> {
        // remote_command は --bind を渡さないため、remote-web もこの loopback address を使う。
        // 実際の service port と同じ出所にしないと、serve の proxy target を誤判定する。
        let address = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), self.port);
        spawn_tailnet_probe_with(address, mimageviewer_ipc::probe_tailnet)
    }

    pub(crate) fn configure_tailscale_serve(&self) -> Result<RemoteTailscaleServeReceiver, String> {
        let (reply, receiver) = mpsc::channel();
        self.tx
            .send(RemoteServiceCommand::ConfigureTailscaleServe { reply })
            .map_err(|_| "tailscale serve の設定を開始できませんでした".to_owned())?;
        Ok(receiver)
    }
}

pub(crate) struct RemoteServiceManager {
    control: RemoteServiceControl,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RemoteServiceManager {
    pub(crate) fn start(
        data_dir: PathBuf,
        log_dir: PathBuf,
        port: u16,
        initially_enabled: bool,
        status: RemoteServiceStatus,
    ) -> Result<Self, String> {
        let paths = RemoteServicePaths::new(data_dir, log_dir, port);
        let pin_configured = Arc::new(AtomicBool::new(auth_file_is_configured(&paths.auth_file)));
        let (tx, rx) = mpsc::channel();
        let worker_status = status.clone();
        let worker_pin_configured = Arc::clone(&pin_configured);
        let thread = std::thread::Builder::new()
            .name("remote-service-owner".to_owned())
            .spawn(move || {
                run_service_manager(
                    rx,
                    paths,
                    initially_enabled,
                    worker_status,
                    worker_pin_configured,
                )
            })
            .map_err(|error| format!("リモート接続の管理を開始できません: {error}"))?;
        Ok(Self {
            control: RemoteServiceControl {
                tx,
                status,
                pin_configured,
                port,
            },
            thread: Some(thread),
        })
    }

    pub(crate) fn control(&self) -> RemoteServiceControl {
        self.control.clone()
    }
}

fn auth_file_is_configured(path: &Path) -> bool {
    mimageviewer_ipc::load_pin_file(path).is_ok()
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
    paths: RemoteServicePaths,
    initially_enabled: bool,
    status: RemoteServiceStatus,
    pin_configured: Arc<AtomicBool>,
) {
    let mut process = None;
    let mut enabled = initially_enabled && pin_configured.load(Ordering::Acquire);
    if enabled {
        start_owned_process(&paths, &status, &mut process);
    } else {
        status.set(RemoteServiceDiagnostic::Stopped);
    }

    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(RemoteServiceCommand::SetEnabled(requested)) => {
                enabled =
                    accepted_enabled_request(requested, pin_configured.load(Ordering::Acquire));
                if enabled {
                    if process.is_none() {
                        start_owned_process(&paths, &status, &mut process);
                    }
                } else {
                    drop(process.take());
                    if requested {
                        status.set(RemoteServiceDiagnostic::Error(
                            "PIN が未設定のためリモート接続を有効にできません".to_owned(),
                        ));
                    } else {
                        status.set(RemoteServiceDiagnostic::Stopped);
                    }
                }
            }
            Ok(RemoteServiceCommand::SetPin { pin, reply }) => {
                let result = mimageviewer_ipc::set_pin_file(&paths.auth_file, &pin);
                if result.is_ok() {
                    pin_configured.store(true, Ordering::Release);
                    if pin_update_plan(enabled) == PinUpdatePlan::Restart {
                        drop(process.take());
                        start_owned_process(&paths, &status, &mut process);
                    }
                }
                let _ = reply.send(result);
            }
            Ok(RemoteServiceCommand::RotateSessionSecret { reply }) => {
                let result = mimageviewer_ipc::rotate_session_secret_file(&paths.auth_file);
                if result.is_ok() && pin_update_plan(enabled) == PinUpdatePlan::Restart {
                    drop(process.take());
                    start_owned_process(&paths, &status, &mut process);
                }
                let _ = reply.send(result);
            }
            Ok(RemoteServiceCommand::ConfigureTailscaleServe { reply }) => {
                let result = configure_tailscale_serve(paths.port);
                if tailscale_serve_completion_plan(enabled, &result)
                    == TailscaleServeCompletionPlan::Restart
                {
                    drop(process.take());
                    start_owned_process(&paths, &status, &mut process);
                }
                let _ = reply.send(result);
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

fn accepted_enabled_request(requested: bool, pin_configured: bool) -> bool {
    requested && pin_configured
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinUpdatePlan {
    KeepStopped,
    Restart,
}

fn pin_update_plan(enabled: bool) -> PinUpdatePlan {
    if enabled {
        PinUpdatePlan::Restart
    } else {
        PinUpdatePlan::KeepStopped
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TailscaleServeCompletionPlan {
    KeepProcess,
    Restart,
}

fn tailscale_serve_completion_plan(
    enabled: bool,
    result: &Result<(), mimageviewer_ipc::TailscaleCommandError>,
) -> TailscaleServeCompletionPlan {
    if enabled && result.is_ok() {
        TailscaleServeCompletionPlan::Restart
    } else {
        TailscaleServeCompletionPlan::KeepProcess
    }
}

fn spawn_tailnet_probe_with(
    address: SocketAddr,
    probe: impl FnOnce(SocketAddr) -> mimageviewer_ipc::TailnetProbe + Send + 'static,
) -> Result<RemoteTailnetProbeReceiver, String> {
    let (reply, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("remote-tailnet-probe".to_owned())
        .spawn(move || {
            let _ = reply.send(probe(address));
        })
        .map_err(|error| format!("Tailscale の状態確認を開始できません: {error}"))?;
    Ok(receiver)
}

fn configure_tailscale_serve(port: u16) -> Result<(), mimageviewer_ipc::TailscaleCommandError> {
    configure_tailscale_serve_with(port, mimageviewer_ipc::run_tailscale)
}

fn configure_tailscale_serve_with(
    port: u16,
    run: impl FnOnce(
        &[&str],
    ) -> Result<
        mimageviewer_ipc::TailscaleCommandOutput,
        mimageviewer_ipc::TailscaleCommandError,
    >,
) -> Result<(), mimageviewer_ipc::TailscaleCommandError> {
    let port = port.to_string();
    run(&["serve", "--bg", &port]).map(|_| ())
}

fn start_owned_process(
    paths: &RemoteServicePaths,
    status: &RemoteServiceStatus,
    process: &mut Option<RemoteServiceProcess>,
) {
    status.set(RemoteServiceDiagnostic::Starting);
    match RemoteServiceProcess::start(paths, status.clone()) {
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
    fn start(paths: &RemoteServicePaths, status: RemoteServiceStatus) -> Result<Self, String> {
        let current_exe = std::env::current_exe()
            .map_err(|error| format!("本体の場所を確認できません: {error}"))?;
        let remote_exe = remote_executable_path(&current_exe)?;
        start_command(paths, status, remote_exe)
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
    paths: &RemoteServicePaths,
    status: RemoteServiceStatus,
    remote_exe: PathBuf,
) -> Result<RemoteServiceProcess, String> {
    if !remote_exe.is_file() {
        return Err(format!(
            "リモート接続に必要な実行ファイルが見つかりません: {}",
            remote_exe.display()
        ));
    }
    let log_dir = paths
        .log_file
        .parent()
        .ok_or("診断ログの保存先を確認できません")?;
    std::fs::create_dir_all(log_dir).map_err(|error| {
        format!(
            "診断ログの保存先を作成できません ({}): {error}",
            log_dir.display()
        )
    })?;
    let command = remote_command(&remote_exe, paths);
    spawn_process(command, remote_exe, status)
}

fn remote_command(remote_exe: &Path, paths: &RemoteServicePaths) -> Command {
    let mut command = Command::new(remote_exe);
    command
        .arg("--data-dir")
        .arg(&paths.data_dir)
        .arg("--auth-file")
        .arg(&paths.auth_file)
        .arg("--log")
        .arg(&paths.log_file)
        .arg("--port")
        .arg(paths.port.to_string())
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
    fn control_only_sends_actions_to_the_owner_thread() {
        let (tx, rx) = mpsc::channel();
        let control = RemoteServiceControl {
            tx,
            status: RemoteServiceStatus::stopped(),
            pin_configured: Arc::new(AtomicBool::new(true)),
            port: mimageviewer_ipc::DEFAULT_REMOTE_PORT,
        };
        control.set_enabled(true);
        assert!(matches!(
            rx.recv().unwrap(),
            RemoteServiceCommand::SetEnabled(true)
        ));
        let _receiver = control.configure_tailscale_serve().unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            RemoteServiceCommand::ConfigureTailscaleServe { .. }
        ));
        let _receiver = control.rotate_session_secret().unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            RemoteServiceCommand::RotateSessionSecret { .. }
        ));
    }

    #[test]
    fn tailnet_probe_runs_on_a_dedicated_thread_with_the_service_address() {
        let (observed_tx, observed_rx) = mpsc::channel();
        let receiver =
            spawn_tailnet_probe_with("127.0.0.1:4242".parse().unwrap(), move |address| {
                observed_tx
                    .send((std::thread::current().name().map(str::to_owned), address))
                    .unwrap();
                mimageviewer_ipc::TailnetProbe {
                    cli_found: true,
                    serve: mimageviewer_ipc::RemoteWebFeatureStatus::Unknown,
                    serve_url: None,
                    serve_conflict: None,
                    serve_unsupported_path: None,
                    status_url: None,
                    https_certificate: mimageviewer_ipc::RemoteWebFeatureStatus::Unknown,
                    key_expiry_unix_seconds: None,
                }
            })
            .unwrap();
        let (thread_name, address) = observed_rx.recv().unwrap();
        assert_eq!(thread_name.as_deref(), Some("remote-tailnet-probe"));
        assert_eq!(address, "127.0.0.1:4242".parse().unwrap());
        assert!(receiver.recv().unwrap().cli_found);
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
        let paths = RemoteServicePaths::new(
            PathBuf::from(r"C:\isolated data"),
            PathBuf::from(r"C:\local\mimageviewer\remote"),
            mimageviewer_ipc::DEFAULT_REMOTE_PORT,
        );
        let command = remote_command(Path::new(r"C:\miv\mimageviewer-remote.exe"), &paths);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--data-dir",
                r"C:\isolated data",
                "--auth-file",
                r"C:\isolated data\remote-web-auth.json",
                "--log",
                r"C:\local\mimageviewer\remote\remote-web-log.jsonl",
                "--port",
                "8787",
                MANAGED_BY_CORE_ARG,
            ]
        );
    }

    #[test]
    fn pin_state_gates_enable_and_an_enabled_pin_update_restarts() {
        assert!(!accepted_enabled_request(true, false));
        assert!(!accepted_enabled_request(false, true));
        assert!(accepted_enabled_request(true, true));
        assert_eq!(pin_update_plan(false), PinUpdatePlan::KeepStopped);
        assert_eq!(pin_update_plan(true), PinUpdatePlan::Restart);
    }

    #[test]
    fn tailscale_serve_runs_on_the_owner_worker_and_restarts_only_after_success() {
        let success = std::thread::Builder::new()
            .name("remote-service-owner".to_owned())
            .spawn(|| {
                configure_tailscale_serve_with(8787, |arguments| {
                    assert_eq!(std::thread::current().name(), Some("remote-service-owner"));
                    assert_eq!(arguments, ["serve", "--bg", "8787"]);
                    Ok(mimageviewer_ipc::TailscaleCommandOutput { stdout: Vec::new() })
                })
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(success, Ok(()));
        assert_eq!(
            tailscale_serve_completion_plan(true, &success),
            TailscaleServeCompletionPlan::Restart
        );

        let failure = Err(mimageviewer_ipc::TailscaleCommandError::ExecutionFailed {
            stderr: "serve failed".to_owned(),
        });
        assert_eq!(
            tailscale_serve_completion_plan(true, &failure),
            TailscaleServeCompletionPlan::KeepProcess
        );
        assert_eq!(
            tailscale_serve_completion_plan(false, &success),
            TailscaleServeCompletionPlan::KeepProcess
        );
    }

    #[test]
    fn core_owns_auth_below_data_and_log_outside_it() {
        let paths = RemoteServicePaths::new(
            PathBuf::from(r"C:\Users\test\AppData\Roaming\mimageviewer"),
            PathBuf::from(r"C:\Users\test\AppData\Local\mimageviewer\remote"),
            mimageviewer_ipc::DEFAULT_REMOTE_PORT,
        );
        assert!(paths.auth_file.starts_with(&paths.data_dir));
        assert!(!paths.log_file.starts_with(&paths.data_dir));
    }

    #[test]
    fn core_treats_missing_and_corrupt_auth_as_unconfigured() {
        let temp = tempfile::tempdir().unwrap();
        let auth_file = temp.path().join(AUTH_FILE_NAME);
        assert!(!auth_file_is_configured(&auth_file));

        std::fs::write(&auth_file, b"not valid JSON").unwrap();
        assert!(!auth_file_is_configured(&auth_file));

        mimageviewer_ipc::set_pin_file(&auth_file, "123456").unwrap();
        assert!(auth_file_is_configured(&auth_file));
    }
}
