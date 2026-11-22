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
fn assert_reply(client: &mut Stream, status: Status) {
    let reply: Response = read_frame(client).unwrap();
    assert_eq!(reply.protocol_version, PROTOCOL_VERSION);
    assert_eq!(reply.application_version, APPLICATION_VERSION);
    assert_eq!(reply.status, status);
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
        let mut client = Stream::open(&self.name).unwrap();
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
    let mut first = service.open(&request());
    wait_for_buffered_reply(&first, Status::Queued);
    let mut second = service.open(&request());
    assert_reply(&mut second, Status::Queued);
    assert_reply(&mut first, Status::Queued);
    // Both peers still own open client handles. Stop must cancel the EOF waits AND
    // the next accept, and run must join all connection workers before returning.
    service.stop();
    drop(first);
    drop(second);
}
