//! Opt-in worker-owned clipboard fixture. Unit tests never touch the OS clipboard/input.
use super::{
    RunnerBridge, TestScriptActionSelection, TestScriptWindowIdentity, UiCommand, UiSmokeAction,
    native_mouse_environment_error, rhai_error,
};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const SIZE: u32 = 200;

#[derive(Clone, Debug, PartialEq)]
enum Format {
    Named(&'static str),
    Standard(u32),
}
#[derive(Clone, Debug)]
struct Payload {
    format: Format,
    bytes: Vec<u8>,
}

fn unicode_text(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn cf_html(source: &str, fragment: &str) -> Vec<u8> {
    let prefix = "<html><body><!--StartFragment-->";
    let suffix = "<!--EndFragment--></body></html>";
    let header = |start: usize, end: usize, fs: usize, fe: usize| {
        format!(
            "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{fs:010}\r\nEndFragment:{fe:010}\r\nSourceURL:{source}\r\n"
        )
    };
    let start = header(0, 0, 0, 0).len();
    let fs = start + prefix.len();
    let fe = fs + fragment.len();
    let end = fe + suffix.len();
    let mut bytes = format!(
        "{}{}{}{}",
        header(start, end, fs, fe),
        prefix,
        fragment,
        suffix
    )
    .into_bytes();
    bytes.push(0);
    bytes
}

fn dib() -> Vec<u8> {
    // BITMAPINFOHEADER, positive height / bottom-up 32-bit BI_RGB, opaque blue.
    let mut bytes = vec![0; 40];
    bytes[0..4].copy_from_slice(&40_u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&SIZE.to_le_bytes());
    bytes[8..12].copy_from_slice(&SIZE.to_le_bytes());
    bytes[12..14].copy_from_slice(&1_u16.to_le_bytes());
    bytes[14..16].copy_from_slice(&32_u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&(SIZE * SIZE * 4).to_le_bytes());
    for _ in 0..SIZE * SIZE {
        bytes.extend([200, 80, 30, 255]);
    }
    bytes
}

fn hdrop(path: &Path) -> Vec<u8> {
    let mut bytes = vec![0; 20]; // DROPFILES
    bytes[0..4].copy_from_slice(&20_u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    bytes.extend(unicode_text(&path.to_string_lossy()));
    bytes.extend([0, 0]); // terminating empty path
    bytes
}

fn payloads(name: &str, origin: &str, png: &[u8], source: &Path) -> Result<Vec<Payload>, String> {
    let named = |format, bytes| Payload {
        format: Format::Named(format),
        bytes,
    };
    let standard = |format, bytes| Payload {
        format: Format::Standard(format),
        bytes,
    };
    let image_html = || {
        cf_html(
            &format!("{origin}/page.html"),
            "<img src=\"image.png\" width=\"200\" height=\"200\">",
        )
    };
    Ok(match name {
        "html" => vec![named("HTML Format", image_html())],
        "png" => vec![named("PNG", png.to_vec()), standard(8, dib())],
        "dib" => vec![standard(8, dib())],
        "hdrop" => vec![standard(15, hdrop(source))],
        "office" => {
            // The reader must reject solely by advertised Office formats, before opening.
            let mut descriptor = vec![0; 52];
            descriptor[0..4].copy_from_slice(&52_u32.to_le_bytes());
            vec![
                named("Embed Source", vec![0xd0, 0xcf, 0x11, 0xe0]),
                named("Object Descriptor", descriptor),
                named("XML Spreadsheet", unicode_text("<Workbook/>")),
                standard(8, dib()),
                named("HTML Format", image_html()),
            ]
        }
        "paragraph" => vec![
            named(
                "HTML Format",
                cf_html(
                    &format!("{origin}/paragraph.html"),
                    "<p>Clipboard smoke text only.</p>",
                ),
            ),
            standard(13, unicode_text("Clipboard smoke text only.")),
        ],
        _ => return Err(format!("unknown clipboard fixture: {name}")),
    })
}

fn read_http_request_line(stream: &mut std::net::TcpStream) -> std::io::Result<Vec<u8>> {
    // The listener polls for shutdown; each accepted connection instead uses
    // blocking reads bounded by the remaining request deadline.
    stream.set_nonblocking(false)?;
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut request = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || request.len() >= 4096 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP headers exceeded deadline/budget",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        let mut buffer = [0; 512];
        let available = (4096 - request.len()).min(buffer.len());
        let count = stream.read(&mut buffer[..available])?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "HTTP headers incomplete",
            ));
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            let line_end = request.windows(2).position(|w| w == b"\r\n").unwrap();
            request.truncate(line_end);
            return Ok(request);
        }
    }
}

struct Fixture {
    manual: PathBuf,
    captures: PathBuf,
    source: PathBuf,
    png: Vec<u8>,
    origin: String,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new(bridge: &RunnerBridge) -> Result<Self, String> {
        Self::new_at_folder(PathBuf::from(bridge.latest_snapshot()?.current_folder_path))
    }
    fn new_at_folder(path: PathBuf) -> Result<Self, String> {
        let manual = path
            .canonicalize()
            .map_err(|e| format!("clipboard fixture path: {e}"))?;
        let root = manual.parent().ok_or("clipboard fixture has no parent")?;
        let data = root
            .parent()
            .ok_or("clipboard fixture has no data directory")?;
        if manual.file_name().and_then(|n| n.to_str()) != Some("manual")
            || root.file_name().and_then(|n| n.to_str()) != Some("clipboard-capture")
            || !data.join(".disposable-smoke-data").is_file()
            || data
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                != Some("portable-smoke")
        {
            return Err("clipboard fixture requires exact disposable portable-smoke/data/clipboard-capture/manual".into());
        }
        let captures = root.join("captures");
        let source = root.join("source/shell-copy.png");
        let png = std::fs::read(manual.join("seed.png")).map_err(|e| e.to_string())?;
        if !source.is_file() || !captures.is_dir() {
            return Err("clipboard fixture siblings were not prepared".into());
        }
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let origin = format!(
            "http://{}",
            listener.local_addr().map_err(|e| e.to_string())?
        );
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let image = png.clone();
        let server = thread::Builder::new().name("clipboard-smoke-http".into()).spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                        if let Ok(request) = read_http_request_line(&mut stream) {
                            let (status, kind, body): (&str, &str, &[u8]) = if request.starts_with(b"GET /image.png ") {
                                ("200 OK", "image/png", &image)
                            } else if request.starts_with(b"GET /page.html ") {
                                ("200 OK", "text/html", b"<html><body><img src=\"image.png\"></body></html>")
                            } else if request.starts_with(b"GET /paragraph.html ") {
                                ("200 OK", "text/html", b"<html><body><p>Clipboard smoke text only.</p></body></html>")
                            } else { ("404 Not Found", "text/plain", b"not found") };
                            let header = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = stream.write_all(header.as_bytes());
                            let _ = stream.write_all(body);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        }).map_err(|e| e.to_string())?;
        Ok(Self {
            manual,
            captures,
            source,
            png,
            origin,
            stop,
            server: Some(server),
        })
    }

    fn content(&self, name: &str) -> Result<Vec<Payload>, String> {
        payloads(name, &self.origin, &self.png, &self.source)
    }

    fn files(&self) -> Result<Map, String> {
        fn collect(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
            for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_symlink() {
                    return Err("clipboard output contains a link".into());
                }
                if kind.is_dir() {
                    collect(&entry.path(), out)?;
                } else if kind.is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                {
                    out.push(entry.path());
                }
            }
            Ok(())
        }
        let mut manual = Vec::new();
        let mut captures = Vec::new();
        collect(&self.manual, &mut manual)?;
        collect(&self.captures, &mut captures)?;
        let matches = |paths: &[PathBuf]| -> Result<i64, String> {
            let mut count = 0;
            for path in paths {
                if std::fs::read(path).map_err(|e| e.to_string())? == self.png {
                    count += 1;
                }
            }
            Ok(count)
        };
        let mut map = Map::new();
        map.insert("manual_count".into(), (manual.len() as i64).into());
        map.insert("capture_count".into(), (captures.len() as i64).into());
        map.insert("manual_matching_png".into(), matches(&manual)?.into());
        map.insert("capture_matching_png".into(), matches(&captures)?.into());
        map.insert(
            "shell_copy_matches".into(),
            (self.manual.join("shell-copy.png").is_file()
                && std::fs::read(self.manual.join("shell-copy.png")).map_err(|e| e.to_string())?
                    == self.png)
                .into(),
        );
        map.insert(
            "manual_paths".into(),
            manual
                .iter()
                .map(|p| Dynamic::from(p.to_string_lossy().to_string()))
                .collect::<rhai::Array>()
                .into(),
        );
        map.insert(
            "capture_paths".into(),
            captures
                .iter()
                .map(|p| Dynamic::from(p.to_string_lossy().to_string()))
                .collect::<rhai::Array>()
                .into(),
        );
        Ok(map)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.server.take() {
            let _ = worker.join();
        }
    }
}

trait ClipboardWriter {
    fn open(&mut self) -> Result<(), String>;
    fn replace(&mut self, content: &[Payload]) -> Result<(), String>;
    fn close(&mut self) -> Result<(), String>;
    fn delay(&mut self, duration: Duration) -> Result<(), String>;
}

fn write_once(writer: &mut impl ClipboardWriter, content: &[Payload]) -> Result<(), String> {
    writer.open()?;
    let written = writer.replace(content);
    let closed = writer.close();
    written.and(closed)
}

fn two_phase_write(writer: &mut impl ClipboardWriter, content: &[Payload]) -> Result<(), String> {
    write_once(writer, content)?;
    writer.delay(Duration::from_millis(25))?;
    // No retry: a busy second OpenClipboard is precisely the regression under test.
    write_once(writer, content).map_err(|e| format!("Excel second Open/Set/Close failed: {e}"))
}

#[cfg(windows)]
struct NativeWriter<'a> {
    bridge: &'a RunnerBridge,
    hwnd: u64,
}

#[cfg(windows)]
impl ClipboardWriter for NativeWriter<'_> {
    fn open(&mut self) -> Result<(), String> {
        self.bridge.interrupt.check()?;
        unsafe {
            windows::Win32::System::DataExchange::OpenClipboard(Some(
                windows::Win32::Foundation::HWND(self.hwnd as usize as *mut _),
            ))
        }
        .map_err(|e| e.to_string())
    }
    fn replace(&mut self, content: &[Payload]) -> Result<(), String> {
        use windows::{
            Win32::{
                Foundation::{GlobalFree, HANDLE, HGLOBAL},
                System::{
                    DataExchange::{EmptyClipboard, RegisterClipboardFormatW, SetClipboardData},
                    Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
                },
            },
            core::PCWSTR,
        };
        unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
        for payload in content {
            let format = match payload.format {
                Format::Standard(value) => value,
                Format::Named(name) => {
                    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
                    let value = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
                    if value == 0 {
                        return Err("RegisterClipboardFormatW failed".into());
                    }
                    value
                }
            };
            let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, payload.bytes.len()) }
                .map_err(|e| e.to_string())?;
            let pointer = unsafe { GlobalLock(memory) };
            if pointer.is_null() {
                let _ = unsafe { GlobalFree(Some(memory)) };
                return Err("GlobalLock clipboard fixture failed".into());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    payload.bytes.as_ptr(),
                    pointer.cast::<u8>(),
                    payload.bytes.len(),
                );
                let _ = GlobalUnlock(memory);
            }
            if let Err(e) = unsafe { SetClipboardData(format, Some(HANDLE(memory.0))) } {
                let _ = unsafe { GlobalFree(Some(HGLOBAL(memory.0))) };
                return Err(e.to_string());
            }
            // Successful SetClipboardData transfers ownership to Windows.
        }
        Ok(())
    }
    fn close(&mut self) -> Result<(), String> {
        unsafe { windows::Win32::System::DataExchange::CloseClipboard() }.map_err(|e| e.to_string())
    }
    fn delay(&mut self, duration: Duration) -> Result<(), String> {
        super::wait_interruptibly(&self.bridge.interrupt, duration).map_err(|e| e.to_string())
    }
}

fn selected_root(bridge: &RunnerBridge) -> Result<TestScriptWindowIdentity, String> {
    match bridge.action_selection()? {
        TestScriptActionSelection::Targeted(owner @ TestScriptWindowIdentity::Root { .. }) => {
            Ok(owner)
        }
        _ => Err("clipboard smoke requires select_root()".into()),
    }
}

pub(super) fn register(engine: &mut Engine, bridge: RunnerBridge) {
    let fixture: Arc<Mutex<Option<Fixture>>> = Arc::new(Mutex::new(None));
    let monitor_bridge = bridge.clone();
    engine.register_fn(
        "clipboard_capture_smoke",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let action = match name.as_str() {
                "monitors_on" => UiSmokeAction::ClipboardMonitorsOn,
                "monitors_off" => UiSmokeAction::ClipboardMonitorsOff,
                _ => {
                    return Err(rhai_error(format!(
                        "unknown clipboard smoke action: {name}"
                    )));
                }
            };
            monitor_bridge
                .send(UiCommand::SmokeAction(action))
                .map_err(rhai_error)
        },
    );
    let files_fixture = Arc::clone(&fixture);
    let files_bridge = bridge.clone();
    engine.register_fn(
        "clipboard_files",
        move || -> Result<Map, Box<EvalAltResult>> {
            let mut session = files_fixture
                .lock()
                .map_err(|_| rhai_error("clipboard fixture lock poisoned"))?;
            if session.is_none() {
                *session = Some(Fixture::new(&files_bridge).map_err(rhai_error)?);
            }
            session.as_ref().unwrap().files().map_err(rhai_error)
        },
    );
    #[cfg(windows)]
    {
        let write_fixture = Arc::clone(&fixture);
        let write_bridge = bridge.clone();
        engine.register_fn(
            "clipboard_fixture",
            move |name: ImmutableString| -> Result<i64, Box<EvalAltResult>> {
                let owner = selected_root(&write_bridge).map_err(rhai_error)?;
                let mut session = write_fixture
                    .lock()
                    .map_err(|_| rhai_error("clipboard fixture lock poisoned"))?;
                if session.is_none() {
                    *session = Some(Fixture::new(&write_bridge).map_err(rhai_error)?);
                }
                let content = session
                    .as_ref()
                    .unwrap()
                    .content(name.as_str())
                    .map_err(rhai_error)?;
                write_once(
                    &mut NativeWriter {
                        bridge: &write_bridge,
                        hwnd: owner.hwnd(),
                    },
                    &content,
                )
                .map_err(|e| native_mouse_environment_error(&write_bridge, e))?;
                Ok(i64::from(unsafe {
                    windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
                }))
            },
        );
        let excel_bridge = bridge.clone();
        let excel_fixture = Arc::clone(&fixture);
        engine.register_fn(
            "clipboard_excel_write",
            move |repetitions: i64| -> Result<Map, Box<EvalAltResult>> {
                if !(1..=100).contains(&repetitions) {
                    return Err(rhai_error("clipboard Excel repetitions must be 1..100"));
                }
                let owner = selected_root(&excel_bridge).map_err(rhai_error)?;
                let mut session = excel_fixture
                    .lock()
                    .map_err(|_| rhai_error("clipboard fixture lock poisoned"))?;
                if session.is_none() {
                    *session = Some(Fixture::new(&excel_bridge).map_err(rhai_error)?);
                }
                let content = session
                    .as_ref()
                    .unwrap()
                    .content("office")
                    .map_err(rhai_error)?;
                let mut writer = NativeWriter {
                    bridge: &excel_bridge,
                    hwnd: owner.hwnd(),
                };
                for _ in 0..repetitions {
                    two_phase_write(&mut writer, &content)
                        .map_err(|e| native_mouse_environment_error(&excel_bridge, e))?;
                    writer
                        .delay(Duration::from_millis(100))
                        .map_err(rhai_error)?;
                }
                let mut result = Map::new();
                result.insert("successful_second_opens".into(), repetitions.into());
                result.insert(
                    "sequence".into(),
                    i64::from(unsafe {
                        windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
                    })
                    .into(),
                );
                Ok(result)
            },
        );
        let paste_bridge = bridge.clone();
        let key_pipe = Arc::new(Mutex::new(None));
        engine.register_fn(
            "clipboard_paste",
            move || -> Result<i64, Box<EvalAltResult>> {
                let mut client = key_pipe
                    .lock()
                    .map_err(|_| rhai_error("clipboard key pipe lock poisoned"))?;
                if client.is_none() {
                    *client = Some(
                        KeyPipeClient::connect(&paste_bridge)
                            .map_err(|e| native_mouse_environment_error(&paste_bridge, e))?,
                    );
                }
                real_paste(&paste_bridge, client.as_mut().unwrap())
                    .map_err(|e| native_mouse_environment_error(&paste_bridge, e))
            },
        );
    }
}

#[cfg(windows)]
struct KeyPipeClient {
    file: std::fs::File,
    session: [u8; 16],
    gesture: u64,
}

#[cfg(windows)]
impl KeyPipeClient {
    fn connect(bridge: &RunnerBridge) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        let name = std::env::var("MIV_UI_SMOKE_BUTTON_PIPE")
            .map_err(|_| "clipboard key helper pipe is missing")?;
        if name.is_empty() || name.len() > 160 || name.contains(['\\', '/']) {
            return Err("clipboard key helper pipe is invalid".into());
        }
        let session = uuid::Uuid::parse_str(
            &std::env::var("MIV_UI_SMOKE_BUTTON_SESSION")
                .map_err(|_| "clipboard key helper session is missing")?,
        )
        .map_err(|e| e.to_string())?
        .to_bytes_le();
        let server = std::env::var("MIV_UI_SMOKE_BUTTON_SERVER_PID")
            .map_err(|_| "clipboard key helper server is missing")?
            .parse::<u32>()
            .map_err(|e| e.to_string())?;
        if server == 0 {
            return Err("clipboard key helper server PID is zero".into());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let file = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!(r"\\.\pipe\{name}"))
            {
                Ok(file) => break file,
                Err(e) if Instant::now() >= deadline => {
                    return Err(format!("clipboard helper connection: {e}"));
                }
                Err(_) => super::wait_interruptibly(&bridge.interrupt, Duration::from_millis(10))
                    .map_err(|e| e.to_string())?,
            }
        };
        let mut actual = 0;
        unsafe {
            windows::Win32::System::Pipes::GetNamedPipeServerProcessId(
                windows::Win32::Foundation::HANDLE(file.as_raw_handle()),
                &mut actual,
            )
        }
        .map_err(|e| e.to_string())?;
        if actual != server {
            return Err("clipboard helper server PID authentication failed".into());
        }
        Ok(Self {
            file,
            session,
            gesture: 0,
        })
    }
    fn command(&mut self, kind: u32, owner: &TestScriptWindowIdentity) -> Result<(), String> {
        let mut frame = Vec::with_capacity(56);
        frame.extend(0x4b43564d_u32.to_le_bytes());
        frame.extend(kind.to_le_bytes());
        frame.extend(self.gesture.to_le_bytes());
        frame.extend(self.session);
        frame.extend(owner.hwnd().to_le_bytes());
        frame.extend(
            unsafe { windows::Win32::System::Threading::GetCurrentProcessId() }.to_le_bytes(),
        );
        frame.extend(0_u32.to_le_bytes());
        frame.extend(owner.backend_token().to_le_bytes());
        self.file.write_all(&frame).map_err(|e| e.to_string())?;
        let mut reply = [0; 16];
        self.file
            .read_exact(&mut reply)
            .map_err(|e| format!("clipboard helper reply: {e}"))?;
        if reply[0..4] != 0x4b43564d_u32.to_le_bytes()
            || reply[4..8] != [0; 4]
            || reply[8..16] != self.gesture.to_le_bytes()
        {
            return Err("clipboard helper reply identity/status mismatch".into());
        }
        Ok(())
    }
    fn begin(&mut self, owner: &TestScriptWindowIdentity) -> Result<(), String> {
        self.gesture = self
            .gesture
            .checked_add(1)
            .ok_or("clipboard gesture ID exhausted")?;
        self.command(1, owner)
    }
    fn release(&mut self, owner: &TestScriptWindowIdentity) -> Result<(), String> {
        self.command(2, owner)
    }
}

#[cfg(windows)]
fn real_paste(bridge: &RunnerBridge, client: &mut KeyPipeClient) -> Result<i64, String> {
    use windows::Win32::UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible},
    };
    use windows::Win32::{Foundation::HWND, System::Threading::GetCurrentProcessId};
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetThreadDesktop(thread: u32) -> *mut std::ffi::c_void;
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> *mut std::ffi::c_void;
        fn CloseDesktop(desktop: *mut std::ffi::c_void) -> i32;
        fn GetUserObjectInformationW(
            handle: *mut std::ffi::c_void,
            index: i32,
            info: *mut std::ffi::c_void,
            size: u32,
            needed: *mut u32,
        ) -> i32;
    }
    fn desktop_name(handle: *mut std::ffi::c_void) -> Result<Vec<u16>, String> {
        let mut name = vec![0_u16; 256];
        let mut needed = 0;
        if handle.is_null()
            || unsafe {
                GetUserObjectInformationW(
                    handle,
                    2,
                    name.as_mut_ptr().cast(),
                    (name.len() * 2) as u32,
                    &mut needed,
                )
            } == 0
        {
            return Err("clipboard input desktop name unavailable".into());
        }
        name.truncate((needed as usize) / 2);
        Ok(name)
    }
    let owner = selected_root(bridge)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    bridge.validate_selected_owner_fresh(&owner, deadline)?;
    let hwnd = HWND(owner.hwnd() as usize as *mut _);
    let validate = || -> Result<(), String> {
        bridge.validate_selected_owner_cached(&owner, deadline)?;
        let mut process = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process)) };
        if process != unsafe { GetCurrentProcessId() }
            || !unsafe { IsWindowVisible(hwnd).as_bool() }
            || unsafe { GetForegroundWindow() } != hwnd
        {
            return Err("clipboard paste exact root is not the visible foreground owner".into());
        }
        let input = unsafe { OpenInputDesktop(0, 0, 1) };
        if input.is_null() {
            return Err("clipboard paste cannot open input desktop".into());
        }
        let input_name = desktop_name(input);
        unsafe {
            CloseDesktop(input);
        }
        let expected_name = input_name?;
        if desktop_name(unsafe { GetThreadDesktop(thread) })? != expected_name
            || desktop_name(unsafe {
                GetThreadDesktop(windows::Win32::System::Threading::GetCurrentThreadId())
            })? != expected_name
        {
            return Err(
                "clipboard paste requires app, worker and input on the same desktop".into(),
            );
        }
        Ok(())
    };
    validate()?;
    for key in [VK_CONTROL, VK_V, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN] {
        if unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0 {
            return Err("clipboard paste requires released user modifiers and V".into());
        }
    }
    let initial = bridge.latest_snapshot()?.clipboard_capture.paste_count;
    client.begin(&owner)?;
    // The external owner holds keys until the real GetAsyncKeyState consumer ACK.
    let accepted = (|| -> Result<i64, String> {
        loop {
            validate()?;
            let count = bridge.latest_snapshot()?.clipboard_capture.paste_count;
            if count > initial {
                return Ok(count);
            }
            super::wait_interruptibly(&bridge.interrupt, Duration::from_millis(10))
                .map_err(|e| e.to_string())?;
        }
    })();
    // Always request release after successful Down, including App guard failure.
    // The external owner also cleans up on pipe disconnect, deadline, cancellation and death.
    let released = client.release(&owner);
    let count = accepted?;
    released?;
    let released_frame = bridge.latest_snapshot()?.snapshot_frame;
    while bridge.latest_snapshot()?.snapshot_frame <= released_frame + 1 {
        validate()?;
        (bridge.wake)();
        super::wait_interruptibly(&bridge.interrupt, Duration::from_millis(10))
            .map_err(|e| e.to_string())?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_capture_html_offsets_source_and_payload_combinations_are_real_formats() {
        let html = cf_html("http://127.0.0.1:1234/page.html", "<p>日本語</p>");
        let text = std::str::from_utf8(&html[..html.len() - 1]).unwrap();
        let offset = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key))
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        assert_eq!(
            &text[offset("StartFragment:")..offset("EndFragment:")],
            "<p>日本語</p>"
        );
        assert!(text[offset("StartHTML:")..offset("EndHTML:")].starts_with("<html>"));
        assert!(text.contains("SourceURL:http://127.0.0.1:1234/page.html"));
        let office = payloads(
            "office",
            "http://127.0.0.1:1",
            &[1],
            Path::new("C:/source.png"),
        )
        .unwrap();
        for required in [
            Format::Named("Embed Source"),
            Format::Named("Object Descriptor"),
            Format::Standard(8),
            Format::Named("HTML Format"),
        ] {
            assert!(office.iter().any(|p| p.format == required));
        }
        let text_only = payloads(
            "paragraph",
            "http://127.0.0.1:1",
            &[1],
            Path::new("C:/source.png"),
        )
        .unwrap();
        assert_eq!(text_only.len(), 2);
        assert!(
            text_only
                .iter()
                .all(|p| !matches!(p.format, Format::Standard(8) | Format::Named("PNG")))
        );
        let png = payloads(
            "png",
            "http://127.0.0.1:1",
            &[1, 2],
            Path::new("C:/source.png"),
        )
        .unwrap();
        assert_eq!(png[0].bytes, [1, 2]);
        let dib = dib();
        assert_eq!(dib.len(), 40 + SIZE as usize * SIZE as usize * 4);
        assert_eq!(&dib[14..16], &32_u16.to_le_bytes());
        let drop = hdrop(Path::new("C:/日本語.png"));
        assert_eq!(&drop[16..20], &1_u32.to_le_bytes());
        assert_eq!(
            &drop[20..],
            [unicode_text("C:/日本語.png"), vec![0, 0]].concat()
        );
        let parsed = crate::clipboard_capture::html::parse_cf_html(
            &payloads(
                "html",
                "http://127.0.0.1:1234",
                &[],
                Path::new("source.png"),
            )
            .unwrap()[0]
                .bytes,
        )
        .unwrap();
        assert_eq!(parsed.page_url, "http://127.0.0.1:1234/page.html");
        assert_eq!(parsed.candidates, ["http://127.0.0.1:1234/image.png"]);
        let paragraph = payloads(
            "paragraph",
            "http://127.0.0.1:1234",
            &[],
            Path::new("source.png"),
        )
        .unwrap();
        assert!(
            crate::clipboard_capture::html::parse_cf_html(&paragraph[0].bytes)
                .unwrap()
                .candidates
                .is_empty()
        );
        let decoded = crate::clipboard_capture::data::decode_image(
            &crate::clipboard_capture::data::RawClipboardData {
                dib: Some(dib),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (200, 200));
        let rgba = image::load_from_memory(&decoded.png).unwrap().to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [30, 80, 200, 255]);
    }

    #[test]
    fn clipboard_capture_loopback_http_handles_split_requests_and_joins_without_ui_or_clipboard() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("portable-smoke/data");
        let root = data.join("clipboard-capture");
        let manual = root.join("manual");
        for folder in [&manual, &root.join("source"), &root.join("captures")] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(data.join(".disposable-smoke-data"), b"test").unwrap();
        std::fs::write(manual.join("seed.png"), b"fixture-png").unwrap();
        std::fs::write(root.join("source/shell-copy.png"), b"fixture-png").unwrap();
        std::fs::write(manual.join(".miv-clipboard-test.part"), b"partial").unwrap();
        let fixture = Fixture::new_at_folder(manual).unwrap();
        assert_eq!(
            fixture.files().unwrap()["manual_count"].as_int().unwrap(),
            1
        );
        let mut client =
            std::net::TcpStream::connect(fixture.origin.strip_prefix("http://").unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.write_all(b"GET /ima").unwrap();
        thread::sleep(Duration::from_millis(5));
        client.write_all(b"ge.png HTTP/1.1\r\n").unwrap();
        thread::sleep(Duration::from_millis(5));
        client.write_all(b"Host: localhost\r\n\r\n").unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with(b"fixture-png"));
        let stopped = Arc::clone(&fixture.stop);
        drop(fixture); // joined server before temp fixture removal
        assert!(stopped.load(Ordering::Acquire));
    }

    #[derive(Default)]
    struct FakeWriter {
        steps: Vec<String>,
        second_busy: bool,
        set_fails: bool,
        opens: usize,
    }
    impl ClipboardWriter for FakeWriter {
        fn open(&mut self) -> Result<(), String> {
            self.opens += 1;
            self.steps.push("Open".into());
            if self.second_busy && self.opens == 2 {
                Err("busy".into())
            } else {
                Ok(())
            }
        }
        fn replace(&mut self, _: &[Payload]) -> Result<(), String> {
            self.steps.push("Set".into());
            if self.set_fails {
                Err("set failed".into())
            } else {
                Ok(())
            }
        }
        fn close(&mut self) -> Result<(), String> {
            self.steps.push("Close".into());
            Ok(())
        }
        fn delay(&mut self, duration: Duration) -> Result<(), String> {
            self.steps.push(format!("Delay{}", duration.as_millis()));
            Ok(())
        }
    }
    #[test]
    fn clipboard_capture_excel_two_phase_closes_before_25ms_and_never_retries_second_open() {
        let mut writer = FakeWriter::default();
        two_phase_write(&mut writer, &[]).unwrap();
        assert_eq!(
            writer.steps,
            ["Open", "Set", "Close", "Delay25", "Open", "Set", "Close"]
        );
        let mut writer = FakeWriter {
            second_busy: true,
            ..Default::default()
        };
        assert!(
            two_phase_write(&mut writer, &[])
                .unwrap_err()
                .contains("second")
        );
        assert_eq!(writer.steps, ["Open", "Set", "Close", "Delay25", "Open"]);
        let mut writer = FakeWriter {
            set_fails: true,
            ..Default::default()
        };
        assert!(two_phase_write(&mut writer, &[]).is_err());
        assert_eq!(writer.steps, ["Open", "Set", "Close"]);
    }
}
