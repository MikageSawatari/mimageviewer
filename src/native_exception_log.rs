//! Best-effort first-chance native exception evidence. Nothing on the handler
//! path may allocate, acquire Rust locks/TLS, or call CRT/COM/loader services.
//! Only startup opens/rotates the sink. Stack overflow on foreign threads has
//! no guaranteed stack reserve; recording and subsequent WER handling are not
//! guaranteed (fail-fast exceptions can bypass VEH altogether).

// Keep the formatter restricted to core and concrete primitive inputs. In
// particular, do not format std types via Debug or accept arbitrary Display.
mod line {
    use core::fmt::{self, Write};
    use miv_startup::{ProcessIdentity, Role};

    pub(super) const CAPACITY: usize = 2048;
    pub(super) const LIMIT_NOTE: &str =
        " (limit reached; further native exceptions are not recorded this session)";

    pub(super) struct Buffer<const N: usize> {
        bytes: [u8; N],
        len: usize,
    }

    impl<const N: usize> Buffer<N> {
        pub(super) const fn new() -> Self {
            Self {
                bytes: [0; N],
                len: 0,
            }
        }

        pub(super) fn as_bytes(&self) -> &[u8] {
            &self.bytes[..self.len]
        }

        fn finish(&mut self) {
            if N != 0 {
                self.bytes[self.len] = b'\n';
                self.len += 1;
            }
        }
    }

    impl<const N: usize> Write for Buffer<N> {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            // Reserve the last byte for a newline, including on truncation.
            let count = text.len().min(N.saturating_sub(1).saturating_sub(self.len));
            self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
            self.len += count;
            Ok(())
        }
    }

    pub(super) struct ExceptionLine {
        pub identity: Option<ProcessIdentity>,
        pub intervals: u64,
        pub code: u32,
        pub flags: u32,
        pub address: usize,
        pub thread: u32,
        pub number_parameters: u32,
        pub access: usize,
        pub target: usize,
        pub exe_base: usize,
        pub ntdll_base: usize,
        pub limit_reached: bool,
    }

    pub(super) fn format<const N: usize>(buffer: &mut Buffer<N>, input: &ExceptionLine) {
        buffer.len = 0;
        let _ = write!(
            buffer,
            "[SystemTime {{ intervals: {} }}] NATIVE EXCEPTION chance=first code=0x{:08X} flags=0x{:08X} address=0x{:x} thread={}",
            input.intervals, input.code, input.flags, input.address, input.thread
        );
        if input.code == 0xC000_0005 && input.number_parameters >= 2 {
            let access = match input.access {
                0 => "read",
                1 => "write",
                8 => "execute",
                _ => "unknown",
            };
            let _ = write!(buffer, " access={access} target=0x{:x}", input.target);
        }
        let _ = write!(
            buffer,
            " version={} exe_base=0x{:x} ntdll_base=0x{:x}",
            env!("CARGO_PKG_VERSION"),
            input.exe_base,
            input.ntdll_base
        );
        if let Some(identity) = input.identity {
            identity_fields(buffer, identity);
        }
        if input.limit_reached {
            let _ = buffer.write_str(LIMIT_NOTE);
        }
        buffer.finish();
    }

    fn identity_fields<const N: usize>(buffer: &mut Buffer<N>, identity: ProcessIdentity) {
        let role = match identity.role {
            Role::Core => "core",
            Role::Launcher => "launcher",
        };
        let _ = write!(
            buffer,
            " schema=1 run={} pid={} start_utc_ms={} start_ticks={} role={role}",
            identity.run, identity.pid, identity.started_utc_ms, identity.started_ticks
        );
    }

    pub(super) fn header<const N: usize>(buffer: &mut Buffer<N>, identity: ProcessIdentity) {
        buffer.len = 0;
        // Start a boundary even if a previous process left an incomplete line.
        let _ = buffer.write_str("\nNATIVE RUN");
        identity_fields(buffer, identity);
        buffer.finish();
    }
}

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use line::{Buffer, CAPACITY, ExceptionLine};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::IntoRawHandle;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_APPEND_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, WriteFile,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

const SESSION_LIMIT: u32 = 256;
const ROTATE_BYTES: u64 = 1024 * 1024;
static RECORDER: NativeExceptionRecorder = NativeExceptionRecorder::new();
static INSTALL_STARTED: AtomicBool = AtomicBool::new(false);

struct NativeExceptionRecorder {
    logging: AtomicBool,
    lines: AtomicU32,
    buffer: UnsafeCell<Buffer<CAPACITY>>,
    sink: AtomicPtr<core::ffi::c_void>,
    exe_base: AtomicUsize,
    ntdll_base: AtomicUsize,
    preparing: AtomicBool,
    identity: UnsafeCell<Option<miv_startup::ProcessIdentity>>,
}

// The buffer is accessed only between a successful CAS and its Release store.
// The preparation CAS permits one identity write, before publishing the sink
// with Release; record reads that immutable identity only after an Acquire load
// of a valid sink. Startup registers VEH after publication. The static lives
// until process exit; local test instances own independent sinks/buffers.
unsafe impl Sync for NativeExceptionRecorder {}

impl NativeExceptionRecorder {
    const fn new() -> Self {
        Self {
            logging: AtomicBool::new(false),
            lines: AtomicU32::new(0),
            buffer: UnsafeCell::new(Buffer::new()),
            sink: AtomicPtr::new(core::ptr::null_mut()),
            exe_base: AtomicUsize::new(0),
            ntdll_base: AtomicUsize::new(0),
            preparing: AtomicBool::new(false),
            identity: UnsafeCell::new(None),
        }
    }

    // Called once, before this instance is exposed to a handler.
    fn prepare(
        &self,
        log_dir: &Path,
        identity: Option<miv_startup::ProcessIdentity>,
    ) -> io::Result<()> {
        self.prepare_with_header(log_dir, identity, write_header)
    }

    fn prepare_with_header(
        &self,
        log_dir: &Path,
        identity: Option<miv_startup::ProcessIdentity>,
        write: impl FnOnce(HANDLE, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.preparing.swap(true, Ordering::AcqRel) {
            return Err(io::Error::other("native recorder already initialized"));
        }
        let sink = open_sink(log_dir)?;
        if let Some(identity) = identity {
            let mut header = Buffer::<CAPACITY>::new();
            line::header(&mut header, identity);
            if let Err(error) = write(sink, header.as_bytes()) {
                unsafe { CloseHandle(sink) };
                return Err(error);
            }
            // One initializer writes once, before the sink's Release publication.
            // A failed/short header never publishes a correlatable identity.
            unsafe { *self.identity.get() = Some(identity) };
        }
        self.exe_base.store(
            unsafe { GetModuleHandleW(core::ptr::null()) } as usize,
            Ordering::Relaxed,
        );
        self.ntdll_base.store(
            unsafe { GetModuleHandleW(windows_sys::w!("ntdll.dll")) } as usize,
            Ordering::Relaxed,
        );
        self.sink.store(sink, Ordering::Release);
        Ok(())
    }

    /// `info` and its exception record must be valid for the duration of this call.
    unsafe fn record(&self, info: *mut EXCEPTION_POINTERS) -> i32 {
        let Some(info) = (unsafe { info.as_ref() }) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let Some(record) = (unsafe { info.ExceptionRecord.as_ref() }) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let code = record.ExceptionCode as u32;
        if !matches!(
            code,
            0xC000_0005 // EXCEPTION_ACCESS_VIOLATION
            | 0xC000_00FD // EXCEPTION_STACK_OVERFLOW
            | 0x8000_0003 // EXCEPTION_BREAKPOINT
            | 0xC000_001D // EXCEPTION_ILLEGAL_INSTRUCTION
            | 0xC000_0094 // EXCEPTION_INT_DIVIDE_BY_ZERO
            | 0xC000_0095 // STATUS_INTEGER_OVERFLOW
            | 0xC000_0374 // STATUS_HEAP_CORRUPTION
        ) || self
            .logging
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }

        let sink = self.sink.load(Ordering::Acquire);
        if !sink.is_null() && sink != INVALID_HANDLE_VALUE {
            if let Ok(previous) = self
                .lines
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    (n < SESSION_LIMIT).then_some(n.saturating_add(1))
                })
            {
                let mut time = FILETIME {
                    dwLowDateTime: 0,
                    dwHighDateTime: 0,
                };
                unsafe { GetSystemTimePreciseAsFileTime(&mut time) };
                let (access, target) = if code == 0xC000_0005 && record.NumberParameters >= 2 {
                    (
                        record.ExceptionInformation[0],
                        record.ExceptionInformation[1],
                    )
                } else {
                    (0, 0)
                };
                // No stack capture, formatting allocations, or general logging here.
                let buffer = unsafe { &mut *self.buffer.get() };
                line::format(
                    buffer,
                    &ExceptionLine {
                        identity: unsafe { *self.identity.get() },
                        intervals: (u64::from(time.dwHighDateTime) << 32)
                            | u64::from(time.dwLowDateTime),
                        code,
                        flags: record.ExceptionFlags,
                        address: record.ExceptionAddress as usize,
                        thread: unsafe { GetCurrentThreadId() },
                        number_parameters: record.NumberParameters,
                        access,
                        target,
                        exe_base: self.exe_base.load(Ordering::Relaxed),
                        ntdll_base: self.ntdll_base.load(Ordering::Relaxed),
                        limit_reached: previous + 1 == SESSION_LIMIT,
                    },
                );
                let bytes = buffer.as_bytes();
                let mut written = 0;
                // One raw append attempt. No retry, flush, or error-reporting path
                // inside VEH, even if disk I/O fails or another exception occurs.
                unsafe {
                    WriteFile(
                        sink,
                        bytes.as_ptr(),
                        bytes.len() as u32,
                        &mut written,
                        core::ptr::null_mut(),
                    )
                };
            }
        }
        self.logging.store(false, Ordering::Release);
        EXCEPTION_CONTINUE_SEARCH
    }
}

impl Drop for NativeExceptionRecorder {
    fn drop(&mut self) {
        // Only local test instances drop; the production static owns its handle
        // for the entire process lifetime, without unregister/reopen races.
        let sink = *self.sink.get_mut();
        if !sink.is_null() && sink != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(sink) };
        }
    }
}

fn open_sink(log_dir: &Path) -> io::Result<HANDLE> {
    std::fs::create_dir_all(log_dir)?;
    let path = log_dir.join("native-exception.log");
    let size = match std::fs::metadata(&path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    // Simplification: rotate only before opening; never replace a live handle.
    // Rare failures stop initialization and use the existing diagnostic notice.
    if size > ROTATE_BYTES {
        let backup = log_dir.join("native-exception.log.bak");
        match std::fs::remove_file(&backup) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        std::fs::rename(&path, backup)?;
    }
    // Keep std's Windows long-path handling at startup. The recorder owns the
    // resulting handle; only the allocation-free raw write runs inside VEH.
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .access_mode(FILE_APPEND_DATA)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(&path)?;
    Ok(file.into_raw_handle())
}

fn write_header(sink: HANDLE, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0;
    let success = unsafe {
        WriteFile(
            sink,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
    if success == 0 {
        Err(io::Error::last_os_error())
    } else if written as usize != bytes.len() {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "incomplete native run header",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn install_in(
    log_dir: &Path,
    identity: Option<miv_startup::ProcessIdentity>,
) -> Result<(), String> {
    // Startup has one owner; duplicate entry points must never rotate/reopen a
    // live sink or register another handler. Initialization failures are not retried.
    if INSTALL_STARTED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    RECORDER.prepare(log_dir, identity).map_err(|error| {
        format!("native exception logger: native-exception.log initialization failed: {error}")
    })?;
    let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
    if handle.is_null() {
        return Err(format!(
            "native exception logger: AddVectoredExceptionHandler failed: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub(crate) fn install_background() {
    let log_dir = crate::data_dir::logs_dir();
    let identity = miv_startup::process_identity();
    if let Err(error) = std::thread::Builder::new()
        .name("native-exception-startup".into())
        .spawn(move || report_install_result(install_in(&log_dir, identity)))
    {
        report_install_result(Err(format!(
            "native exception logger: startup worker failed: {error}"
        )));
    }
}

#[cfg(test)]
pub(crate) fn install() -> Result<(), String> {
    install_in(
        &crate::data_dir::logs_dir(),
        miv_startup::process_identity(),
    )
}

pub(crate) fn report_install_result(result: Result<(), String>) {
    match result {
        Ok(()) => {
            crate::logger::log("native exception logger: installed (logs/native-exception.log)")
        }
        Err(error) => {
            crate::logger::log(&error);
            crate::log_writer::report_diagnostic_error(error);
        }
    }
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    unsafe { RECORDER.record(info) }
}

/// A first-chance observation, never evidence by itself of fatal termination.
/// Addresses, access targets and arbitrary local log text do not cross this API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeObservation {
    pub utc_ms: u64,
    pub code: u32,
}

/// Called on the diagnostics worker before this process opens/rotates its sink.
pub(crate) fn read_correlated(
    log_dir: &Path,
    expected: miv_startup::ProcessIdentity,
) -> io::Result<Vec<NativeObservation>> {
    let mut observations = Vec::new();
    for name in ["native-exception.log.bak", "native-exception.log"] {
        let mut file = match std::fs::File::open(log_dir.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        // A bounded tail includes the most recent complete session header. A
        // header outside the bound is intentionally unavailable, not inferred.
        let start = file.metadata()?.len().saturating_sub(ROTATE_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(ROTATE_BYTES).read_to_end(&mut bytes)?;
        collect_correlated(&bytes, start != 0, expected, &mut observations);
    }
    Ok(observations)
}

fn collect_correlated(
    bytes: &[u8],
    skip_first: bool,
    expected: miv_startup::ProcessIdentity,
    observations: &mut Vec<NativeObservation>,
) {
    let mut active = None;
    for (index, bytes) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        if (index == 0 && skip_first) || bytes.len() > CAPACITY || bytes.last() != Some(&b'\n') {
            active = None;
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes[..bytes.len() - 1]) else {
            active = None;
            continue;
        };
        if text.starts_with("NATIVE RUN") {
            // Any malformed/foreign boundary ends the previous active interval.
            active = text.strip_prefix("NATIVE RUN ").and_then(parse_identity);
        } else if active == Some(expected)
            && observations.len() < SESSION_LIMIT as usize
            && let Some(observation) = parse_observation(text, expected)
        {
            observations.push(observation);
        } else if !text.is_empty() && !text.starts_with("[SystemTime { intervals: ") {
            // Do not carry an interval across a damaged/partially recognizable
            // boundary (including leading whitespace or an interrupted marker).
            active = None;
        }
    }
}

fn decimal<T: std::str::FromStr>(text: &str) -> Option<T> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn parse_identity(text: &str) -> Option<miv_startup::ProcessIdentity> {
    let mut fields = text.split(' ');
    if fields.next()? != "schema=1" {
        return None;
    }
    let run = decimal(fields.next()?.strip_prefix("run=")?)?;
    let pid = decimal(fields.next()?.strip_prefix("pid=")?)?;
    let started_utc_ms = decimal(fields.next()?.strip_prefix("start_utc_ms=")?)?;
    let started_ticks = decimal(fields.next()?.strip_prefix("start_ticks=")?)?;
    let role = match fields.next()? {
        "role=core" => miv_startup::Role::Core,
        "role=launcher" => miv_startup::Role::Launcher,
        _ => return None,
    };
    if fields.next().is_some() {
        return None;
    }
    Some(miv_startup::ProcessIdentity {
        role,
        run,
        pid,
        started_utc_ms,
        started_ticks,
    })
}

fn hexadecimal(text: &str) -> Option<u64> {
    let text = text.strip_prefix("0x")?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(text, 16).ok()
}

fn parse_observation(
    text: &str,
    expected: miv_startup::ProcessIdentity,
) -> Option<NativeObservation> {
    let text = text.strip_suffix(line::LIMIT_NOTE).unwrap_or(text);
    let (detail, _) = text.rsplit_once(" schema=")?;
    let identity = &text[detail.len() + 1..];
    if parse_identity(identity)? != expected {
        return None;
    }
    let (intervals, detail) = detail
        .strip_prefix("[SystemTime { intervals: ")?
        .split_once(" }] NATIVE EXCEPTION chance=first ")?;
    let intervals: u64 = decimal(intervals)?;
    let mut fields = detail.split(' ');
    let code = u32::try_from(hexadecimal(fields.next()?.strip_prefix("code=")?)?).ok()?;
    if !matches!(
        code,
        0xC000_0005
            | 0xC000_00FD
            | 0x8000_0003
            | 0xC000_001D
            | 0xC000_0094
            | 0xC000_0095
            | 0xC000_0374
    ) {
        return None;
    }
    u32::try_from(hexadecimal(fields.next()?.strip_prefix("flags=")?)?).ok()?;
    hexadecimal(fields.next()?.strip_prefix("address=")?)?;
    decimal::<u32>(fields.next()?.strip_prefix("thread=")?)?;
    let mut next = fields.next()?;
    if let Some(access) = next.strip_prefix("access=") {
        if code != 0xC000_0005 || !matches!(access, "read" | "write" | "execute" | "unknown") {
            return None;
        }
        hexadecimal(fields.next()?.strip_prefix("target=")?)?;
        next = fields.next()?;
    }
    let version = next.strip_prefix("version=")?;
    if version.is_empty()
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        return None;
    }
    hexadecimal(fields.next()?.strip_prefix("exe_base=")?)?;
    hexadecimal(fields.next()?.strip_prefix("ntdll_base=")?)?;
    if fields.next().is_some() {
        return None;
    }
    Some(NativeObservation {
        utc_ms: intervals.checked_sub(116_444_736_000_000_000)? / 10_000,
        code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_RECORD, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };

    // Each fault child runs exactly one test in a fresh process. Even there,
    // leave the production static untouched, like the local synthetic fixtures.
    static CHILD_RECORDER: NativeExceptionRecorder = NativeExceptionRecorder::new();

    unsafe extern "system" fn child_handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        unsafe { CHILD_RECORDER.record(info) }
    }

    fn input() -> ExceptionLine {
        ExceptionLine {
            identity: None,
            intervals: 123456789,
            code: 0xC000_0005,
            flags: 2,
            address: 0x1234,
            thread: 42,
            number_parameters: 2,
            access: 0,
            target: 0x5678,
            exe_base: 0x1000,
            ntdll_base: 0x2000,
            limit_reached: false,
        }
    }

    fn formatted(input: &ExceptionLine) -> String {
        let mut buffer = Buffer::<CAPACITY>::new();
        line::format(&mut buffer, input);
        std::str::from_utf8(buffer.as_bytes()).unwrap().to_owned()
    }

    fn identity() -> miv_startup::ProcessIdentity {
        miv_startup::ProcessIdentity {
            role: miv_startup::Role::Core,
            run: 123,
            pid: 42,
            started_utc_ms: 1000,
            started_ticks: 456,
        }
    }

    fn header(identity: miv_startup::ProcessIdentity) -> String {
        let mut buffer = Buffer::<CAPACITY>::new();
        line::header(&mut buffer, identity);
        std::str::from_utf8(buffer.as_bytes()).unwrap().to_owned()
    }

    fn event(identity: miv_startup::ProcessIdentity, utc_ms: u64) -> String {
        let mut data = input();
        data.identity = Some(identity);
        data.intervals = 116_444_736_000_000_000 + utc_ms * 10_000;
        formatted(&data)
    }

    fn correlated(text: &str, expected: miv_startup::ProcessIdentity) -> Vec<NativeObservation> {
        let mut observations = Vec::new();
        collect_correlated(text.as_bytes(), false, expected, &mut observations);
        observations
    }

    #[test]
    fn native_exception_formatter_correlated_header_and_line() {
        let expected = identity();
        let fields = "schema=1 run=123 pid=42 start_utc_ms=1000 start_ticks=456 role=core";
        assert_eq!(header(expected), format!("\nNATIVE RUN {fields}\n"));
        let text = event(expected, 1234);
        assert!(text.ends_with(&format!(" {fields}\n")));
        assert!(text.contains("address=0x1234 thread=42 access=read target=0x5678"));
        assert_eq!(
            parse_observation(text.trim_end(), expected),
            Some(NativeObservation {
                utc_ms: 1234,
                code: 0xC000_0005,
            })
        );
        let mut other = expected;
        other.role = miv_startup::Role::Launcher;
        assert!(header(other).ends_with("role=launcher\n"));
        assert!(event(other, 1234).ends_with("role=launcher\n"));
        assert!(text.len() < CAPACITY);
    }

    #[test]
    fn native_exception_correlation_requires_header_and_every_identity_field() {
        let expected = identity();
        let observation = NativeObservation {
            utc_ms: 1234,
            code: 0xC000_0005,
        };
        assert!(correlated(&event(expected, 1234), expected).is_empty());
        assert!(
            correlated(
                &format!("{}{}", header(expected), formatted(&input())),
                expected
            )
            .is_empty()
        );
        for other in [
            miv_startup::ProcessIdentity {
                run: expected.run + 1,
                ..expected
            },
            miv_startup::ProcessIdentity {
                pid: expected.pid + 1,
                ..expected
            },
            miv_startup::ProcessIdentity {
                started_utc_ms: expected.started_utc_ms + 1,
                ..expected
            },
            miv_startup::ProcessIdentity {
                started_ticks: expected.started_ticks + 1,
                ..expected
            },
            miv_startup::ProcessIdentity {
                role: miv_startup::Role::Launcher,
                ..expected
            },
        ] {
            assert!(
                correlated(
                    &format!("{}{}", header(expected), event(other, 1234)),
                    expected
                )
                .is_empty()
            );
            assert!(
                correlated(
                    &format!("{}{}", header(other), event(expected, 1234)),
                    expected
                )
                .is_empty()
            );
            let text = format!(
                "{}{}{}{}{}",
                header(expected),
                event(expected, 1234),
                header(other),
                event(expected, 1234),
                event(other, 1234)
            );
            assert_eq!(correlated(&text, expected), vec![observation]);
        }
    }

    #[test]
    fn native_exception_correlation_two_starts_same_pid_different_ticks() {
        let first = identity();
        let second = miv_startup::ProcessIdentity {
            started_ticks: first.started_ticks + 1,
            ..first
        };
        let text = format!(
            "{}{}{}{}{}",
            header(first),
            event(first, 1234),
            header(second),
            event(first, 1235),
            event(second, 1236)
        );
        assert_eq!(
            correlated(&text, first),
            vec![NativeObservation {
                utc_ms: 1234,
                code: 0xC000_0005
            }]
        );
        assert_eq!(
            correlated(&text, second),
            vec![NativeObservation {
                utc_ms: 1236,
                code: 0xC000_0005
            }]
        );
    }

    #[test]
    fn native_exception_correlation_malformed_boundary_ends_interval() {
        let expected = identity();
        for boundary in [
            "NATIVE RUN\n".to_owned(),
            " NATIVE RUN malformed\n".to_owned(),
            "NATIVE R\n".to_owned(),
            header(expected).replace("schema=1", "schema=2"),
            header(expected).replace("start_ticks=456 ", ""),
            header(expected).replace("role=core", "role=worker"),
            header(expected).replace("pid=42", "pid=-42"),
            header(expected).replace("pid=42", "pid=4294967296"),
            header(expected).replace("run=123", "run=18446744073709551616"),
            header(expected).replace("role=core", "role=core extra=1"),
            header(expected).replace("role=core", "role=core role=core"),
            format!("NATIVE RUN {}\n", "x".repeat(CAPACITY)),
            "NATIVE RUN interrupted".to_owned(),
        ] {
            let text = format!(
                "{}{}{}{}",
                header(expected),
                event(expected, 1234),
                boundary,
                event(expected, 1235)
            );
            assert_eq!(
                correlated(&text, expected),
                vec![NativeObservation {
                    utc_ms: 1234,
                    code: 0xC000_0005
                }],
                "{boundary}"
            );
        }
    }

    #[test]
    fn native_exception_correlation_rejects_partial_and_malformed_events() {
        let expected = identity();
        let original = event(expected, 1234);
        for bad in [
            original.trim_end().to_owned(),
            original.replace("code=0xC0000005", "code=0xDEADBEEF"),
            original.replace("flags=0x00000002", "flags=bad"),
            original.replace("thread=42", "thread=oops"),
            original.replace("address=0x1234", "address=bad"),
            original.replace("target=0x5678", "target=bad"),
            original.replace("schema=1", "schema=2"),
            original.replace("role=core", "role=core extra=1"),
            original.replace("116444736012340000", "1"),
            format!("{}{}\n", original.trim_end(), "x".repeat(CAPACITY)),
        ] {
            assert!(
                correlated(&format!("{}{bad}", header(expected)), expected).is_empty(),
                "{bad}"
            );
        }
        let mut limited = input();
        limited.identity = Some(expected);
        limited.intervals = 116_444_736_012_340_000;
        limited.limit_reached = true;
        assert_eq!(
            correlated(
                &format!("{}{}", header(expected), formatted(&limited)),
                expected
            )
            .len(),
            1
        );
    }

    #[test]
    fn native_exception_header_failure_never_publishes_identity_or_sink() {
        for partial in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let recorder = NativeExceptionRecorder::new();
            let result =
                recorder.prepare_with_header(dir.path(), Some(identity()), |sink, bytes| {
                    if partial {
                        write_header(sink, &bytes[..bytes.len() / 2])?;
                    }
                    Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "injected header failure",
                    ))
                });
            assert!(result.is_err());
            assert!(recorder.sink.load(Ordering::Acquire).is_null());
            assert!(unsafe { *recorder.identity.get() }.is_none());
            record(&recorder, 0xC000_0005, 2);
            assert!(read_correlated(dir.path(), identity()).unwrap().is_empty());
            assert_eq!(recorder.lines.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn native_exception_local_recorder_publishes_successful_header_once() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = NativeExceptionRecorder::new();
        recorder.prepare(dir.path(), Some(identity())).unwrap();
        record(&recorder, 0xC000_0005, 2);
        let before = contents(dir.path());
        assert_eq!(before.matches("NATIVE RUN ").count(), 1);
        assert!(before.lines().last().unwrap().ends_with("role=core"));
        assert_eq!(read_correlated(dir.path(), identity()).unwrap().len(), 1);
        assert!(recorder.prepare(dir.path(), None).is_err());
        assert_eq!(contents(dir.path()), before);
    }

    #[test]
    fn native_exception_correlation_read_before_rotation_retains_previous_backup() {
        let dir = tempfile::tempdir().unwrap();
        let expected = identity();
        let previous = format!("{}{}", header(expected), event(expected, 1234));
        std::fs::write(dir.path().join("native-exception.log.bak"), previous).unwrap();
        let current = format!(
            "{}\n{}{}",
            "x".repeat(ROTATE_BYTES as usize + 1),
            header(expected),
            event(expected, 1235)
        );
        std::fs::write(dir.path().join("native-exception.log"), current).unwrap();
        let captured = read_correlated(dir.path(), expected).unwrap();
        assert_eq!(
            captured,
            vec![
                NativeObservation {
                    utc_ms: 1234,
                    code: 0xC000_0005
                },
                NativeObservation {
                    utc_ms: 1235,
                    code: 0xC000_0005
                }
            ]
        );
        let recorder = NativeExceptionRecorder::new();
        recorder
            .prepare(
                dir.path(),
                Some(miv_startup::ProcessIdentity {
                    run: 999,
                    ..expected
                }),
            )
            .unwrap();
        assert_eq!(
            read_correlated(dir.path(), expected).unwrap(),
            vec![captured[1]]
        );
        assert_eq!(captured.len(), 2);
    }

    #[test]
    fn native_exception_correlation_bounded_records_and_no_inferred_tail_header() {
        let dir = tempfile::tempdir().unwrap();
        let expected = identity();
        let text = format!(
            "{}{}",
            header(expected),
            event(expected, 1234).repeat(SESSION_LIMIT as usize + 10)
        );
        for name in ["native-exception.log", "native-exception.log.bak"] {
            std::fs::write(dir.path().join(name), &text).unwrap();
        }
        assert_eq!(
            read_correlated(dir.path(), expected).unwrap().len(),
            SESSION_LIMIT as usize
        );
        let text = format!(
            "{}{}\n{}",
            header(expected),
            "x".repeat(ROTATE_BYTES as usize),
            event(expected, 1234)
        );
        std::fs::write(dir.path().join("native-exception.log"), &text).unwrap();
        std::fs::remove_file(dir.path().join("native-exception.log.bak")).unwrap();
        assert!(read_correlated(dir.path(), expected).unwrap().is_empty());
    }

    #[test]
    fn native_exception_formatter_exact_line() {
        assert_eq!(
            formatted(&input()),
            concat!(
                "[SystemTime { intervals: 123456789 }] NATIVE EXCEPTION chance=first code=0xC0000005 flags=0x00000002 address=0x1234 thread=42 access=read target=0x5678 version=",
                env!("CARGO_PKG_VERSION"),
                " exe_base=0x1000 ntdll_base=0x2000\n"
            )
        );
    }

    #[test]
    fn native_exception_formatter_av_fields_require_two_parameters() {
        let mut data = input();
        for (value, name) in [(0, "read"), (1, "write"), (8, "execute"), (7, "unknown")] {
            data.access = value;
            assert!(formatted(&data).contains(&format!(" access={name} target=0x5678")));
        }
        for count in [0, 1] {
            data.number_parameters = count;
            let text = formatted(&data);
            assert!(!text.contains(" access="));
            assert!(!text.contains(" target="));
        }
        data.number_parameters = 2;
        data.code = 0xC000_00FD;
        let text = formatted(&data);
        assert!(!text.contains(" access="));
        assert!(!text.contains(" target="));
        assert!(text.contains("code=0xC00000FD"));
    }

    #[test]
    fn native_exception_formatter_truncates_and_reserves_newline() {
        let mut small = Buffer::<8>::new();
        small.write_str("12345678").unwrap();
        small.write_str("discarded").unwrap();
        assert_eq!(small.as_bytes(), b"1234567");
        line::format(&mut small, &input());
        assert_eq!(small.as_bytes(), b"[System\n");
        line::format(&mut small, &input()); // reuse resets the cursor
        assert_eq!(small.as_bytes(), b"[System\n");
        let mut one = Buffer::<1>::new();
        line::format(&mut one, &input());
        assert_eq!(one.as_bytes(), b"\n");
        let mut zero = Buffer::<0>::new();
        line::format(&mut zero, &input());
        assert!(zero.as_bytes().is_empty());
    }

    #[test]
    fn native_exception_formatter_marks_limit() {
        let mut data = input();
        data.limit_reached = true;
        assert_eq!(
            formatted(&data),
            format!("{}{}\n", formatted(&input()).trim_end(), line::LIMIT_NOTE)
        );
    }

    fn recorder(root: &Path) -> NativeExceptionRecorder {
        let recorder = NativeExceptionRecorder::new();
        recorder.prepare(root, None).unwrap();
        recorder
    }

    fn record(recorder: &NativeExceptionRecorder, code: u32, count: u32) {
        let mut record = EXCEPTION_RECORD {
            ExceptionCode: code as i32,
            NumberParameters: count,
            ExceptionAddress: 0x1234usize as _,
            ..Default::default()
        };
        record.ExceptionInformation[0] = 1;
        record.ExceptionInformation[1] = 0x5678;
        let mut info = EXCEPTION_POINTERS {
            ExceptionRecord: &mut record,
            ContextRecord: core::ptr::null_mut(),
        };
        assert_eq!(
            unsafe { recorder.record(&mut info) },
            EXCEPTION_CONTINUE_SEARCH
        );
    }

    fn contents(root: &Path) -> String {
        std::fs::read_to_string(root.join("native-exception.log")).unwrap()
    }

    #[test]
    fn native_exception_recorder_filters_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        assert_eq!(
            unsafe { recorder.record(core::ptr::null_mut()) },
            EXCEPTION_CONTINUE_SEARCH
        );
        let mut empty = EXCEPTION_POINTERS::default();
        assert_eq!(
            unsafe { recorder.record(&mut empty) },
            EXCEPTION_CONTINUE_SEARCH
        );
        record(&recorder, 0xE06D_7363, 2); // handled C++ exception: outside the set
        assert!(contents(dir.path()).is_empty());
        assert_eq!(recorder.lines.load(Ordering::Relaxed), 0);
        record(&recorder, 0xC000_0005, 2);
        record(&recorder, 0xC000_0005, 1);
        for code in [
            0xC000_00FD,
            0x8000_0003,
            0xC000_001D,
            0xC000_0094,
            0xC000_0095,
            0xC000_0374,
        ] {
            record(&recorder, code, 2);
        }
        let text = contents(dir.path());
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 8);
        assert!(lines[0].contains("access=write target=0x5678"));
        assert!(!lines[1].contains("access="));
        assert!(
            lines
                .iter()
                .all(|line| line.contains(&format!("thread={}", unsafe { GetCurrentThreadId() })))
        );
        assert!(lines.iter().all(|line| line.contains("chance=first")));
        assert!(!lines[0].contains("exe_base=0x0 "));
        assert!(!lines[0].ends_with("ntdll_base=0x0"));
    }

    #[test]
    fn native_exception_recorder_invalid_sink_and_reentry_do_not_count() {
        for handle in [core::ptr::null_mut(), INVALID_HANDLE_VALUE] {
            let recorder = NativeExceptionRecorder::new();
            recorder.sink.store(handle, Ordering::Relaxed);
            record(&recorder, 0xC000_0005, 2);
            assert_eq!(recorder.lines.load(Ordering::Relaxed), 0);
            assert!(!recorder.logging.load(Ordering::Relaxed));
        }
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder.logging.store(true, Ordering::Relaxed);
        record(&recorder, 0xC000_0005, 2);
        assert!(contents(dir.path()).is_empty());
        assert_eq!(recorder.lines.load(Ordering::Relaxed), 0);
        recorder.logging.store(false, Ordering::Release);
        record(&recorder, 0xC000_0005, 2);
        assert_eq!(contents(dir.path()).lines().count(), 1);
    }

    #[test]
    fn native_exception_recorder_caps_session_and_isolates_instances() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let first = recorder(a.path());
        let second = recorder(b.path());
        for _ in 0..SESSION_LIMIT + 3 {
            record(&first, 0xC000_0005, 2);
        }
        record(&second, 0xC000_0005, 2);
        let text = contents(a.path());
        assert_eq!(text.lines().count(), SESSION_LIMIT as usize);
        assert_eq!(text.matches(line::LIMIT_NOTE).count(), 1);
        assert!(text.lines().last().unwrap().ends_with(line::LIMIT_NOTE));
        assert_eq!(first.lines.load(Ordering::Relaxed), SESSION_LIMIT);
        assert_eq!(contents(b.path()).lines().count(), 1);
        assert!(!contents(b.path()).contains(line::LIMIT_NOTE));
    }

    #[test]
    fn native_exception_rotation_only_above_one_mib_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native-exception.log");
        let backup = dir.path().join("native-exception.log.bak");
        for length in [ROTATE_BYTES - 1, ROTATE_BYTES] {
            std::fs::File::create(&path)
                .unwrap()
                .set_len(length)
                .unwrap();
            let recorder = recorder(dir.path());
            record(&recorder, 0xC000_0005, 2);
            assert!(std::fs::metadata(&path).unwrap().len() > length);
            assert!(!backup.exists());
        }
        std::fs::File::create(&path)
            .unwrap()
            .set_len(ROTATE_BYTES + 1)
            .unwrap();
        std::fs::write(&backup, "old backup").unwrap();
        let recorder = recorder(dir.path());
        assert_eq!(std::fs::metadata(&backup).unwrap().len(), ROTATE_BYTES + 1);
        assert!(contents(dir.path()).is_empty());
        record(&recorder, 0xC000_0005, 2);
        assert_eq!(contents(dir.path()).lines().count(), 1);
    }

    #[test]
    fn native_exception_sink_supports_unprefixed_long_path_and_rotation() {
        use std::os::windows::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let mut log_dir = dir.path().to_path_buf();
        while log_dir
            .join("native-exception.log")
            .as_os_str()
            .encode_wide()
            .count()
            <= 320
        {
            log_dir.push("native-exception-long-path-segment");
        }
        assert!(log_dir.is_absolute());
        assert!(!log_dir.to_string_lossy().starts_with(r"\\?\"));
        std::fs::create_dir_all(&log_dir).unwrap();

        let path = log_dir.join("native-exception.log");
        let backup = log_dir.join("native-exception.log.bak");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(ROTATE_BYTES + 1)
            .unwrap();
        std::fs::write(&backup, "old backup").unwrap();

        let recorder = recorder(&log_dir);
        record(&recorder, 0xC000_0005, 2);
        let text = contents(&log_dir);
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("code=0xC0000005"));
        assert!(text.contains("access=write target=0x5678"));
        assert_eq!(std::fs::metadata(backup).unwrap().len(), ROTATE_BYTES + 1);
    }

    #[test]
    fn native_exception_sink_shares_read_write_delete() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        let path = dir.path().join("native-exception.log");
        let mut other = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .unwrap();
        other.write_all(b"external\n").unwrap();
        std::fs::rename(&path, dir.path().join("renamed.log")).unwrap();
        record(&recorder, 0xC000_0005, 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("renamed.log"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn native_exception_sink_startup_failure_preserves_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native-exception.log");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(ROTATE_BYTES + 1)
            .unwrap();
        std::fs::create_dir(dir.path().join("native-exception.log.bak")).unwrap();
        assert!(
            NativeExceptionRecorder::new()
                .prepare(dir.path(), None)
                .is_err()
        );
        assert_eq!(std::fs::metadata(path).unwrap().len(), ROTATE_BYTES + 1);
    }

    fn crash_child(mode: &str, test_name: &str, expected_code: u32) {
        const MODE: &str = "MIV_NATIVE_EXC_CHILD";
        const ROOT: &str = "MIV_NATIVE_EXC_ROOT";
        if std::env::var(MODE).ok().as_deref() == Some(mode) {
            unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
            let root = std::env::var_os(ROOT).unwrap();
            CHILD_RECORDER.prepare(Path::new(&root), None).unwrap();
            assert!(!unsafe { AddVectoredExceptionHandler(1, Some(child_handler)) }.is_null());
            if mode == "av" {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    core::arch::asm!("mov {value}, qword ptr [{address}]", address = in(reg) 0usize, value = out(reg) _, options(nostack));
                }
            } else {
                #[inline(never)]
                fn overflow(depth: usize) -> usize {
                    let pad = std::hint::black_box([0u8; 8192]);
                    if std::hint::black_box(depth) == usize::MAX {
                        return 0;
                    }
                    let value = overflow(depth + 1);
                    std::hint::black_box(&pad);
                    value + usize::from(pad[0])
                }
                std::thread::Builder::new()
                    .stack_size(128 * 1024)
                    .spawn(|| overflow(0))
                    .unwrap()
                    .join()
                    .unwrap();
            }
            panic!("fault instruction unexpectedly returned");
        }

        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env(MODE, mode)
            .env(ROOT, dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > Duration::from_secs(20) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("native exception {mode} child exceeded 20 seconds");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let text = contents(dir.path());
        assert_eq!(
            status.code().map(|code| code as u32),
            Some(expected_code),
            "status={status}; log={text}"
        );
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            text.contains(&format!("code=0x{expected_code:08X}")),
            "{text}"
        );
        assert!(text.contains(" thread="), "{text}");
        if mode == "av" {
            assert!(text.contains("access=read target=0x0"), "{text}");
        } else {
            assert!(!text.contains(" access="), "{text}");
        }
        eprintln!(
            "native exception {mode} child completed in {:?}",
            started.elapsed()
        );
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn native_exception_real_av_child() {
        crash_child(
            "av",
            "native_exception_log::tests::native_exception_real_av_child",
            0xC000_0005,
        );
    }

    #[test]
    fn native_exception_real_stack_overflow_child() {
        crash_child(
            "so",
            "native_exception_log::tests::native_exception_real_stack_overflow_child",
            0xC000_00FD,
        );
    }
}
