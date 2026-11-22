//! Real named-pipe tests, with no foreground grant or desktop interaction.
use super::*;
use crate::{
    APPLICATION_VERSION, PROTOCOL_VERSION, Placement, Request, Response, Status, read_frame,
    write_frame,
};
use std::{
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

fn name() -> String {
    static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        r"\\.\pipe\miv-browser818-terminal-{}-{}",
        std::process::id(),
        ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
fn request() -> Request {
    Request::new(
        Placement {
            left: -800,
            top: 0,
            right: 0,
            bottom: 600,
            maximized: false,
        },
        std::path::absolute("日本語.png").unwrap(),
    )
}
fn open_when_listening(name: &str) -> io::Result<Stream> {
    use windows::{Win32::System::Pipes::WaitNamedPipeW, core::PCWSTR};
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // Test-only synchronization with the kernel's pending ConnectNamedPipe state.
    // One availability wait, one open: no sleep or connection retry.
    if !unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), 3_000) }.as_bool() {
        return Err(io::Error::last_os_error());
    }
    Stream::open(name)
}
fn open_for_manual_accept(server: &Server) -> Stream {
    let name = server.name.clone();
    let stop = server.stop.clone();
    let client = std::thread::spawn(move || {
        let result = open_when_listening(&name);
        if result.is_err() {
            stop.signal();
        }
        result
    });
    // A reused, disconnected instance cannot be opened before connect is started.
    // Complete that operation here, then retain the original prebuffered-request
    // assertions: PeerWorkers::accept observes the already connected client.
    let connected = server.pending.connect();
    let opened = client.join().unwrap();
    connected.unwrap();
    opened.unwrap()
}
fn assert_reply(client: &mut Stream, status: Status) {
    let reply: Response = read_frame(client).unwrap();
    assert_eq!(reply.protocol_version, PROTOCOL_VERSION);
    assert_eq!(reply.application_version, APPLICATION_VERSION);
    assert_eq!(reply.status, status);
}
fn assert_refused_without_reply(client: &mut Stream) {
    use windows::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED};
    match client.read(&mut [0; 1]) {
        Ok(0) => {}
        Err(error) => assert!(
            matches!(error.raw_os_error(), Some(code)
            if code == ERROR_BROKEN_PIPE.0 as i32 || code == ERROR_NO_DATA.0 as i32
                || code == ERROR_PIPE_NOT_CONNECTED.0 as i32),
            "{error}"
        ),
        Ok(count) => panic!("overflow peer received {count} response bytes"),
    }
}
fn wait_for_buffered_reply(client: &Stream, status: Status) {
    use windows::Win32::System::Pipes::PeekNamedPipe;
    let mut expected = Vec::new();
    write_frame(
        &mut expected,
        &Response {
            protocol_version: PROTOCOL_VERSION,
            application_version: APPLICATION_VERSION.into(),
            status,
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut available = 0;
        // Peek proves the response was written without consuming even its header.
        unsafe { PeekNamedPipe(client.handle.0, None, 0, None, Some(&mut available), None) }
            .unwrap();
        if available as usize >= expected.len() {
            return;
        }
        assert!(Instant::now() < deadline, "reply was not buffered");
        std::thread::yield_now();
    }
}

struct Running {
    stop: Arc<Stop>,
    done: mpsc::Receiver<io::Result<()>>,
    worker: Option<JoinHandle<()>>,
}
impl Running {
    fn spawn(stop: Arc<Stop>, run: impl FnOnce() -> io::Result<()> + Send + 'static) -> Self {
        let (tx, done) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(run());
        });
        Self {
            stop,
            done,
            worker: Some(worker),
        }
    }
    fn finish(&mut self) -> io::Result<()> {
        let result = self
            .done
            .recv_timeout(Duration::from_secs(3))
            .expect("pipe worker did not finish");
        self.worker.take().unwrap().join().unwrap();
        result
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.signal();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Service {
    name: String,
    running: Running,
    errors: mpsc::Receiver<io::Error>,
}
impl Service {
    fn start() -> Self {
        let name = name();
        let stop = Stop::new().unwrap();
        // Publish the first listening instance before the test client opens it.
        let server = Server::new(&name, stop.clone(), None).unwrap();
        let (tx, errors) = mpsc::channel();
        let running = Running::spawn(stop, move || {
            server.run(
                None,
                |_| true,
                move |error| {
                    let _ = tx.send(error);
                },
            )
        });
        Self {
            name,
            running,
            errors,
        }
    }
    fn open(&self, request: &Request) -> Stream {
        let mut client = open_when_listening(&self.name).unwrap();
        write_frame(&mut client, request).unwrap();
        client
    }
    fn stop(&mut self) {
        self.running.stop.signal();
        self.running.finish().unwrap();
        assert!(self.errors.try_iter().collect::<Vec<_>>().is_empty());
    }
}

#[test]
fn browser818_ipc_delayed_reply_read_keeps_complete_response_and_accepts_other_peers() {
    for status in [Status::Queued, Status::VersionMismatch] {
        let mut service = Service::start();
        let mut first_request = request();
        if status == Status::VersionMismatch {
            first_request.protocol_version += 1;
        }
        let mut delayed = service.open(&first_request);
        wait_for_buffered_reply(&delayed, status);

        // Keep the first response entirely unread while another connection completes.
        // Keep that second peer open too: neither EOF wait may occupy the accept loop.
        let mut second = service.open(&request());
        assert_reply(&mut second, Status::Queued);
        assert_reply(&mut delayed, status);
        assert!(matches!(
            service.running.done.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(delayed);
        drop(second);

        // Closed peers do not retire the listening name or prevent its next request.
        let mut next = service.open(&request());
        assert_reply(&mut next, Status::Queued);
        drop(next);
        service.stop();
    }
}

#[test]
fn browser818_ipc_client_close_releases_reply_wait_without_stop() {
    let name = name();
    let stop = Stop::new().unwrap();
    let mut server = Stream::server(&name, stop.clone(), None).unwrap();
    let (written_tx, written_rx) = mpsc::channel();
    let mut running = Running::spawn(stop.clone(), move || {
        server.connect()?;
        crate::serve(&mut server, |_| true)?;
        written_tx.send(()).unwrap();
        let result = server.wait_for_client_close();
        server.disconnect();
        result
    });
    let mut client = Stream::open(&name).unwrap();
    write_frame(&mut client, &request()).unwrap();
    written_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_reply(&mut client, Status::Queued);
    assert!(matches!(
        running.done.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    // Unexpected trailing bytes are not a second request or permission to disconnect.
    client.write_all(b"trailing").unwrap();
    drop(client);
    running.finish().unwrap();
    assert!(!stop.is_set(), "EOF, not stop, must release the connection");
}

#[test]
fn browser818_ipc_stop_joins_reply_waiters_with_clients_still_open() {
    let mut service = Service::start();
    let mut clients = Vec::new();
    for index in 0..MAX_PEERS {
        let mut client = service.open(&request());
        if index == 0 {
            wait_for_buffered_reply(&client, Status::Queued);
        } else {
            assert_reply(&mut client, Status::Queued);
        }
        clients.push(client);
    }
    assert_reply(&mut clients[0], Status::Queued);
    // All four peers still own open client handles. Stop must cancel the EOF waits AND
    // the next accept, and run must join all connection workers before returning.
    service.stop();
    drop(clients);
}

#[test]
fn browser818_ipc_peer_limit_rejects_without_reply_and_keeps_listener_alive() {
    assert_eq!(MAX_PEERS, 4);
    let mut service = Service::start();
    let mut clients = Vec::new();
    for _ in 0..MAX_PEERS {
        let mut client = service.open(&request());
        assert_reply(&mut client, Status::Queued);
        clients.push(client); // Retain every response's EOF wait.
    }
    for _ in 0..3 {
        // Do not race a request write against the intentional immediate rejection.
        let mut refused = open_when_listening(&service.name).unwrap();
        assert_refused_without_reply(&mut refused);
        let error = service.errors.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(!service.running.stop.is_set());
        assert!(matches!(
            service.running.done.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }
    // Overflow must neither retire the listener nor leave an unjoined fifth worker.
    service.stop();
    drop(clients);
}

#[test]
fn browser818_ipc_peer_limit_recovers_slot_after_completed_close() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let name = name();
    let stop = Stop::new().unwrap();
    let mut server = Server::new(&name, stop.clone(), None).unwrap();
    // Exercise the exact admission owner used by Server::run, with observable handles
    // so completion is awaited without retrying a rejected request or guessing a delay.
    let mut peers = PeerWorkers::new(stop.clone());
    let delivered = Arc::new(AtomicUsize::new(0));
    let count = delivered.clone();
    let deliver = Arc::new(move |_: Request| {
        count.fetch_add(1, Ordering::Relaxed);
        true
    });
    let (log_tx, log_rx) = mpsc::channel();
    let log = Arc::new(move |error: io::Error| {
        let _ = log_tx.send(error);
    });
    let mut clients = Vec::new();
    for _ in 0..MAX_PEERS {
        let mut client = open_for_manual_accept(&server);
        write_frame(&mut client, &request()).unwrap();
        peers
            .accept(&mut server, None, deliver.clone(), log.clone())
            .unwrap();
        assert_reply(&mut client, Status::Queued);
        clients.push(client);
    }
    assert_eq!(peers.workers.len(), MAX_PEERS);
    assert_eq!(delivered.load(Ordering::Relaxed), MAX_PEERS);
    let pending_handle = server.pending.handle.0;
    let mut refused_clients = Vec::new();
    for _ in 0..2 {
        let mut refused = open_for_manual_accept(&server);
        write_frame(&mut refused, &request()).unwrap();
        peers
            .accept(&mut server, None, deliver.clone(), log.clone())
            .unwrap();
        assert_refused_without_reply(&mut refused);
        assert_eq!(
            log_rx.recv_timeout(Duration::from_secs(3)).unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            server.pending.handle.0, pending_handle,
            "overflow must reuse the listening instance"
        );
        assert_eq!(peers.workers.len(), MAX_PEERS);
        assert_eq!(delivered.load(Ordering::Relaxed), MAX_PEERS);
        refused_clients.push(refused); // Even retained refused handles must not hold up accept.
    }
    for round in 1..=2 {
        drop(clients.remove(0));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !peers.workers.iter().any(JoinHandle::is_finished) {
            assert!(Instant::now() < deadline, "closed peer did not finish");
            std::thread::yield_now();
        }
        // The completed handle still occupies the old owner until this new admission.
        assert_eq!(peers.workers.len(), MAX_PEERS);
        let mut replacement = open_for_manual_accept(&server);
        write_frame(&mut replacement, &request()).unwrap();
        peers
            .accept(&mut server, None, deliver.clone(), log.clone())
            .unwrap();
        assert_reply(&mut replacement, Status::Queued);
        clients.push(replacement);
        assert_eq!(peers.workers.len(), MAX_PEERS);
        assert_eq!(delivered.load(Ordering::Relaxed), MAX_PEERS + round);
        assert!(!stop.is_set());
    }
    // Clients remain open. The same lifetime owner must stop and join every worker.
    drop(peers);
    assert!(stop.is_set());
    assert!(log_rx.try_iter().collect::<Vec<_>>().is_empty());
    drop(clients);
    drop(refused_clients);
}
