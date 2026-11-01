//! Overlapped, stop-event driven pipe I/O. A shutdown cannot miss an operation
//! starting after CancelIoEx: the persistent event participates in every wait.
use std::io::{self, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use mimageviewer_agent_ipc::{Request, read_frame, write_frame};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
            WAIT_OBJECT_0,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
            PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
        },
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE,
                PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
            },
            Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
        },
    },
    core::PCWSTR,
};

struct Event(HANDLE);
unsafe impl Send for Event {}
unsafe impl Sync for Event {}
impl Event {
    fn new() -> io::Result<Self> {
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(Self)
            .map_err(io::Error::other)
    }
    fn set(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(super) struct Server {
    stop: Arc<Event>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    pub(super) fn start(handlers: Arc<super::Handlers>) -> io::Result<Self> {
        let stop = Arc::new(Event::new()?);
        let worker_stop = stop.clone();
        let worker = std::thread::Builder::new()
            .name("agent-pipe".into())
            .spawn(move || {
                if run(handlers, worker_stop).is_err() {
                    crate::logger::log("agent_ipc: pipe server failed");
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.set();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
pub(super) fn fixture_server(handlers: Arc<super::Handlers>) -> io::Result<Server> {
    let path = mimageviewer_agent_ipc::resolve_data_dir(&crate::data_dir::get())?;
    let name = mimageviewer_agent_ipc::derive_pipe_name(
        &path,
        &mimageviewer_agent_ipc::current_user_sid()?,
    )?;
    let stop = Arc::new(Event::new()?);
    let pipes = (0..8)
        .map(|i| Stream::create(&name, i == 0, stop.clone()))
        .collect::<io::Result<_>>()?;
    let worker = std::thread::spawn(move || run_pipes(handlers, pipes));
    Ok(Server {
        stop,
        worker: Some(worker),
    })
}

fn run(handlers: Arc<super::Handlers>, stop: Arc<Event>) -> io::Result<()> {
    let path = mimageviewer_agent_ipc::resolve_data_dir(&crate::data_dir::get())?;
    let name = mimageviewer_agent_ipc::derive_pipe_name(
        &path,
        &mimageviewer_agent_ipc::current_user_sid()?,
    )?;
    // Keep all instances alive before accepting; FIRST_PIPE_INSTANCE remains exclusive
    // even when another listener is being recreated after a disconnect.
    let pipes: Vec<_> = (0..8)
        .map(|i| Stream::create(&name, i == 0, stop.clone()))
        .collect::<io::Result<_>>()?;
    crate::logger::log("agent_ipc: state=listener_started");
    run_pipes(handlers, pipes);
    crate::logger::log("agent_ipc: state=listener_stopped");
    Ok(())
}

fn run_pipes(handlers: Arc<super::Handlers>, pipes: Vec<Stream>) {
    std::thread::scope(|scope| {
        for mut pipe in pipes {
            let handlers = handlers.clone();
            scope.spawn(move || {
                loop {
                    if pipe.connect().is_err() {
                        break;
                    }
                    serve(&pipe, handlers.clone());
                    unsafe {
                        let _ = DisconnectNamedPipe(pipe.inner.handle);
                    }
                    // New per-connection event and protocol state; no old Cancel identity
                    // or response writer can reach the next connected client.
                    pipe.connection_stop = match Event::new() {
                        Ok(e) => Arc::new(e),
                        Err(_) => break,
                    };
                }
            });
        }
    });
}

fn serve(pipe: &Stream, handlers: Arc<super::Handlers>) {
    let connection = super::Connection::new(handlers);
    let (tx, rx) = crossbeam_channel::bounded::<super::Reply>(8);
    let writer_failed = Arc::new(AtomicBool::new(false));
    let mut reader = pipe.clone();
    std::thread::scope(|scope| {
        let mut output = pipe.clone();
        let failed = writer_failed.clone();
        let writer = scope.spawn(move || {
            while let Ok(reply) = rx.recv() {
                if write_frame(&mut output, &reply.wire()).is_err() {
                    failed.store(true, Ordering::Release);
                    output.connection_stop.set();
                    break;
                }
            }
        });
        let mut jobs: Vec<JoinHandle<()>> = Vec::new();
        while !writer_failed.load(Ordering::Acquire) {
            let request = match read_frame::<_, Request>(&mut reader) {
                Ok(Some(r)) => r,
                _ => break,
            };
            jobs.retain_mut(|job| !job.is_finished());
            if let Some(job) = connection.request(request, tx.clone()) {
                jobs.push(job);
            }
        }
        connection.disconnect();
        pipe.connection_stop.set();
        for job in jobs {
            let _ = job.join();
        }
        drop(tx);
        let _ = writer.join();
    });
}

struct Handle {
    handle: HANDLE,
}
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}
#[derive(Clone)]
struct Stream {
    inner: Arc<Handle>,
    stop: Arc<Event>,
    connection_stop: Arc<Event>,
}
impl Stream {
    fn create(name: &str, first: bool, stop: Arc<Event>) -> io::Result<Self> {
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // Same helper as Remote: current user's SID only; no integrity-level change.
        let mut security = crate::remote_ipc::current_user_pipe_security()?;
        let attributes = security.attributes();
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(
                    PIPE_ACCESS_DUPLEX.0
                        | FILE_FLAG_OVERLAPPED.0
                        | if first {
                            FILE_FLAG_FIRST_PIPE_INSTANCE.0
                        } else {
                            0
                        },
                ),
                NAMED_PIPE_MODE(
                    PIPE_TYPE_BYTE.0
                        | PIPE_READMODE_BYTE.0
                        | PIPE_WAIT.0
                        | PIPE_REJECT_REMOTE_CLIENTS.0,
                ),
                8,
                65536,
                65536,
                0,
                Some(&attributes),
            )
        };
        if handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        let inner = Arc::new(Handle { handle });
        Ok(Self {
            inner,
            stop,
            connection_stop: Arc::new(Event::new()?),
        })
    }
    fn connect(&self) -> io::Result<()> {
        let event = Event::new()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started = unsafe { ConnectNamedPipe(self.inner.handle, Some(&mut op)) };
        if started.is_err() && unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
            return Ok(());
        }
        self.finish(&op, started).map(|_| ())
    }
    fn finish(&self, op: &OVERLAPPED, started: windows::core::Result<()>) -> io::Result<usize> {
        if started.is_err() {
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(error.0 as i32));
            }
        }
        let waited = unsafe {
            WaitForMultipleObjects(
                &[self.stop.0, self.connection_stop.0, op.hEvent],
                false,
                INFINITE,
            )
        };
        if waited != windows::Win32::Foundation::WAIT_EVENT(WAIT_OBJECT_0.0 + 2) {
            unsafe {
                let _ = CancelIoEx(self.inner.handle, Some(op));
                // Keep OVERLAPPED and its buffer alive until the cancellation completes.
                let mut ignored = 0;
                let _ = GetOverlappedResult(self.inner.handle, op, &mut ignored, true);
            }
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "agent pipe stopped",
            ));
        }
        let mut transferred = 0;
        unsafe { GetOverlappedResult(self.inner.handle, op, &mut transferred, false) }
            .map_err(io::Error::other)?;
        Ok(transferred as usize)
    }
}
impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let event = Event::new()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started = unsafe { ReadFile(self.inner.handle, Some(buffer), None, Some(&mut op)) };
        self.finish(&op, started)
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let event = Event::new()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started = unsafe { WriteFile(self.inner.handle, Some(bytes), None, Some(&mut op)) };
        self.finish(&op, started)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_interrupts_an_idle_connected_reader_without_retry() {
        let name = format!(r"\\.\pipe\miv-agent-test-{}", uuid::Uuid::new_v4());
        let stop = Arc::new(Event::new().unwrap());
        let mut stream = Stream::create(&name, true, stop.clone()).unwrap();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = std::thread::spawn(move || {
            stream.connect().unwrap();
            ready_tx.send(()).unwrap();
            let result = read_frame::<_, Request>(&mut stream);
            done_tx.send(result.unwrap_err().kind()).unwrap();
        });
        let _client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .unwrap();
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        stop.set();
        assert_eq!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            io::ErrorKind::ConnectionAborted
        );
        worker.join().unwrap();
    }
}
