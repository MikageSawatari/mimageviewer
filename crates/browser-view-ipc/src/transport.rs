//! Each client uses one connection, no reconnect/retry. Pending I/O retains its buffer/OVERLAPPED
//! until cancellation has completed. Stop participates in connect, read AND write.
use std::{
    io::{self, Read, Write},
    sync::Arc,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, WAIT_OBJECT_0},
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
            PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
        },
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
                GetNamedPipeServerProcessId, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
                PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::{
                CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
            },
        },
        UI::WindowsAndMessaging::AllowSetForegroundWindow,
    },
    core::PCWSTR,
};

const IO_TIMEOUT_MS: u32 = 5_000;
// Includes peers waiting for EOF after their reply. Never queue or retry overflow.
const MAX_PEERS: usize = 4;

fn windows_io_error(error: windows::core::Error) -> io::Error {
    // Retain Win32 disconnect codes both at I/O start and at overlapped completion.
    let code = error.code().0 as u32;
    if code & 0xffff_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xffff) as i32)
    } else {
        io::Error::other(error)
    }
}
struct OwnedHandle(HANDLE);
// Windows kernel handles are usable on the worker thread; this owner closes exactly once.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl OwnedHandle {
    fn event() -> io::Result<Self> {
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(Self)
            .map_err(io::Error::other)
    }
}
pub struct Stop(OwnedHandle);
impl Stop {
    pub fn new() -> io::Result<Arc<Self>> {
        Ok(Arc::new(Self(OwnedHandle::event()?)))
    }
    pub fn signal(&self) {
        unsafe {
            let _ = SetEvent(self.0.0);
        }
    }
    pub fn is_set(&self) -> bool {
        unsafe { WaitForSingleObject(self.0.0, 0) == WAIT_OBJECT_0 }
    }
}

pub struct Stream {
    handle: OwnedHandle,
    stop: Option<Arc<Stop>>,
}

/// The retained JoinHandles are both the capacity count and the lifetime owners.
/// No separate permit/counter can release a slot before its worker has finished.
struct PeerWorkers {
    stop: Arc<Stop>,
    workers: Vec<std::thread::JoinHandle<()>>,
}
impl PeerWorkers {
    fn new(stop: Arc<Stop>) -> Self {
        Self {
            stop,
            workers: Vec::new(),
        }
    }
    fn accept(
        &mut self,
        server: &mut Server,
        security: Option<&SECURITY_ATTRIBUTES>,
        deliver: Arc<impl Fn(crate::Request) -> bool + Send + Sync + 'static>,
        log: Arc<impl Fn(io::Error) + Send + Sync + 'static>,
    ) -> io::Result<()> {
        server.pending.connect()?;
        // Reap AFTER accept: peers may have closed while the listener was waiting.
        for index in (0..self.workers.len()).rev() {
            if self.workers[index].is_finished() {
                let _ = self.workers.swap_remove(index).join();
            }
        }
        if self.workers.len() >= MAX_PEERS {
            // No reply, reader, worker, or activation is created for an overflow peer.
            // Reuse this listening instance instead of allocating another for each refusal.
            server.pending.disconnect();
            log(io::Error::new(
                io::ErrorKind::WouldBlock,
                "browser peer limit reached",
            ));
            return Ok(());
        }
        let mut pipe = server.take_connected(security)?;
        let stop = self.stop.clone();
        self.workers.push(
            std::thread::Builder::new()
                .name("miv-browser-view-peer".into())
                .spawn(move || {
                    if let Err(error) = pipe.serve_until_closed(|request| deliver(request)) {
                        if !stop.is_set() {
                            log(error);
                        }
                    }
                })?,
        );
        Ok(())
    }
}
impl Drop for PeerWorkers {
    fn drop(&mut self) {
        // Also retire every peer on an accept/spawn failure, without detached reads.
        self.stop.signal();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Retain a listening instance while each connected instance owns its reply/EOF wait.
/// The successor is created before handing off the previous handle, so the pipe name
/// stays owned throughout the handoff (FIRST_PIPE_INSTANCE is only used once).
pub struct Server {
    name: String,
    pending: Stream,
    stop: Arc<Stop>,
}
impl Server {
    pub fn new(
        name: &str,
        stop: Arc<Stop>,
        security: Option<&SECURITY_ATTRIBUTES>,
    ) -> io::Result<Self> {
        Ok(Self {
            name: name.to_owned(),
            pending: Stream::server(name, stop.clone(), security)?,
            stop,
        })
    }

    fn take_connected(&mut self, security: Option<&SECURITY_ATTRIBUTES>) -> io::Result<Stream> {
        let next = Stream::create_server(&self.name, self.stop.clone(), security, false)?;
        Ok(std::mem::replace(&mut self.pending, next))
    }

    /// Security must stay valid for this call. At most four peers retain workers;
    /// overflow is disconnected without a reply. Stop cancels and joins every peer.
    pub fn run(
        mut self,
        security: Option<&SECURITY_ATTRIBUTES>,
        deliver: impl Fn(crate::Request) -> bool + Send + Sync + 'static,
        log: impl Fn(io::Error) + Send + Sync + 'static,
    ) -> io::Result<()> {
        let deliver = Arc::new(deliver);
        let log = Arc::new(log);
        let mut peers = PeerWorkers::new(self.stop.clone());
        while !self.stop.is_set() {
            match peers.accept(&mut self, security, deliver.clone(), log.clone()) {
                Ok(()) => {}
                Err(_) if self.stop.is_set() => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl Stream {
    /// The caller owns a current-user-only descriptor for the duration of this call.
    pub fn server(
        name: &str,
        stop: Arc<Stop>,
        security: Option<&SECURITY_ATTRIBUTES>,
    ) -> io::Result<Self> {
        Self::create_server(name, stop, security, true)
    }
    fn create_server(
        name: &str,
        stop: Arc<Stop>,
        security: Option<&SECURITY_ATTRIBUTES>,
        first: bool,
    ) -> io::Result<Self> {
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
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
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                0,
                security.map(|v| v as *const _),
            )
        };
        if handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            handle: OwnedHandle(handle),
            stop: Some(stop),
        })
    }
    pub fn open(name: &str) -> io::Result<Self> {
        use std::os::windows::{fs::OpenOptionsExt, io::IntoRawHandle};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED.0)
            .open(name)?;
        Ok(Self {
            handle: OwnedHandle(HANDLE(file.into_raw_handle())),
            stop: None,
        })
    }
    fn check_stop(&self) -> io::Result<()> {
        if self.stop.as_ref().is_some_and(|stop| stop.is_set()) {
            // read_exact/write_all retry Interrupted; a persistent stop must be terminal.
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "browser listener stopped",
            ))
        } else {
            Ok(())
        }
    }
    fn complete(
        &self,
        started: windows::core::Result<()>,
        op: &mut OVERLAPPED,
        timeout: u32,
    ) -> io::Result<usize> {
        if let Err(error) = started {
            if error.code() != ERROR_IO_PENDING.to_hresult() {
                return Err(windows_io_error(error));
            }
        }
        let mut events = Vec::with_capacity(2);
        if let Some(stop) = &self.stop {
            events.push(stop.0.0);
        }
        events.push(op.hEvent);
        let completed_index = events.len() - 1;
        let wait = unsafe { WaitForMultipleObjects(&events, false, timeout) };
        if wait.0 != WAIT_OBJECT_0.0 + completed_index as u32 {
            unsafe {
                let _ = CancelIoEx(self.handle.0, Some(&*op));
                let mut discarded = 0;
                let _ = GetOverlappedResult(self.handle.0, op, &mut discarded, true);
            }
            return Err(io::Error::new(
                if self.stop.as_ref().is_some_and(|stop| stop.is_set()) {
                    io::ErrorKind::ConnectionAborted
                } else {
                    io::ErrorKind::TimedOut
                },
                "browser pipe operation stopped or timed out",
            ));
        }
        let mut count = 0;
        unsafe { GetOverlappedResult(self.handle.0, op, &mut count, false) }
            .map_err(windows_io_error)?;
        Ok(count as usize)
    }
    pub fn connect(&self) -> io::Result<()> {
        self.check_stop()?;
        let event = OwnedHandle::event()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started = unsafe { ConnectNamedPipe(self.handle.0, Some(&mut op)) };
        if started
            .as_ref()
            .is_err_and(|error| error.code() == ERROR_PIPE_CONNECTED.to_hresult())
        {
            return self.check_stop();
        }
        self.complete(started, &mut op, INFINITE).map(|_| ())
    }
    pub fn disconnect(&self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.handle.0);
        }
    }
    /// A successful write only put the reply into the pipe buffer. The client closes
    /// after reading the entire reply; do not discard those bytes via DisconnectNamedPipe.
    /// This wait has no timeout. Stop participates in every read, including new reads
    /// after unexpected trailing client bytes, and cancels the pending OVERLAPPED.
    pub fn wait_for_client_close(&self) -> io::Result<()> {
        use windows::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED};
        let mut trailing = [0; 256];
        loop {
            match self.operation(Some(&mut trailing), None, INFINITE) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error)
                    if matches!(error.raw_os_error(), Some(code)
                    if code == ERROR_BROKEN_PIPE.0 as i32 || code == ERROR_PIPE_NOT_CONNECTED.0 as i32) =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub fn serve_until_closed(
        &mut self,
        deliver: impl FnOnce(crate::Request) -> bool,
    ) -> io::Result<()> {
        let result = crate::serve(self, deliver).and_then(|()| self.wait_for_client_close());
        self.disconnect();
        result
    }
    fn operation(
        &self,
        read: Option<&mut [u8]>,
        write: Option<&[u8]>,
        timeout: u32,
    ) -> io::Result<usize> {
        self.check_stop()?;
        let event = OwnedHandle::event()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started = unsafe {
            match read {
                Some(bytes) => ReadFile(self.handle.0, Some(bytes), None, Some(&mut op)),
                None => WriteFile(self.handle.0, write, None, Some(&mut op)),
            }
        };
        self.complete(started, &mut op, timeout)
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.operation(Some(bytes), None, IO_TIMEOUT_MS)
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.operation(None, Some(bytes), IO_TIMEOUT_MS)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn allow_foreground(pid: u32) -> io::Result<()> {
    unsafe { AllowSetForegroundWindow(pid) }.map_err(io::Error::other)
}
impl crate::Connection for Stream {
    fn server_pid(&mut self) -> io::Result<u32> {
        let mut pid = 0;
        unsafe { GetNamedPipeServerProcessId(self.handle.0, &mut pid) }
            .map_err(io::Error::other)?;
        Ok(pid)
    }
    fn allow_foreground(&mut self, pid: u32) -> io::Result<()> {
        allow_foreground(pid)
    }
}

#[cfg(test)]
#[path = "transport_terminal_tests.rs"]
mod terminal_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    fn fixture() -> (Stream, Arc<Stop>, String) {
        static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let name = format!(
            r"\\.\pipe\miv-browser818-test-{}-{}",
            std::process::id(),
            ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let stop = Stop::new().unwrap();
        let stream = Stream::server(&name, stop.clone(), None).unwrap();
        (stream, stop, name)
    }
    #[test]
    fn browser818_ipc_stop_interrupts_connect_and_partial_frame_read() {
        for connected in [false, true] {
            let (mut server, stop, name) = fixture();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let result = if connected {
                    server.connect().unwrap();
                    let mut header = [0; 4];
                    server.read_exact(&mut header).unwrap();
                    assert_eq!(u32::from_le_bytes(header), 20);
                    ready_tx.send(()).unwrap();
                    server.read_exact(&mut [0; 20])
                } else {
                    ready_tx.send(()).unwrap();
                    server.connect()
                };
                done_tx.send(result.unwrap_err().kind()).unwrap();
            });
            let _client = if connected {
                let mut client = Stream::open(&name).unwrap();
                client.write_all(&20_u32.to_le_bytes()).unwrap();
                Some(client) // Hold the incomplete frame's connection open throughout stop/join.
            } else {
                None
            };
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            stop.signal();
            assert_eq!(
                done_rx
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap(),
                io::ErrorKind::ConnectionAborted
            );
            worker.join().unwrap();
        }
    }
    #[test]
    fn browser818_ipc_stop_before_next_operation_cannot_be_missed() {
        let (mut server, stop, _) = fixture();
        stop.signal();
        assert_eq!(
            server.connect().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
        assert_eq!(
            server.read(&mut [0; 4]).unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
        assert_eq!(
            server.write(&[1]).unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }
}
