use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mimageviewer_ipc::{
    ClientHello, MAX_CONTROL_FRAME_BYTES, PIPE_NAME, PROTOCOL_VERSION, ThumbnailError,
    ThumbnailErrorCode, ThumbnailRequest, ThumbnailResponse, negotiate, read_frame, write_frame,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows::core::PCWSTR;

use super::thumbnail::{ThumbnailEngine, WorkerContext};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const PIPE_MAX_INSTANCES: u32 = 16;
const WORK_QUEUE_CAPACITY: usize = 256;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

enum Work {
    Request {
        request: ThumbnailRequest,
        reply: mpsc::SyncSender<ThumbnailResponse>,
    },
    Stop,
}

pub(super) struct ServerGuard {
    stop: Arc<AtomicBool>,
    listener: Option<std::thread::JoinHandle<()>>,
    workers: Vec<std::thread::JoinHandle<()>>,
    work_tx: mpsc::SyncSender<Work>,
}

impl ServerGuard {
    pub(super) fn start(settings: crate::settings::Settings) -> Result<Self, String> {
        let worker_count = settings.parallelism.thread_count().clamp(1, 8);
        let engine = Arc::new(ThumbnailEngine::new(settings));
        let (work_tx, work_rx) = mpsc::sync_channel::<Work>(WORK_QUEUE_CAPACITY);
        let work_rx = Arc::new(Mutex::new(work_rx));
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let work_rx = Arc::clone(&work_rx);
            let engine = Arc::clone(&engine);
            workers.push(
                std::thread::Builder::new()
                    .name(format!("remote-thumb-{index}"))
                    .spawn(move || worker_loop(&work_rx, &engine))
                    .map_err(|error| format!("remote IPC worker を開始できません: {error}"))?,
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let listener_stop = Arc::clone(&stop);
        let listener_tx = work_tx.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let listener = std::thread::Builder::new()
            .name("remote-ipc-listener".to_owned())
            .spawn(move || listener_loop(listener_stop, listener_tx, ready_tx))
            .map_err(|error| format!("remote IPC listener を開始できません: {error}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                crate::logger::log(format!(
                    "remote_ipc: listening pipe={PIPE_NAME} protocol={PROTOCOL_VERSION} workers={worker_count}"
                ));
                Ok(Self {
                    stop,
                    listener: Some(listener),
                    workers,
                    work_tx,
                })
            }
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                for _ in 0..worker_count {
                    let _ = work_tx.send(Work::Stop);
                }
                for worker in workers {
                    let _ = worker.join();
                }
                let _ = listener.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                poke_listener();
                let _ = listener.join();
                for _ in 0..worker_count {
                    let _ = work_tx.send(Work::Stop);
                }
                for worker in workers {
                    let _ = worker.join();
                }
                Err("remote IPC listener の起動確認がタイムアウトしました".to_owned())
            }
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        poke_listener();
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        for _ in 0..self.workers.len() {
            let _ = self.work_tx.send(Work::Stop);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        crate::logger::log("remote_ipc: stopped".to_owned());
    }
}

fn worker_loop(work_rx: &Mutex<mpsc::Receiver<Work>>, engine: &ThumbnailEngine) {
    let context = WorkerContext::open();
    loop {
        let work = {
            let receiver = work_rx.lock().unwrap_or_else(|error| error.into_inner());
            receiver.recv()
        };
        match work {
            Ok(Work::Request { request, reply }) => {
                let response = engine.handle(request, &context);
                let _ = reply.send(response);
            }
            Ok(Work::Stop) | Err(_) => break,
        }
    }
}

fn listener_loop(
    stop: Arc<AtomicBool>,
    work_tx: mpsc::SyncSender<Work>,
    ready: mpsc::SyncSender<Result<(), String>>,
) {
    let first = match create_server_pipe(true) {
        Ok(pipe) => pipe,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "remote IPC pipe を作成できません。同名サーバが既に存在する可能性があります: {error}"
            )));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut next = Some(first);
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let pipe = match next.take() {
            Some(pipe) => pipe,
            None => match create_server_pipe(false) {
                Ok(pipe) => pipe,
                Err(error) => {
                    crate::logger::log(format!("remote_ipc: CreateNamedPipeW failed: {error}"));
                    break;
                }
            },
        };
        let connected = unsafe {
            ConnectNamedPipe(pipe.handle, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED
        };
        if !connected {
            continue;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        let work_tx = work_tx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("remote-ipc-connection".to_owned())
            .spawn(move || handle_connection(pipe, work_tx))
        {
            crate::logger::log(format!(
                "remote_ipc: connection thread spawn failed: {error}"
            ));
        }
    }
    crate::logger::log("remote_ipc: listener exiting".to_owned());
}

fn handle_connection(mut pipe: PipeStream, work_tx: mpsc::SyncSender<Work>) {
    let hello: ClientHello = match read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES) {
        Ok(hello) => hello,
        Err(error) => {
            crate::logger::log(format!("remote_ipc: handshake read failed: {error}"));
            return;
        }
    };
    let response = negotiate(hello.protocol_version);
    if let Err(error) = write_frame(&mut pipe, &response) {
        crate::logger::log(format!("remote_ipc: handshake write failed: {error}"));
        return;
    }
    if !response.accepted {
        crate::logger::log(format!(
            "remote_ipc: protocol mismatch rejected client={} server={}",
            hello.protocol_version, response.protocol_version
        ));
        return;
    }
    let request: ThumbnailRequest = match read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES) {
        Ok(request) => request,
        Err(mimageviewer_ipc::FrameError::Io(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            // 起動時 probe はハンドシェイクだけで切断する。
            return;
        }
        Err(error) => {
            crate::logger::log(format!("remote_ipc: request read failed: {error}"));
            return;
        }
    };
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let response = match work_tx.try_send(Work::Request {
        request,
        reply: reply_tx,
    }) {
        Ok(()) => reply_rx.recv_timeout(RESPONSE_TIMEOUT).unwrap_or_else(|_| {
            ThumbnailResponse::Error(ThumbnailError::new(
                ThumbnailErrorCode::Internal,
                "mIV 本体のサムネイル処理がタイムアウトしました",
            ))
        }),
        Err(mpsc::TrySendError::Full(_)) => ThumbnailResponse::Error(ThumbnailError::new(
            ThumbnailErrorCode::Busy,
            "mIV 本体のサムネイルキューが混雑しています",
        )),
        Err(mpsc::TrySendError::Disconnected(_)) => ThumbnailResponse::Error(ThumbnailError::new(
            ThumbnailErrorCode::Internal,
            "mIV 本体のサムネイルワーカーが停止しています",
        )),
    };
    if let Err(error) = write_frame(&mut pipe, &response) {
        crate::logger::log(format!("remote_ipc: response write failed: {error}"));
    }
}

fn create_server_pipe(first: bool) -> Result<PipeStream, std::io::Error> {
    let name = wide_nul(PIPE_NAME);
    let mut access = PIPE_ACCESS_DUPLEX;
    if first {
        access = FILE_FLAGS_AND_ATTRIBUTES(access.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0);
    }
    let mode = NAMED_PIPE_MODE(
        PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0,
    );
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            access,
            mode,
            PIPE_MAX_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(PipeStream {
            handle,
            server_side: true,
        })
    }
}

fn poke_listener() {
    let _ = open_client_pipe(Duration::from_millis(100));
}

fn open_client_pipe(timeout: Duration) -> Result<PipeStream, std::io::Error> {
    let name = wide_nul(PIPE_NAME);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match handle {
            Ok(handle) => {
                return Ok(PipeStream {
                    handle,
                    server_side: false,
                });
            }
            Err(_) => {
                let error = unsafe { GetLastError() };
                if error == ERROR_PIPE_BUSY && std::time::Instant::now() < deadline {
                    unsafe {
                        let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 20);
                    }
                    continue;
                }
                if error == ERROR_FILE_NOT_FOUND && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                return Err(std::io::Error::last_os_error());
            }
        }
    }
}

struct PipeStream {
    handle: HANDLE,
    server_side: bool,
}

unsafe impl Send for PipeStream {}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0_u32;
        unsafe { ReadFile(self.handle, Some(buffer), Some(&mut read), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(read as usize)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let mut written = 0_u32;
        unsafe { WriteFile(self.handle, Some(buffer), Some(&mut written), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        unsafe {
            if self.server_side {
                let _ = DisconnectNamedPipe(self.handle);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}
