use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use positions::*;
use std::io::Cursor;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\noriginal\x00\xff\r\n";
fn view() -> View {
    View {
        screen_x: 100.0,
        screen_y: 50.0,
        outer_width: 1000.0,
        outer_height: 800.0,
        inner_width: 984.0,
        inner_height: 704.0,
        device_pixel_ratio: 1.0,
        context_event: None,
    }
}
fn window() -> BrowserWindow {
    BrowserWindow {
        left: 100.0,
        top: 50.0,
        width: 1000.0,
        height: 800.0,
        state: "normal".into(),
    }
}
fn request() -> protocol::Request {
    protocol::Request {
        v: 1,
        kind: "open_bytes".into(),
        data_b64: STANDARD.encode(PNG),
        content_type: "image/jpeg".into(),
        page_url: "https://user:secret@page.invalid/story#fragment".into(),
        image_url: "https://image.invalid/before.jpg".into(),
        final_url: "https://final.invalid/after?x=1#fragment".into(),
        view: view(),
        zoom: 1.0,
        window: window(),
    }
}
fn timestamp() -> CaptureTimestamp {
    CaptureTimestamp {
        stamp: "20261122-123456-789".into(),
        month: "2026-11".into(),
    }
}
fn config(root: &Path) -> Config {
    let launcher = root.join("launcher with space.exe");
    std::fs::write(&launcher, b"fixture").unwrap();
    Config {
        v: 1,
        launcher,
        output_dir: root.join("images"),
    }
}
#[derive(Default)]
struct Fake {
    motw_error: bool,
    launch_error: bool,
    motw: Vec<String>,
    args: Vec<std::ffi::OsString>,
}
impl Effects for Fake {
    fn motw(&mut self, _: &Path, origin: &CaptureOrigin) -> Result<(), ErrorCode> {
        self.motw
            .push(mimageviewer_capture_file::motw_contents(origin).unwrap());
        if self.motw_error {
            Err(ErrorCode::MotwFailed)
        } else {
            Ok(())
        }
    }
    fn launch(&mut self, command: &mut Command) -> Result<(), ErrorCode> {
        self.args = command.get_args().map(|value| value.to_owned()).collect();
        if self.launch_error {
            Err(ErrorCode::LaunchFailed)
        } else {
            Ok(())
        }
    }
}
fn run(config: &Config, managed: &Path, request: protocol::Request, fake: &mut Fake) -> Response {
    handle(config, managed, request, &timestamp(), fake)
}

#[test]
fn browser818_host_original_bytes_month_filename_motw_and_exact_command() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let mut fake = Fake::default();
    let response = run(
        &config,
        &temp.path().join("browser-host"),
        request(),
        &mut fake,
    );
    assert!(response.ok);
    let path = response.path.unwrap();
    assert_eq!(
        path,
        config
            .output_dir
            .join("2026-11/20261122-123456-789-page.invalid_01.png")
    );
    assert_eq!(std::fs::read(&path).unwrap(), PNG);
    assert_eq!(
        fake.motw,
        vec![
            "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://page.invalid/story\r\nHostUrl=https://final.invalid/after?x=1\r\n"
        ]
    );
    assert_eq!(
        fake.args,
        vec![
            "--browser-view".into(),
            "108,138,1092,842,0".into(),
            "--".into(),
            path.into_os_string()
        ]
    );
}
#[test]
fn browser818_host_collisions_never_overwrite_and_failures_keep_saved_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let managed = temp.path().join("browser-host");
    let mut paths = Vec::new();
    for (motw_error, launch_error, expected) in [
        (false, false, None),
        (true, false, Some(ErrorCode::MotwFailed)),
        (false, true, Some(ErrorCode::LaunchFailed)),
    ] {
        let mut fake = Fake {
            motw_error,
            launch_error,
            ..Default::default()
        };
        let response = run(&config, &managed, request(), &mut fake);
        assert_eq!(response.error, expected);
        assert_eq!(response.ok, expected.is_none());
        let path = response.path.unwrap();
        assert!(!paths.contains(&path));
        assert_eq!(std::fs::read(&path).unwrap(), PNG);
        paths.push(path);
        if motw_error {
            assert!(fake.args.is_empty());
        }
    }
    assert!(
        paths[1]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("-2-page.invalid_01.png")
    );
    for path in paths {
        assert_eq!(std::fs::read(path).unwrap(), PNG);
    }
}
#[test]
fn browser818_host_managed_destination_is_rejected_before_directory_creation() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = config(temp.path());
    let managed = temp.path().join("browser-host");
    for output in [managed.clone(), managed.join("images/new")] {
        config.output_dir = output;
        let response = run(&config, &managed, request(), &mut Fake::default());
        assert_eq!(response.error, Some(ErrorCode::InvalidDestination));
        assert!(!managed.exists());
    }
}
#[test]
fn browser818_host_version_kind_urls_base64_and_image_validation() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let managed = temp.path().join("browser-host");
    for field in 0..8 {
        let mut input = request();
        let expected = match field {
            0 => {
                input.v = 2;
                ErrorCode::UnsupportedVersion
            }
            1 => {
                input.kind = "download".into();
                ErrorCode::InvalidRequest
            }
            2 => {
                input.page_url = "file:///x".into();
                ErrorCode::InvalidUrl
            }
            3 => {
                input.image_url = "data:image/png,x".into();
                ErrorCode::InvalidUrl
            }
            4 => {
                input.final_url = "https://ok.invalid/\r\nZoneId=0".into();
                ErrorCode::InvalidUrl
            }
            5 => {
                input.data_b64 = "!!!!".into();
                ErrorCode::InvalidImage
            }
            6 => {
                input.data_b64 = STANDARD.encode(b"<html>not an image</html>");
                ErrorCode::InvalidImage
            }
            _ => {
                input.view.inner_width = 0.0;
                ErrorCode::InvalidPlacement
            }
        };
        assert_eq!(
            run(&config, &managed, input, &mut Fake::default()).error,
            Some(expected)
        );
    }
    assert!(!config.output_dir.exists());
}
#[test]
fn browser818_host_signature_formats_ignore_mime_and_reject_truncated_avif() {
    for (bytes, expected) in [
        (PNG, "png"),
        (&b"\xff\xd8\xff"[..], "jpg"),
        (&b"GIF89a"[..], "gif"),
        (&b"BM"[..], "bmp"),
        (&b"RIFFxxxxWEBP"[..], "webp"),
        (&b"\0\0\0\x14ftypavif\0\0\0\0avif"[..], "avif"),
    ] {
        assert_eq!(image_bytes::extension(bytes).unwrap(), expected);
    }
    assert_eq!(
        image_bytes::extension(b"\0\0\0\xffftypavif\0\0\0\0"),
        Err(ErrorCode::InvalidImage)
    );
    for value in ["", "Zg=", "Zh==", "Zm9v\n", "!!!!"] {
        assert!(image_bytes::decode(value).is_err());
    }
}

#[test]
fn browser818_host_frame_bounds_are_checked_before_body_read() {
    assert!(protocol::validate_length(protocol::MAX_FRAME_BYTES).is_ok());
    for size in [0, protocol::MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        let mut reader = Cursor::new(size.to_le_bytes());
        assert!(matches!(
            protocol::read_request(&mut reader),
            Err(ErrorCode::MessageTooLarge)
        ));
        assert_eq!(reader.position(), 4);
    }
    assert!(protocol::read_request(&mut Cursor::new([1, 0])).is_err());
    assert!(protocol::read_request(&mut Cursor::new([5, 0, 0, 0, b'{', b'}'])).is_err());
    let mut output = Vec::new();
    let oversized = Response::failure(
        ErrorCode::LaunchFailed,
        Some(PathBuf::from("x".repeat(protocol::MAX_RESPONSE_BYTES))),
    );
    assert!(protocol::write_response(&mut output, &oversized).is_err());
    assert!(output.is_empty()); // Reject before writing the response header too.
}
struct Short<T>(T);
impl<T: Read> Read for Short<T> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let len = out.len().min(1);
        self.0.read(&mut out[..len])
    }
}
impl<T: Write> Write for Short<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(&bytes[..bytes.len().min(2)])
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
#[test]
fn browser818_host_short_io_origins_and_missing_registration_have_framed_responses() {
    let temp = tempfile::tempdir().unwrap();
    let exe = temp.path().join("browser-host/generation/host.exe");
    let body = serde_json::to_vec(&request()).unwrap();
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend(body);
    let parsed = protocol::read_request(&mut Short(Cursor::new(frame.clone()))).unwrap();
    assert_eq!(parsed.data_b64, request().data_b64);
    for origin in [
        None,
        Some("chrome-extension://wrong/"),
        Some(protocol::ORIGINS[0]),
        Some(protocol::ORIGINS[1]),
    ] {
        let mut out = Short(Vec::new());
        run_one(
            origin,
            &exe,
            &mut Short(Cursor::new(&frame)),
            &mut out,
            &mut Fake::default(),
        )
        .unwrap();
        let size = u32::from_le_bytes(out.0[..4].try_into().unwrap()) as usize;
        assert_eq!(size, out.0.len() - 4);
        let response: serde_json::Value = serde_json::from_slice(&out.0[4..]).unwrap();
        let expected = if origin.is_some_and(|v| protocol::ORIGINS.contains(&v)) {
            "registration_missing"
        } else {
            "invalid_origin"
        };
        assert_eq!(response["error"], expected);
        assert!(
            !std::str::from_utf8(&out.0[4..])
                .unwrap()
                .contains(&request().data_b64)
        );
    }
}
#[test]
fn browser818_host_config_version_and_launcher_missing_are_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let exe = temp.path().join("browser-host/gen/host.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let mut config = config(temp.path());
    std::fs::write(
        exe.with_file_name("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let (loaded, managed) = read_config(&exe).unwrap();
    assert_eq!(managed, temp.path().join("browser-host"));
    let mut boundary = serde_json::to_vec(&config).unwrap();
    boundary.resize(65_536, b' ');
    std::fs::write(exe.with_file_name("config.json"), &boundary).unwrap();
    assert!(read_config(&exe).is_ok());
    boundary.push(b' ');
    std::fs::write(exe.with_file_name("config.json"), &boundary).unwrap();
    assert!(matches!(
        read_config(&exe),
        Err(ErrorCode::RegistrationMissing)
    ));
    std::fs::remove_file(&config.launcher).unwrap();
    assert_eq!(
        run(&loaded, &managed, request(), &mut Fake::default()).error,
        Some(ErrorCode::LauncherMissing)
    );
    config.v = 2;
    std::fs::write(
        exe.with_file_name("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        read_config(&exe),
        Err(ErrorCode::RegistrationMissing)
    ));
}
#[test]
fn browser818_host_one_packet_config_to_saved_response_with_unicode_path() {
    let temp = tempfile::tempdir().unwrap();
    let exe = temp.path().join("browser-host/generation/host.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let mut config = config(temp.path());
    config.output_dir = temp.path().join("画像 with space");
    std::fs::write(
        exe.with_file_name("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let body = serde_json::to_vec(&request()).unwrap();
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend(body);
    // Exactly one request is consumed, even if another frame follows it.
    let first_frame_len = frame.len();
    frame.extend_from_within(..);
    let mut input = Cursor::new(frame);
    let mut output = Short(Vec::new());
    let mut fake = Fake::default();
    run_one(
        Some(protocol::ORIGINS[1]),
        &exe,
        &mut input,
        &mut output,
        &mut fake,
    )
    .unwrap();
    assert_eq!(input.position() as usize, first_frame_len);
    let response: serde_json::Value = serde_json::from_slice(&output.0[4..]).unwrap();
    assert_eq!(response["ok"], true);
    assert!(response.get("error").is_none());
    let path = PathBuf::from(response["path"].as_str().unwrap());
    assert!(path.starts_with(&config.output_dir));
    assert_eq!(std::fs::read(&path).unwrap(), PNG);
    assert_eq!(fake.args.last(), Some(&path.into_os_string()));
    assert_eq!(fake.motw.len(), 1);
}
#[cfg(windows)]
#[test]
fn browser818_host_native_motw_uses_final_url_and_sanitized_page_url() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let mut native = NativeEffects;
    struct MotwOnly<'a>(&'a mut NativeEffects);
    impl Effects for MotwOnly<'_> {
        fn motw(&mut self, path: &Path, origin: &CaptureOrigin) -> Result<(), ErrorCode> {
            self.0.motw(path, origin)
        }
        fn launch(&mut self, _: &mut Command) -> Result<(), ErrorCode> {
            Ok(())
        }
    }
    let response = handle(
        &config,
        &temp.path().join("browser-host"),
        request(),
        &timestamp(),
        &mut MotwOnly(&mut native),
    );
    assert!(response.ok);
    let path = response.path.unwrap();
    let ads = std::fs::read_to_string(format!("{}:Zone.Identifier", path.display())).unwrap();
    assert_eq!(
        ads,
        "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://page.invalid/story\r\nHostUrl=https://final.invalid/after?x=1\r\n"
    );
    assert_eq!(std::fs::read(path).unwrap(), PNG);
}
#[test]
fn browser818_positions_event_zoom_dpr_negative_and_stale_geometry() {
    let mut v = view();
    let w = window();
    let estimate = calculate(&v, 1.0, &w).unwrap();
    assert_eq!(estimate.cli_value(), "108,138,1092,842,0");
    v.device_pixel_ratio = 1.875;
    v.inner_width /= 1.25;
    v.inner_height /= 1.25;
    assert_eq!(
        calculate(&v, 1.25, &w).unwrap().cli_value(),
        "162,207,1638,1263,0"
    );
    v.context_event = Some(ContextEvent {
        screen_x: 1200.0,
        screen_y: 400.0,
        client_x: 250.0,
        client_y: 263.0,
        window_screen_x: v.screen_x,
        window_screen_y: v.screen_y,
        inner_width: v.inner_width,
        inner_height: v.inner_height,
        device_pixel_ratio: v.device_pixel_ratio,
    });
    assert_eq!(
        calculate(&v, 1.25, &w).unwrap().cli_value(),
        "1331,107,2807,1163,0"
    );
    for field in 0..4 {
        let mut stale = v.clone();
        match field {
            0 => stale.screen_x += 1.0,
            1 => stale.inner_width += 1.0,
            2 => stale.inner_height += 1.0,
            _ => stale.device_pixel_ratio += 0.25,
        };
        let with_stale = calculate(&stale, 1.25, &w).unwrap();
        stale.context_event = None;
        assert_eq!(with_stale, calculate(&stale, 1.25, &w).unwrap());
    }
    let mut negative = view();
    negative.screen_x = -1900.0;
    assert!(calculate(&negative, 1.0, &w).unwrap().left < 0);
    let mut maximized = window();
    maximized.state = "maximized".into();
    let overlay = calculate(&negative, 1.0, &maximized).unwrap();
    assert!(!overlay.maximized);
    assert_eq!(overlay, calculate(&negative, 1.0, &window()).unwrap());
}
#[test]
fn browser818_positions_rejects_nonfinite_overflow_and_nonpositive_dimensions() {
    for invalid in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        assert!(calculate(&view(), invalid, &window()).is_err());
    }
    let mut v = view();
    v.screen_x = f64::MAX;
    assert!(calculate(&v, 1.0, &window()).is_err());
    v = view();
    v.inner_height = 0.0;
    assert!(calculate(&v, 1.0, &window()).is_err());
    let mut w = window();
    w.width = -1.0;
    assert!(calculate(&view(), 1.0, &w).is_err());
    w = window();
    w.top = f64::NAN;
    assert!(calculate(&view(), 1.0, &w).is_err());
    v = view();
    v.context_event = Some(ContextEvent {
        screen_x: f64::INFINITY,
        screen_y: 400.0,
        client_x: 250.0,
        client_y: 263.0,
        window_screen_x: v.screen_x,
        window_screen_y: v.screen_y,
        inner_width: v.inner_width,
        inner_height: v.inner_height,
        device_pixel_ratio: v.device_pixel_ratio,
    });
    assert!(calculate(&v, 1.0, &window()).is_err());
}
