//! Per-call pipe ownership: the adapter never opens the application's databases.
use mimageviewer_agent_ipc::*;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
mod transport;
use transport::PipeStream;

#[derive(Default)]
pub struct CallControl {
    pub cancelled: AtomicBool,
    writer: Mutex<Option<InFlight>>,
}
struct InFlight {
    file: PipeStream,
    request_id: u64,
}
/// Close the shared writer on every worker exit, including handshake failure.
/// A delayed cancellation task then sees None and cannot retain this connection.
struct WriterLease(Arc<CallControl>);
impl Drop for WriterLease {
    fn drop(&mut self) {
        if let Ok(mut writer) = self.0.writer.lock() {
            writer.take();
        }
    }
}
impl CallControl {
    /// Never wait for a pipe write on the stdin/event-loop thread (including EOF).
    pub fn cancel(self: &Arc<Self>) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let control = self.clone();
        std::thread::spawn(move || control.write_cancel());
    }
    fn write_cancel(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            if let Some(writer) = writer.as_mut() {
                let _ = write_frame(
                    &mut writer.file,
                    &Request {
                        id: 3,
                        command: Command::Cancel {
                            request_id: writer.request_id,
                        },
                    },
                );
            }
        }
    }
}

pub trait Backend: Send + Sync + 'static {
    fn call(
        &self,
        command: Command,
        client_name: &str,
        control: Arc<CallControl>,
    ) -> Result<Payload, AgentError>;
}
pub struct PipeBackend {
    pipe_name: String,
}
impl PipeBackend {
    #[cfg(windows)]
    pub fn new(data_dir: &Path) -> io::Result<Self> {
        let dir = resolve_data_dir(data_dir)?;
        Ok(Self {
            pipe_name: derive_pipe_name(&dir, &current_user_sid()?)?,
        })
    }
    #[cfg(not(windows))]
    pub fn new(_: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "mIV IPC requires Windows",
        ))
    }
}
pub fn default_data_dir() -> io::Result<PathBuf> {
    // Match the installed core's data_dir::flavor_default without linking its GUI
    // and native dependency graph into this console adapter.
    Ok(PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join("mimageviewer"))
}
fn unavailable() -> AgentError {
    AgentError::new(
        ErrorCode::NotReady,
        "mIV が起動していません。mIV を起動してからもう一度お試しください",
    )
}
fn invalid_response() -> AgentError {
    AgentError::new(ErrorCode::Internal, "mIV からの応答を確認できませんでした")
}
fn connection_error(error: io::Error) -> AgentError {
    match error.raw_os_error() {
        Some(2) => unavailable(), // ERROR_FILE_NOT_FOUND
        Some(231) => AgentError::new(
            ErrorCode::Busy,
            "mIV の AI 連携が混み合っています。少し待ってからもう一度お試しください",
        ),
        Some(5) => AgentError::new(
            ErrorCode::Internal,
            "mIV への接続が拒否されました。mIV と AI アプリを同じユーザーの通常の権限で起動してください",
        ),
        code => {
            // Diagnostics contain the OS code only, never the pipe/data path.
            eprintln!("mimageviewer-mcp: pipe connect failed; os_error={code:?}");
            AgentError::new(ErrorCode::Internal, "mIV に接続できませんでした")
        }
    }
}
fn receive(file: &mut PipeStream, id: u64) -> Result<Payload, AgentError> {
    let response: Response = read_frame(file)
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    if response.id != id {
        return Err(invalid_response());
    }
    response.result
}
impl Backend for PipeBackend {
    fn call(
        &self,
        command: Command,
        client_name: &str,
        control: Arc<CallControl>,
    ) -> Result<Payload, AgentError> {
        let mut reader = PipeStream::connect(&self.pipe_name).map_err(connection_error)?;
        let _lease = WriterLease(control.clone());
        let mut writer = control.writer.lock().map_err(|_| invalid_response())?;
        if control.cancelled.load(Ordering::Acquire) {
            return Err(AgentError::new(ErrorCode::Cancelled, "取り消しました"));
        }
        *writer = Some(InFlight {
            file: reader.clone(),
            request_id: 1,
        });
        write_frame(
            &mut writer.as_mut().unwrap().file,
            &Request {
                id: 1,
                command: Command::Hello {
                    version: AGENT_PROTOCOL_VERSION,
                    client_name: client_name.chars().take(128).collect(),
                },
            },
        )
        .map_err(|_| unavailable())?;
        drop(writer);
        match receive(&mut reader, 1)? {
            Payload::Hello { version } => check_version(version)?,
            _ => return Err(invalid_response()),
        }
        {
            let mut writer = control.writer.lock().map_err(|_| invalid_response())?;
            if control.cancelled.load(Ordering::Acquire) {
                return Err(AgentError::new(ErrorCode::Cancelled, "取り消しました"));
            }
            let writer = writer.as_mut().unwrap();
            writer.request_id = 2;
            write_frame(&mut writer.file, &Request { id: 2, command })
                .map_err(|_| unavailable())?;
        }
        // The only reader owns blocking I/O. Cancellation writes use its cloned handle.
        receive(&mut reader, 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_error_mapping_distinguishes_busy_missing_denied_and_other() {
        let missing = connection_error(io::Error::from_raw_os_error(2));
        assert_eq!(missing.code, ErrorCode::NotReady);
        assert_eq!(
            missing.message,
            "mIV が起動していません。mIV を起動してからもう一度お試しください"
        );
        let busy = connection_error(io::Error::from_raw_os_error(231));
        assert_eq!(busy.code, ErrorCode::Busy);
        assert_eq!(
            busy.message,
            "mIV の AI 連携が混み合っています。少し待ってからもう一度お試しください"
        );
        let denied = connection_error(io::Error::from_raw_os_error(5));
        assert_eq!(denied.code, ErrorCode::Internal);
        assert_eq!(
            denied.message,
            "mIV への接続が拒否されました。mIV と AI アプリを同じユーザーの通常の権限で起動してください"
        );
        for error in [
            io::Error::from_raw_os_error(6),
            io::Error::other("private diagnostic path"),
        ] {
            let error = connection_error(error);
            assert_eq!(error.code, ErrorCode::Internal);
            assert_eq!(error.message, "mIV に接続できませんでした");
        }
    }
    #[test]
    fn cancellation_targets_current_request_and_worker_exit_closes_writer() {
        for request_id in [1, 2] {
            let temp = tempfile::NamedTempFile::new().unwrap();
            let control = Arc::new(CallControl::default());
            let lease = WriterLease(control.clone());
            *control.writer.lock().unwrap() = Some(InFlight {
                file: PipeStream::from_file_for_test(temp.reopen().unwrap()),
                request_id,
            });
            control.write_cancel();
            let frame: Request = read_frame(&mut temp.reopen().unwrap()).unwrap().unwrap();
            assert_eq!(
                frame,
                Request {
                    id: 3,
                    command: Command::Cancel { request_id }
                }
            );
            drop(lease);
            assert!(control.writer.lock().unwrap().is_none());
            control.write_cancel();
            let mut file = temp.reopen().unwrap();
            let _: Request = read_frame(&mut file).unwrap().unwrap();
            assert!(read_frame::<_, Request>(&mut file).unwrap().is_none());
        }
    }
    #[cfg(windows)]
    #[test]
    fn real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld() {
        for cancel_hello in [true, false] {
            real_pipe_fixture(cancel_hello, false);
        }
    }
    #[cfg(windows)]
    #[test]
    fn real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked() {
        real_pipe_fixture(false, true);
    }
    #[cfg(windows)]
    fn real_pipe_fixture(cancel_hello: bool, blocked_stdout: bool) {
        use std::{
            fs::File,
            os::windows::io::{AsRawHandle, FromRawHandle},
            sync::mpsc,
            time::Duration,
        };
        use windows::{
            Win32::{
                Foundation::{ERROR_PIPE_CONNECTED, GetLastError, HANDLE},
                Storage::FileSystem::{
                    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FlushFileBuffers,
                    PIPE_ACCESS_DUPLEX,
                },
                System::Pipes::{
                    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE,
                    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
                },
            },
            core::PCWSTR,
        };
        {
            let temp = tempfile::tempdir().unwrap();
            let name = derive_pipe_name(temp.path(), &current_user_sid().unwrap()).unwrap();
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let handle = unsafe {
                CreateNamedPipeW(
                    PCWSTR(wide.as_ptr()),
                    FILE_FLAGS_AND_ATTRIBUTES(
                        PIPE_ACCESS_DUPLEX.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0,
                    ),
                    NAMED_PIPE_MODE(
                        PIPE_TYPE_BYTE.0
                            | PIPE_READMODE_BYTE.0
                            | PIPE_WAIT.0
                            | PIPE_REJECT_REMOTE_CLIENTS.0,
                    ),
                    1,
                    65536,
                    65536,
                    0,
                    None,
                )
            };
            assert!(!handle.is_invalid());
            let mut server = unsafe { File::from_raw_handle(handle.0) };
            // Diagnose access failures before starting a synchronous fixture
            // listener. A denied client open must not leave ConnectNamedPipe
            // blocked forever or appear only as a later "started" timeout.
            let probe = PipeStream::connect(&name).unwrap_or_else(|error| {
                panic!("named pipe client open failed before fixture threads: raw_os_error={:?}, error={error}", error.raw_os_error());
            });
            drop(probe);
            unsafe { DisconnectNamedPipe(handle) }.unwrap();
            let (started_send, started) = mpsc::channel();
            let (cancel_send, cancel_received) = mpsc::channel();
            let server_thread = std::thread::spawn(move || {
                let handle = HANDLE(server.as_raw_handle());
                if unsafe { ConnectNamedPipe(handle, None) }.is_err() {
                    assert_eq!(unsafe { GetLastError() }, ERROR_PIPE_CONNECTED);
                }
                let hello: Request = read_frame(&mut server).unwrap().unwrap();
                assert!(matches!(hello.command, Command::Hello { .. }));
                let target = if cancel_hello {
                    hello.id
                } else {
                    write_frame(
                        &mut server,
                        &Response {
                            id: hello.id,
                            result: Ok(Payload::Hello {
                                version: AGENT_PROTOCOL_VERSION,
                            }),
                        },
                    )
                    .unwrap();
                    let request: Request = read_frame(&mut server).unwrap().unwrap();
                    assert_eq!(request.command, Command::Status);
                    request.id
                };
                started_send.send(()).unwrap();
                // Deliberately send no response. The adapter's read stays pending
                // until a cancel write reaches this same duplex pipe.
                let cancel: Request = read_frame(&mut server).unwrap().unwrap();
                assert_eq!(cancel.command, Command::Cancel { request_id: target });
                cancel_send.send(()).unwrap();
                write_frame(
                    &mut server,
                    &Response {
                        id: target,
                        result: Err(AgentError::new(ErrorCode::Cancelled, "取り消しました")),
                    },
                )
                .unwrap();
                // The client may already have consumed the response and closed;
                // the worker-result assertion below verifies delivery in either order.
                unsafe {
                    let _ = FlushFileBuffers(handle);
                }
            });
            let backend = PipeBackend { pipe_name: name };
            if blocked_stdout {
                crate::tests::run_pipe_cancel_with_blocked_stdout(
                    Arc::new(backend),
                    started,
                    cancel_received,
                );
                server_thread.join().unwrap();
                return;
            }
            let control = Arc::new(CallControl::default());
            let worker_control = control.clone();
            let (done_send, done) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                done_send
                    .send(backend.call(Command::Status, "fixture", worker_control))
                    .unwrap();
            });
            started
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|error| {
                    panic!(
                        "fixture did not receive its request: {error:?}; backend result={:?}",
                        done.try_recv()
                    );
                });
            control.cancel();
            cancel_received
                .recv_timeout(Duration::from_secs(5))
                .expect("cancel write must reach core before the pending response");
            assert_eq!(
                done.recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap_err()
                    .code,
                ErrorCode::Cancelled
            );
            worker.join().unwrap();
            server_thread.join().unwrap();
            assert!(control.writer.lock().unwrap().is_none());
        }
    }
}
