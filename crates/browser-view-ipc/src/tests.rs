use super::*;
use std::io::Cursor;

fn request() -> Request {
    Request::new(
        Placement {
            left: -800,
            top: 10,
            right: 0,
            bottom: 610,
            maximized: false,
        },
        std::path::absolute("日本語の画像.png").unwrap(),
    )
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
struct Duplex {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
    events: Vec<&'static str>,
    grant_fails: bool,
    pid_fails: bool,
}
impl Duplex {
    fn input(value: &impl Serialize) -> Self {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, value).unwrap();
        Self {
            input: Cursor::new(bytes),
            output: Vec::new(),
            events: Vec::new(),
            grant_fails: false,
            pid_fails: false,
        }
    }
    fn reply(&self) -> Response {
        read_frame(&mut Cursor::new(&self.output)).unwrap()
    }
}
impl Read for Duplex {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.events.push("read");
        self.input.read(bytes)
    }
}
impl Write for Duplex {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.events.push("write");
        self.output.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Connection for Duplex {
    fn server_pid(&mut self) -> io::Result<u32> {
        self.events.push("pid");
        if self.pid_fails {
            Err(io::Error::other("PID failed"))
        } else {
            Ok(42)
        }
    }
    fn allow_foreground(&mut self, pid: u32) -> io::Result<()> {
        assert_eq!(pid, 42);
        self.events.push("grant");
        if self.grant_fails {
            Err(io::Error::other("grant denied"))
        } else {
            Ok(())
        }
    }
}
fn response(status: Status) -> Response {
    Response {
        protocol_version: PROTOCOL_VERSION,
        application_version: APPLICATION_VERSION.into(),
        status,
    }
}

#[test]
fn browser818_cli_unicode_delimiter_and_position_round_trip() {
    let expected = request();
    let parsed = parse_cli(&[
        CLI_FLAG.into(),
        expected.placement.cli_value().into(),
        "--".into(),
        expected.path.clone().into_os_string(),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(parsed, expected);
    let literal = parse_cli(&args(&[
        CLI_FLAG,
        "-800,10,0,610,1",
        "--",
        "--browser-view",
    ]))
    .unwrap()
    .unwrap();
    assert!(literal.path.ends_with("--browser-view"));
    assert!(literal.placement.maximized);
    assert!(parse_cli(&args(&["--", CLI_FLAG])).unwrap().is_none());
    assert!(
        parse_cli(&args(&["--data-dir", CLI_FLAG]))
            .unwrap()
            .is_none()
    );
    assert!(
        parse_cli(&args(&["--gpu-backend", "vulkan", "ordinary.png"]))
            .unwrap()
            .is_none()
    );
    assert!(
        parse_cli(&args(&[
            "--perf-log",
            "perf.jsonl",
            CLI_FLAG,
            "0,0,500,400,0",
            "--",
            "x.png"
        ]))
        .unwrap()
        .is_some()
    );
}
#[test]
fn browser818_cli_missing_arguments_and_invalid_rectangles_are_rejected() {
    for values in [
        vec![CLI_FLAG],
        vec![CLI_FLAG, "0,0,500,400,0"],
        vec![CLI_FLAG, "0,0,500,400,0", "x.png"],
        vec![CLI_FLAG, "0,0,500,400,0", "--"],
        vec![CLI_FLAG, "0,0,500,400,0", "--", ""],
        vec![CLI_FLAG, "0,0,500,400,0", "--", "x.png", "extra"],
    ] {
        assert!(parse_cli(&args(&values)).is_err(), "{values:?}");
    }
    for value in [
        "0,0,0,400,0",
        "0,0,500,-1,0",
        "-2147483648,0,2147483647,400,0",
        "0,0,65537,400,0",
        "0,0,NaN,400,0",
        "0,0,500,400,2",
        "0,0,500,400",
        "0,0,2147483648,400,0",
    ] {
        assert!(Placement::parse(value).is_err(), "{value}");
    }
}
#[test]
fn browser818_ipc_unicode_frame_queues_exactly_once_and_ack_is_channel_only() {
    let expected = request();
    let mut stream = Duplex::input(&expected);
    let mut queued = Vec::new();
    serve(&mut stream, |request| {
        queued.push(request);
        true
    })
    .unwrap();
    assert_eq!(queued, vec![expected]);
    assert_eq!(stream.reply().status, Status::Queued);
    let mut stream = Duplex::input(&request());
    serve(&mut stream, |_| false).unwrap();
    assert_eq!(stream.reply().status, Status::ChannelClosed);
}
#[test]
fn browser818_ipc_unknown_protocol_or_application_version_never_queues() {
    for protocol in [true, false] {
        let mut request = request();
        if protocol {
            request.protocol_version += 1;
        } else {
            request.application_version = "old-core".into();
        }
        let mut stream = Duplex::input(&request);
        serve(&mut stream, |_| {
            panic!("incompatible request must not reach activation")
        })
        .unwrap();
        assert_eq!(stream.reply().status, Status::VersionMismatch);
    }
}
#[test]
fn browser818_ipc_invalid_request_never_queues() {
    for bad_path in [false, true] {
        let mut request = request();
        if bad_path {
            request.path = "relative.png".into();
        } else {
            request.placement.right = request.placement.left;
        }
        let mut stream = Duplex::input(&request);
        serve(&mut stream, |_| {
            panic!("invalid request must not reach activation")
        })
        .unwrap();
        assert_eq!(stream.reply().status, Status::InvalidRequest);
    }
    let mut request = request();
    request.path.set_file_name("nul\0.png");
    assert!(request.validate().is_err());
}
#[test]
fn browser818_ipc_frame_limits_reject_before_reading_body_and_writing_header() {
    for size in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        let mut stream = Cursor::new(size.to_le_bytes());
        assert!(read_frame::<Request>(&mut stream).is_err());
        assert_eq!(stream.position(), 4);
    }
    let mut bytes = Vec::new();
    assert!(write_frame(&mut bytes, &"x".repeat(MAX_FRAME_BYTES)).is_err());
    assert!(bytes.is_empty());
    assert!(read_frame::<Request>(&mut Cursor::new([1, 0])).is_err());
}
#[test]
fn browser818_ipc_server_pid_and_grant_precede_request_write() {
    let mut stream = Duplex::input(&response(Status::Queued));
    deliver(&mut stream, &request(), |_| panic!("unexpected warning")).unwrap();
    assert_eq!(&stream.events[..3], &["pid", "grant", "write"]);
    let sent: Request = read_frame(&mut Cursor::new(stream.output)).unwrap();
    assert_eq!(sent, request());
}
#[test]
fn browser818_ipc_foreground_failure_logs_and_keeps_single_delivery() {
    for pid_fails in [false, true] {
        let mut stream = Duplex::input(&response(Status::Queued));
        stream.pid_fails = pid_fails;
        stream.grant_fails = true;
        let mut warnings = Vec::new();
        deliver(&mut stream, &request(), |line| {
            warnings.push(line.to_owned())
        })
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            stream
                .events
                .iter()
                .filter(|event| **event == "pid")
                .count(),
            1
        );
        let mut body = Cursor::new(stream.output);
        let _: Request = read_frame(&mut body).unwrap();
        assert_eq!(body.position() as usize, body.get_ref().len());
    }
    let mut grants = Vec::new();
    let mut warnings = Vec::new();
    grant_child_foreground(
        77,
        |pid| {
            grants.push(pid);
            Err(io::Error::other("denied"))
        },
        |line| warnings.push(line.to_owned()),
    );
    assert_eq!(grants, vec![77]);
    assert_eq!(warnings.len(), 1);
}
#[test]
fn browser818_ipc_old_core_reply_is_terminal_without_retry() {
    for mut reply in [
        response(Status::VersionMismatch),
        response(Status::Queued),
        response(Status::ChannelClosed),
    ] {
        if reply.status == Status::Queued {
            reply.protocol_version += 1;
        }
        let mut stream = Duplex::input(&reply);
        assert!(deliver(&mut stream, &request(), |_| {}).is_err());
        assert_eq!(
            stream
                .events
                .iter()
                .filter(|event| **event == "pid")
                .count(),
            1
        );
    }
    let mut reply = response(Status::Queued);
    reply.application_version = "old".into();
    assert!(deliver(&mut Duplex::input(&reply), &request(), |_| {}).is_err());
}
#[test]
fn browser818_ipc_probe_then_core_mutex_race_routes_without_second_spawn() {
    let mut sends = 0;
    assert_eq!(
        route(false, || panic!("not running")).unwrap(),
        StartupRoute::StartCore
    );
    assert_eq!(
        route(true, || {
            sends += 1;
            Ok(())
        })
        .unwrap(),
        StartupRoute::Forwarded
    );
    assert!(
        route(true, || {
            sends += 1;
            Err(io::Error::other("pipe absent"))
        })
        .is_err()
    );
    assert_eq!(sends, 2);
}
