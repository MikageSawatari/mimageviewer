use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_REMOTE_PORT: u16 = 8787;
pub const TAILSCALE_COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
const PREFERRED_TAILSCALE_EXE: &str = r"C:\Program Files\Tailscale\tailscale.exe";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailscaleCommandOutput {
    pub stdout: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TailscaleCommandError {
    NotFound,
    ExecutionFailed { stderr: String },
}

impl TailscaleCommandError {
    pub fn user_message(&self) -> String {
        match self {
            Self::NotFound => "Tailscale が見つかりません".to_owned(),
            Self::ExecutionFailed { stderr } if stderr.is_empty() => {
                "Tailscale コマンドを実行できませんでした".to_owned()
            }
            Self::ExecutionFailed { stderr } => {
                format!("Tailscale コマンドを実行できませんでした: {stderr}")
            }
        }
    }
}

pub fn tailscale_executable() -> Option<PathBuf> {
    let preferred = PathBuf::from(PREFERRED_TAILSCALE_EXE);
    if preferred.is_file() {
        Some(preferred)
    } else {
        find_on_path("tailscale.exe").or_else(|| find_on_path("tailscale"))
    }
}

pub fn run_tailscale(arguments: &[&str]) -> Result<TailscaleCommandOutput, TailscaleCommandError> {
    let executable = tailscale_executable().ok_or(TailscaleCommandError::NotFound)?;
    run_tailscale_at(&executable, arguments)
}

pub fn run_tailscale_at(
    executable: &Path,
    arguments: &[&str],
) -> Result<TailscaleCommandOutput, TailscaleCommandError> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| execution_error(error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| execution_error("標準出力を読み取れません"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| execution_error("標準エラー出力を読み取れません"))?;
    let stdout_reader = read_pipe(stdout);
    let stderr_reader = read_pipe(stderr);
    let deadline = Instant::now() + TAILSCALE_COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(execution_error(format!(
                    "{} 秒以内に完了しませんでした",
                    TAILSCALE_COMMAND_TIMEOUT.as_secs()
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(execution_error(error.to_string()));
            }
        }
    };
    let stdout = join_pipe(stdout_reader)?;
    let stderr = String::from_utf8_lossy(&join_pipe(stderr_reader)?)
        .trim()
        .to_owned();
    let status = status?;
    if !status.success() {
        return Err(execution_error(stderr));
    }
    Ok(TailscaleCommandOutput { stdout })
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        let candidate = directory.join(name);
        candidate.is_file().then_some(candidate)
    })
}

fn read_pipe(
    mut pipe: impl Read + Send + 'static,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).map(|_| bytes)
    })
}

fn join_pipe(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, TailscaleCommandError> {
    reader
        .join()
        .map_err(|_| execution_error("コマンド出力の読み取り処理が終了しました"))?
        .map_err(|error| execution_error(error.to_string()))
}

fn execution_error(stderr: impl Into<String>) -> TailscaleCommandError {
    TailscaleCommandError::ExecutionFailed {
        stderr: stderr.into(),
    }
}

#[cfg(windows)]
fn configure_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_process(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_remote_port_is_shared_by_core_and_remote_web() {
        assert_eq!(DEFAULT_REMOTE_PORT, 8787);
    }

    #[test]
    fn command_failures_keep_not_found_distinct_from_stderr() {
        assert_eq!(
            TailscaleCommandError::NotFound.user_message(),
            "Tailscale が見つかりません"
        );
        let error = TailscaleCommandError::ExecutionFailed {
            stderr: "serve failed".to_owned(),
        };
        assert!(error.user_message().contains("serve failed"));
    }
}
