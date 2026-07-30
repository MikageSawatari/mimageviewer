use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mimageviewer_ipc::{
    ClientHello, ClientMessage, MAX_CONTROL_FRAME_BYTES, PIPE_NAME, PROTOCOL_VERSION, RequestId,
    ServerMessage, ThumbnailError, ThumbnailErrorCode, ThumbnailRequest, ThumbnailResponse,
    negotiate, read_frame, write_frame,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows::core::PCWSTR;

use super::thumbnail::{ThumbnailEngine, WorkerContext};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const PIPE_MAX_INSTANCES: u32 = 16;
/// 再接続が集中しても待機中 instance を切らさないため、常時この本数を accept 待ちにする。
const ACCEPTOR_COUNT: usize = 4;
const WORK_QUEUE_CAPACITY: usize = 256;

enum Work {
    Request {
        id: RequestId,
        request: ThumbnailRequest,
        reply: mpsc::Sender<ServerMessage>,
    },
    Stop,
}

pub(super) struct ServerGuard {
    stop: Arc<AtomicBool>,
    listeners: Vec<std::thread::JoinHandle<()>>,
    workers: Vec<std::thread::JoinHandle<()>>,
    work_tx: mpsc::SyncSender<Work>,
}

impl ServerGuard {
    pub(super) fn start(settings: crate::settings::Settings) -> Result<Self, String> {
        // 最初の instance は同名サーバの二重起動検出も兼ねる。他の instance も
        // listener 開始前に作り、起動完了時点で複数本が必ず待機できる形にする。
        let mut initial_pipes = Vec::with_capacity(ACCEPTOR_COUNT);
        initial_pipes.push(create_server_pipe(true).map_err(|error| {
            format!(
                "remote IPC pipe を作成できません。同名サーバが既に存在する可能性があります: {error}"
            )
        })?);
        for _ in 1..ACCEPTOR_COUNT {
            initial_pipes.push(
                create_server_pipe(false).map_err(|error| {
                    format!("remote IPC pipe instance を作成できません: {error}")
                })?,
            );
        }

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
        let mut listeners = Vec::with_capacity(ACCEPTOR_COUNT);
        for (index, initial_pipe) in initial_pipes.into_iter().enumerate() {
            let listener_stop = Arc::clone(&stop);
            let listener_tx = work_tx.clone();
            match std::thread::Builder::new()
                .name(format!("remote-ipc-listener-{index}"))
                .spawn(move || acceptor_loop(listener_stop, listener_tx, initial_pipe, index))
            {
                Ok(listener) => listeners.push(listener),
                Err(error) => {
                    stop.store(true, Ordering::Release);
                    for _ in 0..listeners.len() {
                        poke_listener();
                    }
                    for listener in listeners {
                        let _ = listener.join();
                    }
                    for _ in 0..worker_count {
                        let _ = work_tx.send(Work::Stop);
                    }
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(format!("remote IPC listener を開始できません: {error}"));
                }
            }
        }

        crate::logger::log(format!(
            "remote_ipc: listening pipe={PIPE_NAME} protocol={PROTOCOL_VERSION} workers={worker_count} acceptors={ACCEPTOR_COUNT} multiplexed=true"
        ));
        Ok(Self {
            stop,
            listeners,
            workers,
            work_tx,
        })
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for _ in 0..self.listeners.len() {
            poke_listener();
        }
        for listener in self.listeners.drain(..) {
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
            Ok(Work::Request { id, request, reply }) => {
                let response = engine.handle(request, &context);
                let _ = reply.send(ServerMessage::Thumbnail { id, response });
            }
            Ok(Work::Stop) | Err(_) => break,
        }
    }
}

fn acceptor_loop(
    stop: Arc<AtomicBool>,
    work_tx: mpsc::SyncSender<Work>,
    initial_pipe: PipeStream,
    index: usize,
) {
    let mut next = Some(initial_pipe);
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let pipe = match next.take() {
            Some(pipe) => pipe,
            None => match create_server_pipe(false) {
                Ok(pipe) => pipe,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: stage=accept_create acceptor={index} error={error}"
                    ));
                    break;
                }
            },
        };
        let connected = unsafe {
            match ConnectNamedPipe(pipe.handle(), None) {
                Ok(()) => true,
                Err(_) if GetLastError() == ERROR_PIPE_CONNECTED => true,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: stage=accept_connect acceptor={index} os_error={:?} error={error}",
                        GetLastError()
                    ));
                    false
                }
            }
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
            crate::logger::log(format!("remote_ipc: stage=connection_spawn error={error}"));
        }
        // この接続を処理する thread を起こした直後に次の instance を作る。
        // 他の acceptor も並行して待機しているため、再接続 burst に空白を作らない。
    }
    crate::logger::log(format!("remote_ipc: listener exiting acceptor={index}"));
}

fn handle_connection(mut pipe: PipeStream, work_tx: mpsc::SyncSender<Work>) {
    let hello: ClientHello = match read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES) {
        Ok(hello) => hello,
        Err(error) => {
            crate::logger::log(format!(
                "remote_ipc: stage=handshake_read error_kind={} error={error}",
                frame_error_kind(&error)
            ));
            return;
        }
    };
    let response = negotiate(hello.protocol_version);
    if let Err(error) = write_frame(&mut pipe, &response) {
        crate::logger::log(format!(
            "remote_ipc: stage=handshake_write error_kind={} error={error}",
            frame_error_kind(&error)
        ));
        return;
    }
    if !response.accepted {
        crate::logger::log(format!(
            "remote_ipc: protocol mismatch rejected client={} server={}",
            hello.protocol_version, response.protocol_version
        ));
        return;
    }

    let (reply_tx, reply_rx) = mpsc::channel::<ServerMessage>();
    let mut response_pipe = pipe.clone();
    let writer = match std::thread::Builder::new()
        .name("remote-ipc-writer".to_owned())
        .spawn(move || {
            for response in reply_rx {
                if let Err(error) = write_frame(&mut response_pipe, &response) {
                    crate::logger::log(format!(
                        "remote_ipc: stage=response_write error_kind={} error={error}",
                        frame_error_kind(&error)
                    ));
                    break;
                }
            }
        }) {
        Ok(writer) => writer,
        Err(error) => {
            crate::logger::log(format!("remote_ipc: stage=writer_spawn error={error}"));
            return;
        }
    };

    loop {
        let message: ClientMessage = match read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES) {
            Ok(message) => message,
            Err(mimageviewer_ipc::FrameError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                break;
            }
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: stage=request_read error_kind={} error={error}",
                    frame_error_kind(&error)
                ));
                break;
            }
        };
        match message {
            ClientMessage::Thumbnail { id, request } => {
                // queue が満杯ならここで待つ。接続を切断したり、要求を捨てたりしない。
                if send_with_backpressure(
                    &work_tx,
                    Work::Request {
                        id,
                        request,
                        reply: reply_tx.clone(),
                    },
                )
                .is_err()
                {
                    let _ = reply_tx.send(ServerMessage::Thumbnail {
                        id,
                        response: ThumbnailResponse::Error(ThumbnailError::new(
                            ThumbnailErrorCode::Internal,
                            "mIV 本体のサムネイルワーカーが停止しています",
                        )),
                    });
                    break;
                }
            }
        }
    }
    drop(reply_tx);
    let _ = writer.join();
}

fn send_with_backpressure<T>(
    sender: &mpsc::SyncSender<T>,
    value: T,
) -> Result<(), mpsc::SendError<T>> {
    sender.send(value)
}

fn frame_error_kind(error: &mimageviewer_ipc::FrameError) -> &'static str {
    match error {
        mimageviewer_ipc::FrameError::Io(error) => match error.kind() {
            std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
            std::io::ErrorKind::BrokenPipe => "broken_pipe",
            std::io::ErrorKind::ConnectionReset => "connection_reset",
            std::io::ErrorKind::TimedOut => "timed_out",
            _ => "io",
        },
        mimageviewer_ipc::FrameError::TooLarge { .. } => "too_large",
        mimageviewer_ipc::FrameError::Encode(_) => "encode",
        mimageviewer_ipc::FrameError::Decode(_) => "decode",
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
        Ok(PipeStream::new(handle, true))
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
            Ok(handle) => return Ok(PipeStream::new(handle, false)),
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

struct PipeHandle {
    handle: HANDLE,
    server_side: bool,
}

unsafe impl Send for PipeHandle {}
unsafe impl Sync for PipeHandle {}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        unsafe {
            if self.server_side {
                let _ = DisconnectNamedPipe(self.handle);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

#[derive(Clone)]
struct PipeStream {
    inner: Arc<PipeHandle>,
}

impl PipeStream {
    fn new(handle: HANDLE, server_side: bool) -> Self {
        Self {
            inner: Arc::new(PipeHandle {
                handle,
                server_side,
            }),
        }
    }

    fn handle(&self) -> HANDLE {
        self.inner.handle
    }
}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0_u32;
        unsafe { ReadFile(self.handle(), Some(buffer), Some(&mut read), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(read as usize)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let mut written = 0_u32;
        unsafe { WriteFile(self.handle(), Some(buffer), Some(&mut written), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        unsafe { FlushFileBuffers(self.handle()) }.map_err(|_| std::io::Error::last_os_error())
    }
}

fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_work_queue_waits_instead_of_dropping_the_connection() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(1_u8).unwrap();
        let sender = tx.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let waiting = std::thread::spawn(move || {
            let result = send_with_backpressure(&sender, 2_u8);
            let _ = done_tx.send(result.is_ok());
        });
        assert!(done_rx.recv_timeout(Duration::from_millis(30)).is_err());
        assert_eq!(rx.recv().unwrap(), 1);
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        assert_eq!(rx.recv().unwrap(), 2);
        waiting.join().unwrap();
    }
}
