use super::*;
use base64::Engine;
use std::io::BufRead;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;

fn timestamp() -> CaptureTimestamp {
    CaptureTimestamp {
        stamp: "20261002-153012-345".into(),
        month: "2026-10".into(),
    }
}
fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[1, 2, 3, 255, 3, 2, 1, 128])
            .unwrap();
    }
    bytes
}
fn data_url() -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png())
    )
}
fn collect(session: &CaptureFetchSession) -> Vec<Result<FetchedImage, String>> {
    let mut results = Vec::new();
    loop {
        match session
            .results
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            CaptureFetchEvent::Fetched { result, .. } => results.push(result),
            CaptureFetchEvent::FetchComplete => return results,
            event => panic!("unexpected event: {event:?}"),
        }
    }
}
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "background task did not complete"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            break;
        }
        let last = line == "\r\n";
        headers.push_str(&line);
        if last {
            break;
        }
    }
    headers
}
fn response(stream: &mut TcpStream, status: &str, headers: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
}
fn fixture(
    count: usize,
    handle: impl Fn(usize, &mut TcpStream, &str) + Send + Sync + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let handle = Arc::new(handle);
    let thread = std::thread::spawn(move || {
        let mut children = Vec::new();
        for index in 0..count {
            let (mut stream, _) = listener.accept().unwrap();
            let handle = handle.clone();
            children.push(std::thread::spawn(move || {
                let headers = request(&mut stream);
                handle(index, &mut stream, &headers);
            }));
        }
        for child in children {
            child.join().unwrap();
        }
    });
    (url, thread)
}

#[test]
fn pna_matrix_ipv4_ipv6_and_mapped_denied_addresses() {
    let addresses = [
        ("8.8.8.8", AddressClass::Public),
        ("2001:4860::1", AddressClass::Public),
        ("10.0.0.1", AddressClass::Private),
        ("172.31.0.1", AddressClass::Private),
        ("192.168.1.1", AddressClass::Private),
        ("100.64.0.1", AddressClass::Private),
        ("100.127.255.254", AddressClass::Private),
        ("169.254.1.1", AddressClass::Private),
        ("fc00::1", AddressClass::Private),
        ("fdff::1", AddressClass::Private),
        ("fe80::1", AddressClass::Private),
        ("127.0.0.1", AddressClass::Local),
        ("::1", AddressClass::Local),
        ("198.18.0.1", AddressClass::Local),
        ("198.19.255.254", AddressClass::Local),
        ("0.0.0.0", AddressClass::Denied),
        ("::", AddressClass::Denied),
        ("224.0.0.1", AddressClass::Denied),
        ("ff02::1", AddressClass::Denied),
        ("255.255.255.255", AddressClass::Denied),
        ("::ffff:10.1.2.3", AddressClass::Private),
        ("::ffff:127.0.0.1", AddressClass::Local),
        ("::ffff:198.18.0.1", AddressClass::Local),
        ("::ffff:0.0.0.0", AddressClass::Denied),
        ("::ffff:224.0.0.1", AddressClass::Denied),
        ("::ffff:255.255.255.255", AddressClass::Denied),
    ];
    for (ip, expected) in addresses {
        let address = SocketAddr::new(ip.parse().unwrap(), 8080);
        assert_eq!(address_class(address.ip()), expected, "{ip}");
        for page in [
            AddressClass::Public,
            AddressClass::Private,
            AddressClass::Local,
        ] {
            let resolved = filtered_resolve(page, "image.example:8080", &|_| Ok(vec![address]));
            assert_eq!(
                resolved.is_ok(),
                expected != AddressClass::Denied && expected <= page,
                "page {page:?}, image {ip}"
            );
        }
    }
    for ip in [
        "198.17.255.255",
        "198.20.0.1",
        "100.63.255.255",
        "100.128.0.1",
        "172.32.0.1",
        "febf::1",
    ] {
        assert_eq!(
            address_class(ip.parse().unwrap()),
            if ip == "febf::1" {
                AddressClass::Private
            } else {
                AddressClass::Public
            }
        );
    }
}

#[test]
fn page_uses_bare_dns_once_outermost_and_failure_is_public() {
    let calls = AtomicUsize::new(0);
    let resolver = |_: &str| {
        calls.fetch_add(1, Ordering::Relaxed);
        Ok(vec![
            "10.0.0.1:80".parse().unwrap(),
            "8.8.8.8:80".parse().unwrap(),
        ])
    };
    assert_eq!(
        page_class("https://page.example/post", &resolver),
        AddressClass::Public
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        page_class("https://page.example/post", &|_| Ok(vec![
            "10.0.0.1:80".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap()
        ])),
        AddressClass::Private
    );
    assert_eq!(
        page_class("https://page.example/post", &|_| Err(io::Error::other(
            "DNS failed"
        ))),
        AddressClass::Public
    );
    assert_eq!(
        page_class("https://page.example/post", &|_| Ok(vec![
            "[::]:80".parse().unwrap()
        ])),
        AddressClass::Public
    );
    let filtered = filtered_resolve(AddressClass::Public, "image.example:80", &resolver).unwrap();
    assert_eq!(filtered, vec!["8.8.8.8:80".parse::<SocketAddr>().unwrap()]);
}

#[test]
fn localhost_is_fixed_loopback_without_dns_and_suffix_is_exact() {
    for host in ["localhost", "LOCALHOST.", "a.localhost", "a.b.localhost."] {
        assert!(localhost(host));
        assert_eq!(
            page_class(&format!("http://{host}:8080/page"), &|_| panic!(
                "localhost DNS"
            )),
            AddressClass::Local
        );
        let addresses = filtered_resolve(AddressClass::Local, &format!("{host}:8080"), &|_| {
            panic!("localhost DNS")
        })
        .unwrap();
        assert_eq!(addresses.len(), 2);
        assert!(
            addresses
                .iter()
                .all(|a| a.ip().is_loopback() && a.port() == 8080)
        );
        assert!(
            filtered_resolve(AddressClass::Public, &format!("{host}:8080"), &|_| panic!(
                "localhost DNS"
            ))
            .is_err()
        );
    }
    assert!(!localhost("localhost.example.com"));
    let calls = AtomicUsize::new(0);
    filtered_resolve(AddressClass::Public, "localhost.example.com:80", &|_| {
        calls.fetch_add(1, Ordering::Relaxed);
        Ok(vec!["8.8.8.8:80".parse().unwrap()])
    })
    .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn referer_strips_credentials_fragment_and_obeys_every_target_origin() {
    let page = "https://user:password@page.example/path?q=1#fragment";
    assert_eq!(
        referer(page, &Url::parse("https://page.example/image").unwrap()).as_deref(),
        Some("https://page.example/path?q=1")
    );
    assert_eq!(
        referer(page, &Url::parse("https://image.example/image").unwrap()).as_deref(),
        Some("https://page.example/")
    );
    assert_eq!(
        referer(page, &Url::parse("http://page.example/image").unwrap()),
        None
    );
    assert_eq!(
        referer(
            "http://page.example:8080/path",
            &Url::parse("https://page.example/image").unwrap()
        )
        .as_deref(),
        Some("http://page.example:8080/")
    );
    assert_eq!(
        referer(
            "http://page.example/path#fragment",
            &Url::parse("http://page.example/image").unwrap()
        )
        .as_deref(),
        Some("http://page.example/path")
    );
}

#[test]
fn url_checks_initial_inherited_and_redirect_userinfo_bad_ports() {
    for text in [
        "http://user:pw@image.example/a",
        "http://user@image.example/a",
        "http://image.example:25/a",
        "http://image.example:110/a",
        "http://image.example:6000/a",
        "ftp://image.example/a",
    ] {
        assert!(
            validate_http_url(&Url::parse(text).unwrap()).is_err(),
            "{text}"
        );
    }
    let inherited = Url::parse("http://user:pw@image.example/a")
        .unwrap()
        .join("../b")
        .unwrap();
    assert!(validate_http_url(&inherited).is_err());
    for port in [80, 443, 3000, 8000, 8080] {
        assert!(
            validate_http_url(&Url::parse(&format!("http://image.example:{port}/a")).unwrap())
                .is_ok()
        );
    }
}

#[test]
fn local_server_http_redirect_referer_failures_and_content_detection() {
    let (seen_tx, seen_rx) = mpsc::channel();
    let (url, server) = fixture(5, move |_, stream, headers| {
        seen_tx.send(headers.to_string()).unwrap();
        if headers.starts_with("GET /redirect ") {
            response(
                stream,
                "302 Found",
                "Location: /actual\r\nConnection: close\r\n",
                &[],
            );
        } else if headers.starts_with("GET /forbidden ") {
            response(stream, "403 Forbidden", "Connection: close\r\n", b"denied");
        } else if headers.starts_with("GET /html ") {
            response(
                stream,
                "200 OK",
                "Connection: close\r\n",
                b"<html>not an image</html>",
            );
        } else {
            response(
                stream,
                "200 OK",
                "Content-Type: text/html\r\nConnection: close\r\n",
                &png(),
            );
        }
    });
    let root = tempfile::tempdir().unwrap();
    let service = CaptureFetchService::new(root.path().to_owned()).unwrap();
    let session = service.start(
        format!("{url}/page?q=1#fragment"),
        vec![
            format!("{url}/redirect"),
            format!("{url}/image.jpg"),
            format!("{url}/forbidden"),
            format!("{url}/html"),
        ],
        CaptureDestination::Direct(root.path().join("output")),
        timestamp(),
        egui::Context::default(),
    );
    let results = collect(&session);
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 2);
    assert!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .any(|e| e.contains("403"))
    );
    assert!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .any(|e| e.contains("画像"))
    );
    for image in results.into_iter().filter_map(Result::ok) {
        assert_eq!(image.extension, "png");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.content_hash, <[u8; 32]>::from(Sha256::digest(png())));
    }
    server.join().unwrap();
    for headers in seen_rx.try_iter() {
        assert!(
            headers
                .to_ascii_lowercase()
                .contains(&format!("referer: {url}/page?q=1\r\n"))
        );
        assert!(!headers.contains("#fragment"));
        assert!(headers.contains(&format!("mImageViewer/{}", env!("CARGO_PKG_VERSION"))));
    }
}

fn direct_session(directory: &Path, page: &str) -> CaptureFetchSession {
    std::fs::create_dir_all(directory).unwrap();
    let (tx, rx) = bounded(BACKLOG);
    let (cancel_tx, cancel_rx) = bounded(0);
    let owner = Arc::new(Session {
        id: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
        directory: directory.to_owned(),
        page_url: page.into(),
        destination: CaptureDestination::Direct(directory.join("save")),
        timestamp: timestamp(),
        agent: OnceLock::new(),
        phase: Mutex::new(Phase::Fetching),
        cancelled: AtomicBool::new(false),
        cancel_sender: Mutex::new(Some(cancel_tx)),
        cancel_receiver: cancel_rx,
        results: tx,
        ctx: egui::Context::default(),
        pending: AtomicUsize::new(0),
        bytes: AtomicU64::new(0),
        queue: Weak::new(),
    });
    let class = page_class(page, &bare_resolve);
    owner
        .agent
        .set(
            ureq::AgentBuilder::new()
                .redirects(0)
                .try_proxy_from_env(false)
                .timeout_connect(Duration::from_secs(10))
                .resolver(move |netloc: &str| filtered_resolve(class, netloc, &bare_resolve))
                .build(),
        )
        .unwrap();
    CaptureFetchSession { owner, results: rx }
}

#[test]
fn redirects_revalidate_userinfo_port_and_private_destinations() {
    let (url, server) = fixture(2, |_, stream, headers| {
        let target = if headers.starts_with("GET /credentials ") {
            "http://user:pw@127.0.0.1:8080/image"
        } else {
            "http://127.0.0.1:25/image"
        };
        response(
            stream,
            "302 Found",
            &format!("Location: {target}\r\nConnection: close\r\n"),
            &[],
        );
    });
    let root = tempfile::tempdir().unwrap();
    let session = direct_session(&root.path().join("session"), &format!("{url}/page"));
    for (index, path) in ["credentials", "port"].into_iter().enumerate() {
        assert!(
            fetch_image(
                &session.owner,
                index,
                &format!("{url}/{path}"),
                DEADLINE,
                IMAGE_BYTES
            )
            .is_err()
        );
    }
    server.join().unwrap();
    assert!(filtered_resolve(AddressClass::Public, "localhost:8080", &bare_resolve).is_err());
}

#[test]
fn redirect_recomputes_cross_origin_referer_and_limits_hops() {
    let (seen_tx, seen_rx) = mpsc::channel();
    let (target, target_server) = fixture(1, move |_, stream, headers| {
        seen_tx.send(headers.to_string()).unwrap();
        response(stream, "200 OK", "Connection: close\r\n", &png());
    });
    let target = target.replace("127.0.0.1", "localhost");
    let (source, source_server) = fixture(1, move |_, stream, _| {
        response(
            stream,
            "302 Found",
            &format!("Location: {target}/image\r\nConnection: close\r\n"),
            &[],
        );
    });
    let root = tempfile::tempdir().unwrap();
    let session = direct_session(
        &root.path().join("session"),
        &format!("{source}/page?q=private#fragment"),
    );
    assert!(fetch_image(&session.owner, 0, &source, DEADLINE, IMAGE_BYTES).is_ok());
    let headers = seen_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .to_ascii_lowercase();
    assert!(headers.contains(&format!("referer: {source}/\r\n")));
    assert!(!headers.contains("q=private"));
    source_server.join().unwrap();
    target_server.join().unwrap();
    let (url, server) = fixture(6, |_, stream, _| {
        response(
            stream,
            "302 Found",
            "Location: /next\r\nConnection: close\r\n",
            &[],
        )
    });
    let session = direct_session(&root.path().join("limited"), &url);
    assert!(
        fetch_image(&session.owner, 0, &url, DEADLINE, IMAGE_BYTES)
            .unwrap_err()
            .contains("リダイレクト")
    );
    server.join().unwrap();
}

#[test]
fn absent_content_length_is_stream_limited_and_failed_files_release_budget() {
    let (url, server) = fixture(1, |_, stream, _| {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
            .unwrap();
        // This is the real 64 MiB production limit, with no Content-Length.
        let chunk = [7; 64 * 1024];
        for _ in 0..1025 {
            if stream.write_all(&chunk).is_err() {
                break;
            }
        }
    });
    let root = tempfile::tempdir().unwrap();
    let session = direct_session(&root.path().join("session"), &url);
    let error = fetch_image(&session.owner, 0, &url, DEADLINE, IMAGE_BYTES).unwrap_err();
    assert!(error.contains("64 MiB"), "{error}");
    assert_eq!(session.owner.bytes.load(Ordering::Acquire), 0);
    assert!(!session.owner.directory.join("0.image").exists());
    server.join().unwrap();
}

#[test]
fn total_deadline_covers_slow_headers_drip_bodies_and_redirect_remaining_time() {
    for route in ["headers", "drip", "redirect"] {
        let count = if route == "redirect" { 2 } else { 1 };
        let (url, server) = fixture(count, move |_, stream, headers| {
            if headers.starts_with("GET /redirect ") {
                std::thread::sleep(Duration::from_millis(150));
                response(
                    stream,
                    "302 Found",
                    "Location: /headers\r\nConnection: close\r\n",
                    &[],
                );
            } else if headers.starts_with("GET /drip ") {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                    .unwrap();
                for _ in 0..20 {
                    std::thread::sleep(Duration::from_millis(40));
                    if stream.write_all(b"x").is_err() {
                        break;
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(450));
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        });
        let root = tempfile::tempdir().unwrap();
        let session = direct_session(&root.path().join("session"), &url);
        let start = Instant::now();
        assert!(
            fetch_image(
                &session.owner,
                0,
                &format!("{url}/{route}"),
                Duration::from_millis(300),
                IMAGE_BYTES
            )
            .is_err()
        );
        assert!(
            start.elapsed() < Duration::from_millis(650),
            "deadline restarted at {route}: {:?}",
            start.elapsed()
        );
        server.join().unwrap();
    }
}

#[test]
fn oversized_png_header_is_rejected_without_decoder_fallback() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("oversized.image");
    let mut bytes = png();
    bytes[16..20].copy_from_slice(&40000u32.to_be_bytes());
    // PNG header probe does not require image data; the oversized header wins.
    std::fs::write(&path, bytes).unwrap();
    let error = decode_fetched(&path).unwrap_err();
    assert!(error.contains("大きすぎ"), "{error}");
    assert!(validate_dimensions(32768, 8193).is_err());
}

#[test]
fn image_codec_outside_miv_listing_formats_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("unsupported.image");
    let bytes = b"P6\n1 1\n255\n\xff\0\0";
    assert_eq!(image::guess_format(bytes).unwrap(), image::ImageFormat::Pnm);
    // Valid PPM decodes in image, but mIV's built-in listing does not recognize it.
    assert!(image::load_from_memory(bytes).is_ok());
    std::fs::write(&path, bytes).unwrap();
    assert!(
        decode_fetched(&path)
            .unwrap_err()
            .contains("対応する画像ではありません")
    );
}

#[test]
fn data_urls_use_streaming_percent_decode_and_session_budget_is_atomic() {
    let root = tempfile::tempdir().unwrap();
    let session = direct_session(&root.path().join("session"), "https://page.example/");
    let image = fetch_image(&session.owner, 0, &data_url(), DEADLINE, IMAGE_BYTES).unwrap();
    let percent = png()
        .iter()
        .map(|byte| format!("%{byte:02x}"))
        .collect::<String>();
    let other = fetch_image(
        &session.owner,
        1,
        &format!("data:image/png,{percent}"),
        DEADLINE,
        IMAGE_BYTES,
    )
    .unwrap();
    assert_eq!(image.content_hash, other.content_hash);
    assert_eq!(image.content_hash, <[u8; 32]>::from(Sha256::digest(png())));
    assert_eq!(std::fs::read(image.path).unwrap(), png());
    assert!(
        fetch_image(
            &session.owner,
            2,
            "data:text/html;base64,PGh0bWw+",
            DEADLINE,
            IMAGE_BYTES
        )
        .is_err()
    );
    assert!(
        fetch_image(
            &session.owner,
            3,
            "data:image/png,%GG",
            DEADLINE,
            IMAGE_BYTES
        )
        .is_err()
    );
    session
        .owner
        .bytes
        .store(SESSION_BYTES - 1, Ordering::Release);
    let successes = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| reserve_bytes(&session.owner, 1).is_ok()))
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .filter(|success| *success)
            .count()
    });
    assert_eq!(successes, 1);
    assert_eq!(session.owner.bytes.load(Ordering::Acquire), SESSION_BYTES);
    assert!(reserve_bytes(&session.owner, u64::MAX).is_err());
}

#[test]
fn bounded_backlog_cancel_unblocks_workers_and_cleanup_waits_for_fetch_jobs() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(0);
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let active_server = active.clone();
    let max_server = maximum.clone();
    let (url, server) = fixture(8, move |_, stream, _| {
        let count = active_server.fetch_add(1, Ordering::SeqCst) + 1;
        max_server.fetch_max(count, Ordering::SeqCst);
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        active_server.fetch_sub(1, Ordering::SeqCst);
        response(stream, "200 OK", "Connection: close\r\n", &png());
    });
    let root = tempfile::tempdir().unwrap();
    let service = CaptureFetchService::new(root.path().to_owned()).unwrap();
    let first = service.start(
        format!("{url}/page"),
        vec![url.clone(); 4],
        CaptureDestination::Direct(root.path().join("save")),
        timestamp(),
        egui::Context::default(),
    );
    for _ in 0..4 {
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    let directory = first.owner.directory.clone();
    assert!(directory.exists());
    drop(first);
    assert!(
        directory.exists(),
        "cancel must not remove files while a worker owns them"
    );
    let second = service.start(
        format!("{url}/page"),
        vec![url.clone(); 4],
        CaptureDestination::Direct(root.path().join("save")),
        timestamp(),
        egui::Context::default(),
    );
    assert!(entered_rx.recv_timeout(Duration::from_millis(100)).is_err());
    for _ in 0..4 {
        release_tx.send(()).unwrap();
    }
    for _ in 0..4 {
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    for _ in 0..4 {
        release_tx.send(()).unwrap();
    }
    assert_eq!(collect(&second).iter().filter(|r| r.is_ok()).count(), 4);
    server.join().unwrap();
    assert!(maximum.load(Ordering::SeqCst) <= 4);
    wait_until(|| !directory.exists());
    let backpressured = service.start(
        "https://page.example/".into(),
        vec![data_url(); 40],
        CaptureDestination::Direct(root.path().join("save")),
        timestamp(),
        egui::Context::default(),
    );
    wait_until(|| backpressured.results.len() == BACKLOG);
    let directory = backpressured.owner.directory.clone();
    drop(backpressured);
    wait_until(|| !directory.exists());
}

#[test]
fn separate_session_agents_cannot_reuse_private_connections() {
    // A successful first response is fully consumed into the Agent pool.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        response(&mut stream, "200 OK", "Connection: keep-alive\r\n", &png());
        ready_tx.send(()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut byte = [0];
        assert!(
            stream.read(&mut byte).unwrap_or(0) == 0,
            "public session reused the local session socket"
        );
    });
    let root = tempfile::tempdir().unwrap();
    let service = CaptureFetchService::new(root.path().to_owned()).unwrap();
    let local = service.start(
        format!("{url}/page"),
        vec![url.clone()],
        CaptureDestination::Direct(root.path().join("save")),
        timestamp(),
        egui::Context::default(),
    );
    assert!(collect(&local).pop().unwrap().is_ok());
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let public = service.start(
        "https://8.8.8.8/page".into(),
        vec![url],
        CaptureDestination::Direct(root.path().join("save")),
        timestamp(),
        egui::Context::default(),
    );
    assert!(collect(&public).pop().unwrap().is_err());
    server.join().unwrap();
}

#[test]
fn background_save_same_prefix_collision_direct_and_monthly_partial_failure() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("saved");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(
        output.join("20261002-153012-345-my-site.example_01.png"),
        b"existing",
    )
    .unwrap();
    let service = CaptureFetchService::new(root.path().join("data")).unwrap();
    let session = service.start(
        "https://my_site.example/page".into(),
        vec![data_url(); 125],
        CaptureDestination::Direct(output.clone()),
        timestamp(),
        egui::Context::default(),
    );
    let mut images: Vec<_> = collect(&session).into_iter().map(Result::unwrap).collect();
    // The selected-copy index is 3 digits. Put a collision at that width too.
    std::fs::write(
        output.join("20261002-153012-345-my-site.example_001.png"),
        b"existing",
    )
    .unwrap();
    std::fs::remove_file(&images[2].path).unwrap();
    images
        .iter_mut()
        .for_each(|image| image.thumbnail = egui::ColorImage::new([0, 0], Vec::new()));
    session.save(images).unwrap();
    let summary = loop {
        match session
            .results
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            CaptureFetchEvent::SaveComplete(summary) => break summary,
            CaptureFetchEvent::SaveProgress { total, .. } => assert_eq!(total, 125),
            event => panic!("unexpected {event:?}"),
        }
    };
    assert_eq!((summary.saved, summary.failed), (124, 1));
    assert!(!summary.errors.is_empty());
    for path in &summary.paths {
        assert_eq!(path.parent().unwrap(), output);
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("20261002-153012-345-2-my-site.example_")
        );
        let stem = path.file_stem().unwrap().to_str().unwrap();
        assert_eq!(
            crate::filename_stack::prefix_of(stem, '_'),
            "20261002-153012-345-2-my-site.example"
        );
    }
    assert!(
        summary
            .paths
            .iter()
            .all(|p| std::fs::read(p).unwrap() == png())
    );
    let directory = session.owner.directory.clone();
    drop(session);
    wait_until(|| !directory.exists());
    let monthly = service.start(
        "https://page.example/".into(),
        vec![data_url()],
        CaptureDestination::Monthly(Some(output.clone())),
        timestamp(),
        egui::Context::default(),
    );
    let images = collect(&monthly).into_iter().map(Result::unwrap).collect();
    monthly.save(images).unwrap();
    loop {
        if let CaptureFetchEvent::SaveComplete(summary) = monthly
            .results
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            assert_eq!(summary.saved, 1);
            assert_eq!(summary.paths[0].parent().unwrap(), output.join("2026-10"));
            break;
        }
    }
}

#[test]
fn startup_cleans_only_service_owned_tree_before_new_session_directory() {
    let root = tempfile::tempdir().unwrap();
    let orphan = root.path().join("tmp/clipboard-capture/orphan/0.image");
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, b"partial").unwrap();
    let keep = root.path().join("tmp/unrelated.txt");
    std::fs::write(&keep, b"keep").unwrap();
    let service = CaptureFetchService::new(root.path().to_owned()).unwrap();
    let session = service.start(
        "https://page.example/".into(),
        vec![data_url()],
        CaptureDestination::Direct(root.path().join("saved")),
        timestamp(),
        egui::Context::default(),
    );
    assert!(collect(&session).pop().unwrap().is_ok());
    assert!(!orphan.exists());
    assert!(keep.exists());
    assert!(session.owner.directory.exists());
}
