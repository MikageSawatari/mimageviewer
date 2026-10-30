//! Minimal panic evidence, independent of the ordinary checkpoint/device sinks.
//! Preparation and reading belong to a normal worker. The hook only attempts a
//! bounded fixed-buffer raw append to an already-open process-lifetime handle.
use super::checkpoint::Stamp;
use super::{RunIdentity, RunRole};
use core::cell::UnsafeCell;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_APPEND_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, WriteFile,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

const FILE_NAME: &str = "panic-events.log";
const LIMIT: u32 = 16;
const CAPACITY: usize = 384;
const MAX_BYTES: u64 = LIMIT as u64 * CAPACITY as u64;
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
static RECORDER: OnceLock<Recorder<File>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PanicKind {
    Rust,
    GraphicsOutOfMemory,
    GraphicsValidation,
    GraphicsInternal,
}
impl PanicKind {
    const fn code(self) -> u64 {
        match self {
            Self::Rust => 1,
            Self::GraphicsOutOfMemory => 2,
            Self::GraphicsValidation => 3,
            Self::GraphicsInternal => 4,
        }
    }
    const fn from_code(code: u64) -> Option<Self> {
        match code {
            1 => Some(Self::Rust),
            2 => Some(Self::GraphicsOutOfMemory),
            3 => Some(Self::GraphicsValidation),
            4 => Some(Self::GraphicsInternal),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PanicObservation {
    pub stamp: Stamp,
    pub thread: u32,
    pub kind: PanicKind,
}

/// Called once on the diagnostics worker, with this run's private directory.
/// Once published, neither shutdown nor another prepare closes/replaces it.
pub(super) fn prepare(directory: &Path, identity: RunIdentity, origin: Instant) -> io::Result<()> {
    if RECORDER.get().is_some() {
        return Ok(());
    }
    let clock = Clock::prepare(origin)?;
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .access_mode(FILE_APPEND_DATA)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(directory.join(FILE_NAME))?;
    // Only preparation participates in initialization. A hook uses get(), which
    // never waits for this worker or initializes a value itself.
    let _ = RECORDER.set(Recorder::new(file, identity, clock));
    Ok(())
}

/// Accepts no payload, source path, arbitrary formatter, or other free text.
pub(crate) fn record_panic(kind: PanicKind) {
    record_if_ready(RECORDER.get(), kind);
}

fn record_if_ready<S: Sink>(recorder: Option<&Recorder<S>>, kind: PanicKind) {
    if let Some(recorder) = recorder {
        recorder.record_with(kind, || recorder.clock.sample());
    }
}

struct Clock {
    frequency: u64,
    ticks_at_prepare: u64,
    elapsed_at_prepare_ms: u64,
}
impl Clock {
    fn prepare(origin: Instant) -> io::Result<Self> {
        let mut frequency = 0;
        let mut ticks = 0;
        if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0
            || frequency <= 0
            || unsafe { QueryPerformanceCounter(&mut ticks) } == 0
            || ticks < 0
        {
            return Err(io::Error::other("panic evidence clock unavailable"));
        }
        // Windows Instant and raw QPC use the same monotonic source. Warm all
        // clock paths here; no std clock/TLS initialization belongs in a hook.
        let elapsed_at_prepare_ms = origin.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let mut utc = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        unsafe { GetSystemTimePreciseAsFileTime(&mut utc) };
        let _ = unsafe { GetCurrentThreadId() };
        Ok(Self {
            frequency: frequency as u64,
            ticks_at_prepare: ticks as u64,
            elapsed_at_prepare_ms,
        })
    }

    fn sample(&self) -> Option<(Stamp, u32)> {
        let mut ticks = 0;
        if unsafe { QueryPerformanceCounter(&mut ticks) } == 0 || ticks < 0 {
            return None;
        }
        let elapsed_ticks = (ticks as u64).checked_sub(self.ticks_at_prepare)?;
        let elapsed_ms = (u128::from(elapsed_ticks) * 1000 / u128::from(self.frequency))
            .min(u128::from(u64::MAX)) as u64;
        let run_elapsed_ms = self.elapsed_at_prepare_ms.checked_add(elapsed_ms)?;
        let mut utc = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        unsafe { GetSystemTimePreciseAsFileTime(&mut utc) };
        let intervals = u64::from(utc.dwHighDateTime) << 32 | u64::from(utc.dwLowDateTime);
        Some((
            Stamp {
                utc_ms: intervals.checked_sub(FILETIME_UNIX_EPOCH)? / 10_000,
                run_elapsed_ms,
            },
            unsafe { GetCurrentThreadId() },
        ))
    }
}

trait Sink {
    fn write_once(&self, bytes: &[u8]);
}
impl Sink for File {
    fn write_once(&self, bytes: &[u8]) {
        let mut written = 0;
        // A synchronous WriteFile has no latency guarantee. One attempt only:
        // short writes/errors consume this attempt, without retry/log/flush.
        unsafe {
            WriteFile(
                self.as_raw_handle(),
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                core::ptr::null_mut(),
            )
        };
    }
}

struct Recorder<S> {
    sink: S,
    identity: RunIdentity,
    clock: Clock,
    writing: AtomicBool,
    attempts: AtomicU32,
    buffer: UnsafeCell<Buffer>,
}
// Only the successful one-shot CAS owner touches the fixed buffer. Identity,
// clock, and sink are immutable after publication; no hook path drops the file.
unsafe impl<S: Sync> Sync for Recorder<S> {}

impl<S: Sink> Recorder<S> {
    fn new(sink: S, identity: RunIdentity, clock: Clock) -> Self {
        Self {
            sink,
            identity,
            clock,
            writing: AtomicBool::new(false),
            attempts: AtomicU32::new(0),
            buffer: UnsafeCell::new(Buffer::new()),
        }
    }

    fn record_with(&self, kind: PanicKind, sample: impl FnOnce() -> Option<(Stamp, u32)>) {
        if self
            .writing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let attempts = self.attempts.load(Ordering::Relaxed);
        if attempts < LIMIT {
            if let Some((stamp, thread)) = sample() {
                self.attempts.store(attempts + 1, Ordering::Relaxed);
                let buffer = unsafe { &mut *self.buffer.get() };
                if let Some(bytes) = buffer.format(
                    self.identity,
                    PanicObservation {
                        stamp,
                        thread,
                        kind,
                    },
                ) {
                    self.sink.write_once(bytes);
                }
            }
        }
        self.writing.store(false, Ordering::Release);
    }
}

struct Buffer {
    bytes: [u8; CAPACITY],
    len: usize,
}
impl Buffer {
    const fn new() -> Self {
        Self {
            bytes: [0; CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) -> Option<()> {
        *self.bytes.get_mut(self.len)? = byte;
        self.len += 1;
        Some(())
    }

    fn number(&mut self, mut value: u64) -> Option<()> {
        let mut digits = [0; 20];
        let mut count = 0;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        for index in (0..count).rev() {
            self.push(digits[index])?;
        }
        Some(())
    }

    fn format(&mut self, identity: RunIdentity, observation: PanicObservation) -> Option<&[u8]> {
        self.len = 0;
        // Fixed schema 1, role 1 (GUI core); everything else is numeric only.
        for byte in b"miv-panic 1 1" {
            self.push(*byte)?;
        }
        for value in [
            identity.run,
            u64::from(identity.core_pid),
            identity.core_started_utc_ms,
            identity.core_started_ticks,
            observation.stamp.utc_ms,
            observation.stamp.run_elapsed_ms,
            u64::from(observation.thread),
            observation.kind.code(),
        ] {
            self.push(b' ')?;
            self.number(value)?;
        }
        self.push(b'\n')?;
        Some(&self.bytes[..self.len])
    }
}

/// Normal-worker-only, bounded reader. Incomplete/foreign/unknown lines never
/// become evidence, and no source text escapes the numeric allowlist parser.
pub(super) fn read_observations(
    directory: &Path,
    identity: RunIdentity,
) -> io::Result<Vec<PanicObservation>> {
    let file = match File::open(directory.join(FILE_NAME)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(Vec::new());
    }
    Ok(bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter_map(|line| parse_line(line, identity))
        .take(LIMIT as usize)
        .collect())
}

fn parse_number(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > 20 {
        return None;
    }
    bytes.iter().try_fold(0_u64, |value, digit| {
        if !digit.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
    })
}

fn parse_line(line: &[u8], expected: RunIdentity) -> Option<PanicObservation> {
    if line.len() > CAPACITY || !expected.valid() {
        return None;
    }
    let mut fields = line.strip_suffix(b"\n")?.split(|byte| *byte == b' ');
    if fields.next()? != b"miv-panic" || fields.next()? != b"1" || fields.next()? != b"1" {
        return None;
    }
    let identity = RunIdentity {
        role: RunRole::Core,
        run: parse_number(fields.next()?)?,
        core_pid: u32::try_from(parse_number(fields.next()?)?).ok()?,
        core_started_utc_ms: parse_number(fields.next()?)?,
        core_started_ticks: parse_number(fields.next()?)?,
    };
    if identity != expected {
        return None;
    }
    let stamp = Stamp {
        utc_ms: parse_number(fields.next()?)?,
        run_elapsed_ms: parse_number(fields.next()?)?,
    };
    let thread = u32::try_from(parse_number(fields.next()?)?).ok()?;
    let kind = PanicKind::from_code(parse_number(fields.next()?)?)?;
    if fields.next().is_some() || stamp.utc_ms == 0 || thread == 0 {
        return None;
    }
    Some(PanicObservation {
        stamp,
        thread,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeSink {
        writes: Mutex<Vec<Vec<u8>>>,
        // None simulates a failed raw write; Some(n) a possibly short write.
        accepted: Option<usize>,
    }
    impl Sink for FakeSink {
        fn write_once(&self, bytes: &[u8]) {
            let length = self.accepted.unwrap_or(0).min(bytes.len());
            self.writes.lock().unwrap().push(bytes[..length].to_vec());
        }
    }
    fn identity() -> RunIdentity {
        RunIdentity {
            role: RunRole::Core,
            run: 42,
            core_pid: 123,
            core_started_utc_ms: 1000,
            core_started_ticks: 5000,
        }
    }
    fn observation(kind: PanicKind) -> PanicObservation {
        PanicObservation {
            stamp: Stamp {
                utc_ms: 2500,
                run_elapsed_ms: 1500,
            },
            thread: 77,
            kind,
        }
    }
    fn recorder(accepted: Option<usize>) -> Recorder<FakeSink> {
        Recorder::new(
            FakeSink {
                accepted,
                ..Default::default()
            },
            identity(),
            Clock::prepare(Instant::now()).unwrap(),
        )
    }
    fn record(recorder: &Recorder<FakeSink>, kind: PanicKind) {
        let event = observation(kind);
        recorder.record_with(kind, || Some((event.stamp, event.thread)));
    }
    fn line(identity: RunIdentity, observation: PanicObservation) -> Vec<u8> {
        Buffer::new()
            .format(identity, observation)
            .unwrap()
            .to_vec()
    }

    #[test]
    fn panic_single_write_preserves_first_evidence_and_fixed_classification() {
        let recorder = recorder(Some(usize::MAX));
        for kind in [
            PanicKind::Rust,
            PanicKind::GraphicsOutOfMemory,
            PanicKind::GraphicsValidation,
            PanicKind::GraphicsInternal,
        ] {
            record(&recorder, kind);
        }
        let writes = recorder.sink.writes.lock().unwrap();
        assert_eq!(writes.len(), 4);
        for (index, bytes) in writes.iter().enumerate() {
            let parsed = parse_line(bytes, identity()).unwrap();
            assert_eq!(parsed.kind.code(), index as u64 + 1);
            assert_eq!(parsed.thread, 77);
            assert_eq!(parsed.stamp.run_elapsed_ms, 1500);
        }
    }

    #[test]
    fn panic_short_write_and_error_never_retry_and_consume_the_cap() {
        for accepted in [None, Some(7)] {
            let recorder = recorder(accepted);
            for _ in 0..LIMIT + 10 {
                record(&recorder, PanicKind::Rust);
            }
            recorder.record_with(PanicKind::Rust, || panic!("capped path sampled clock"));
            let writes = recorder.sink.writes.lock().unwrap();
            assert_eq!(writes.len(), LIMIT as usize);
            assert!(
                writes
                    .iter()
                    .all(|bytes| bytes.len() == accepted.unwrap_or(0))
            );
        }
    }

    #[test]
    fn panic_reentry_contention_unavailable_and_missing_clock_skip_without_waiting() {
        record_if_ready::<FakeSink>(None, PanicKind::Rust);
        let recorder = recorder(Some(usize::MAX));
        recorder.writing.store(true, Ordering::Relaxed);
        recorder.record_with(PanicKind::Rust, || panic!("contended path sampled clock"));
        recorder.writing.store(false, Ordering::Relaxed);
        recorder.record_with(PanicKind::Rust, || {
            recorder.record_with(PanicKind::GraphicsOutOfMemory, || {
                panic!("reentrant path sampled clock")
            });
            None
        });
        assert_eq!(recorder.attempts.load(Ordering::Relaxed), 0);
        record(&recorder, PanicKind::Rust);
        assert_eq!(recorder.sink.writes.lock().unwrap().len(), 1);
    }

    #[test]
    fn panic_reader_rejects_foreign_malformed_unknown_and_partial_lines() {
        let directory = tempfile::tempdir().unwrap();
        let valid = line(identity(), observation(PanicKind::GraphicsValidation));
        let text = String::from_utf8(valid.clone()).unwrap();
        let mut bytes = Vec::new();
        for malformed in [
            text.replacen("miv-panic 1 1", "miv-panic 2 1", 1),
            text.replacen("miv-panic 1 1", "miv-panic 1 2", 1),
            text.replacen(" 42 ", " 43 ", 1),
            text.replacen(" 123 ", " 124 ", 1),
            text.replacen(" 1000 ", " 1001 ", 1),
            text.replacen(" 5000 ", " 5001 ", 1),
            text.replacen(" 77 ", " 4294967296 ", 1),
            text.replacen(" 2500 ", " 18446744073709551616 ", 1),
            text.replacen(" 2500 ", " +2500 ", 1),
            text.replacen(" 2500 ", "  2500 ", 1),
            text.replace(" 3\n", " 9\n"),
            text.replace(" 3\n", " 3 C:\\private\\secret.zip image.png query\n"),
            text.replace('\n', "\r\n"),
        ] {
            assert_eq!(parse_line(malformed.as_bytes(), identity()), None);
            bytes.extend_from_slice(malformed.as_bytes());
        }
        bytes.extend_from_slice(&valid);
        bytes.extend_from_slice(valid.strip_suffix(b"\n").unwrap());
        std::fs::write(directory.path().join(FILE_NAME), bytes).unwrap();
        assert_eq!(
            read_observations(directory.path(), identity()).unwrap(),
            vec![observation(PanicKind::GraphicsValidation)]
        );
    }

    #[test]
    fn panic_reader_limits_bytes_and_records_without_returning_raw_strings() {
        let directory = tempfile::tempdir().unwrap();
        let valid = line(identity(), observation(PanicKind::Rust));
        std::fs::write(directory.path().join(FILE_NAME), valid.repeat(40)).unwrap();
        assert_eq!(
            read_observations(directory.path(), identity())
                .unwrap()
                .len(),
            LIMIT as usize
        );
        std::fs::write(
            directory.path().join(FILE_NAME),
            vec![b'x'; MAX_BYTES as usize + 1],
        )
        .unwrap();
        assert!(
            read_observations(directory.path(), identity())
                .unwrap()
                .is_empty()
        );
        let absent = directory.path().join("absent");
        assert!(read_observations(&absent, identity()).unwrap().is_empty());
    }

    #[test]
    fn panic_fixed_buffer_holds_all_numeric_maxima_and_local_file_roundtrips() {
        let largest = RunIdentity {
            role: RunRole::Core,
            run: u64::MAX,
            core_pid: u32::MAX,
            core_started_utc_ms: u64::MAX,
            core_started_ticks: u64::MAX,
        };
        let event = PanicObservation {
            stamp: Stamp {
                utc_ms: u64::MAX,
                run_elapsed_ms: u64::MAX,
            },
            thread: u32::MAX,
            kind: PanicKind::GraphicsInternal,
        };
        let bytes = line(largest, event);
        assert!(bytes.len() < CAPACITY);
        assert_eq!(parse_line(&bytes, largest), Some(event));

        let directory = tempfile::tempdir().unwrap();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.path().join(FILE_NAME))
            .unwrap();
        let recorder = Recorder::new(file, identity(), Clock::prepare(Instant::now()).unwrap());
        recorder.record_with(PanicKind::Rust, || {
            let event = observation(PanicKind::Rust);
            Some((event.stamp, event.thread))
        });
        assert_eq!(
            read_observations(directory.path(), identity()).unwrap(),
            vec![observation(PanicKind::Rust)]
        );
    }
}
