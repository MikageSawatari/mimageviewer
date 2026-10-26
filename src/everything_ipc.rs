//! Bounded, worker-only Everything QUERY2 IPC. No filesystem or UI dependencies.
//!
//! Wire definitions verified against the author's public header (not an SDK download):
//! https://github.com/voidtools/ES/blob/main/src/everything_ipc.h
//! The official CLI page links that repository. Definitions are packed DWORDs, including
//! the reply HWND; DRIVE and ROOT are aliases (both 2). The protocol definitions are
//! covered by https://github.com/voidtools/ES/blob/main/LICENSE (MIT, 2025 voidtools).
//! See the distribution notices for the preserved permission notice.
//! Cancellation by a replacement query on the same HWND is documented by the author:
//! https://www.voidtools.com/forum/viewtopic.php?t=14938
//! Native handle conversion follows Microsoft's interprocess-communication guidance:
//! https://learn.microsoft.com/en-us/windows/win32/winprog64/interprocess-communication

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod native;
#[cfg(test)]
mod tests;

pub const MAX_RESULTS: u32 = 10_000;
pub const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_QUERY_UNITS: usize = 32_768;
pub const SEND_TIMEOUT: Duration = Duration::from_secs(60);
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
const CANCEL_SEND_TIMEOUT: Duration = Duration::from_millis(100);
const QUERY2W: usize = 18;
const FULL_PATH: u32 = 4;
const NAME_ASCENDING: u32 = 1;
const ITEM_FOLDER: u32 = 1;
const ITEM_ROOT: u32 = 2;
const QUERY_HEADER_BYTES: usize = 28;
const REPLY_HEADER_BYTES: usize = 20;
const ITEM_BYTES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EverythingRow {
    pub full_path: String,
    pub flags: u32,
}

impl EverythingRow {
    pub fn is_folder(&self) -> bool {
        self.flags & ITEM_ROOT == 0 && self.flags & ITEM_FOLDER != 0
    }

    /// Indexed file claims only. The prepare worker still validates physical identity
    /// and stats the supported candidate; this must never authorize metadata purges.
    pub fn is_file(&self) -> bool {
        self.flags & (ITEM_FOLDER | ITEM_ROOT) == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EverythingReply {
    pub rows: Vec<EverythingRow>,
    pub total_items: u32,
    pub actual_sort: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EverythingError {
    EmptyQuery,
    EmbeddedNul,
    QueryTooLong,
    Unavailable,
    UnsupportedVersion,
    DatabaseNotLoaded,
    Rejected,
    NotResponding,
    Busy,
    SendFailed,
    Timeout,
    Disconnected,
    Cancelled,
    ReplyTooLarge,
    Malformed(&'static str),
    Native(u32),
    RequestIdsExhausted,
}

impl EverythingError {
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::EmptyQuery => "Everything の検索条件を入力してください。",
            Self::EmbeddedNul | Self::QueryTooLong => {
                "検索条件が長すぎるか、使用できない文字を含んでいます。"
            }
            Self::Unavailable => {
                "Everything に接続できません。検索クライアントを起動してください。"
            }
            Self::UnsupportedVersion | Self::Rejected => {
                "この Everything では IPC 検索を利用できません。通常版の 1.4.1 以降と IPC 設定を確認してください。"
            }
            Self::DatabaseNotLoaded => {
                "Everything の索引を準備中です。準備後にもう一度検索してください。"
            }
            Self::ReplyTooLarge => {
                "Everything の検索結果が大きすぎます。検索条件を絞ってください。"
            }
            Self::Cancelled => "検索を取り消しました。",
            Self::NotResponding => {
                "Everything が応答していません。起動直後の索引の読み込みや更新などの処理が終わるまで待ってから、もう一度検索してください。"
            }
            Self::Busy | Self::Timeout => {
                "Everything の処理が遅いか、混雑しています。しばらく待つか、検索条件を絞って再検索してください。"
            }
            _ => {
                "Everything から結果を取得できませんでした。起動状態・権限を確認して再検索してください。"
            }
        }
    }
}

impl fmt::Display for EverythingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Diagnostics contain failure categories, never query strings or result paths.
        write!(f, "Everything IPC: {self:?}")
    }
}
impl std::error::Error for EverythingError {}

/// Fresh per-run cancellation. Clones share one flag and manual-reset wake event.
/// It is deliberately impossible to reset the token and revive an old request.
#[derive(Clone)]
pub struct CancelHandle {
    inner: Arc<CancelInner>,
}

struct CancelInner {
    flag: AtomicBool,
    #[cfg(windows)]
    event: std::os::windows::io::OwnedHandle,
}

impl CancelHandle {
    pub fn new() -> Result<Self, EverythingError> {
        #[cfg(windows)]
        let event = {
            use std::os::windows::io::FromRawHandle;
            use windows::Win32::System::Threading::CreateEventW;
            let event = unsafe { CreateEventW(None, true, false, None) }
                .map_err(|e| EverythingError::Native(e.code().0 as u32))?;
            // SAFETY: this newly created handle is exclusively transferred to OwnedHandle.
            unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(event.0) }
        };
        Ok(Self {
            inner: Arc::new(CancelInner {
                flag: AtomicBool::new(false),
                #[cfg(windows)]
                event,
            }),
        })
    }

    /// Nonblocking local publication; the UI never joins the query worker.
    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::Release);
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::Threading::SetEvent;
            let _ = unsafe { SetEvent(HANDLE(self.inner.event.as_raw_handle())) };
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::Acquire)
    }

    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.inner.flag
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EverythingVersion {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

impl EverythingVersion {
    fn supports_query2(self) -> bool {
        // A future major isn't evidence of this wire's compatibility. 1.5 is accepted
        // only through the QUERY2 acceptance and strict response parsing below.
        self.major == 1 && (self.minor == 5 || (self.minor == 4 && self.revision >= 1))
    }
}

/// Owned callback data. A native implementation copies during WM_COPYDATA and parses
/// on its worker after returning; borrowed callback pointers cannot cross this API.
#[derive(Debug)]
pub struct EndpointReply {
    pub reply_id: u32,
    pub bytes: Vec<u8>,
}

/// Injectable single-run endpoint. The Windows implementation owns its reply window
/// and message pump on the calling worker. Fakes never discover a real client.
pub trait EverythingEndpoint {
    fn version(&mut self, cancel: &CancelHandle) -> Result<EverythingVersion, EverythingError>;
    fn database_loaded(&mut self) -> Result<bool, EverythingError>;
    fn reply_hwnd(&self) -> u32;
    /// Install correlation before send, including immediate/reentrant responses.
    fn activate(&mut self, reply_id: u32);
    fn deactivate(&mut self);
    /// API success and receiver acceptance must both be checked by this method.
    fn send(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), EverythingError>;
    /// Monotonic elapsed time since endpoint construction, injectable in fakes.
    fn elapsed(&self) -> Duration;
    /// Wait on cancellation/event/messages up to the supplied absolute deadline.
    fn wait(
        &mut self,
        cancel: &CancelHandle,
        deadline: Duration,
    ) -> Result<EndpointReply, EverythingError>;
}

static NEXT_REPLY_ID: AtomicU32 = AtomicU32::new(1);

fn next_reply_id() -> Result<u32, EverythingError> {
    // Never wrap: destroyed HWND reuse cannot alias any earlier run's correlation ID.
    NEXT_REPLY_ID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| EverythingError::RequestIdsExhausted)
}

fn cancelled(cancel: &CancelHandle) -> Result<(), EverythingError> {
    if cancel.is_cancelled() {
        Err(EverythingError::Cancelled)
    } else {
        Ok(())
    }
}

/// Run on a dedicated worker, never on the UI thread. Does not start/stop Everything.
pub fn query(query: &str, cancel: &CancelHandle) -> Result<EverythingReply, EverythingError> {
    validate_query(query)?;
    cancelled(cancel)?;
    #[cfg(windows)]
    {
        let query = query.to_owned();
        let run_cancel = cancel.clone();
        native::run_cancellable(cancel, move || {
            let mut endpoint = native::WindowsEndpoint::discover()?;
            query_with_endpoint(&query, &run_cancel, &mut endpoint)
        })
    }
    #[cfg(not(windows))]
    Err(EverythingError::Unavailable)
}

pub fn query_with_endpoint(
    query: &str,
    cancel: &CancelHandle,
    endpoint: &mut impl EverythingEndpoint,
) -> Result<EverythingReply, EverythingError> {
    validate_query(query)?;
    cancelled(cancel)?;
    let version_started = Instant::now();
    let version = endpoint.version(cancel);
    cancelled(cancel)?;
    let version = version?;
    if !version.supports_query2() {
        return Err(EverythingError::UnsupportedVersion);
    }
    let version_elapsed = version_started.elapsed();
    let readiness_started = Instant::now();
    let loaded = endpoint.database_loaded();
    cancelled(cancel)?;
    let loaded = loaded?;
    if !loaded {
        return Err(EverythingError::DatabaseNotLoaded);
    }
    let readiness_elapsed = readiness_started.elapsed();
    let reply_id = next_reply_id()?;
    let cancel_id = next_reply_id()?;
    let bytes = encode_query(query, endpoint.reply_hwnd(), reply_id)?;
    endpoint.activate(reply_id);
    let deadline = endpoint.elapsed().saturating_add(REPLY_TIMEOUT);
    let result = (|| {
        cancelled(cancel)?;
        let send_started = Instant::now();
        let sent = endpoint.send(
            &bytes,
            deadline
                .saturating_sub(endpoint.elapsed())
                .min(SEND_TIMEOUT),
        );
        cancelled(cancel)?;
        sent?;
        let send_elapsed = send_started.elapsed();
        let wait_started = Instant::now();
        loop {
            cancelled(cancel)?;
            if endpoint.elapsed() >= deadline {
                return Err(EverythingError::Timeout);
            }
            let reply = endpoint.wait(cancel, deadline);
            cancelled(cancel)?;
            let reply = reply?;
            if reply.reply_id != reply_id {
                continue;
            }
            let wait_elapsed = wait_started.elapsed();
            let parse_started = Instant::now();
            let parsed = parse_reply(&reply.bytes)?;
            cancelled(cancel)?;
            let parse_elapsed = parse_started.elapsed();
            // One success record per request. The existing bounded asynchronous
            // perf sink carries only numeric diagnostics, never query/path data.
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "everything",
                    "ipc_complete",
                    None,
                    u64::from(reply_id),
                    &[
                        ("source", "everything".into()),
                        (
                            "version_ms",
                            (version_elapsed.as_secs_f64() * 1000.0).into(),
                        ),
                        (
                            "readiness_ms",
                            (readiness_elapsed.as_secs_f64() * 1000.0).into(),
                        ),
                        ("send_ms", (send_elapsed.as_secs_f64() * 1000.0).into()),
                        (
                            "reply_wait_ms",
                            (wait_elapsed.as_secs_f64() * 1000.0).into(),
                        ),
                        ("parse_ms", (parse_elapsed.as_secs_f64() * 1000.0).into()),
                        ("reply_bytes", reply.bytes.len().into()),
                        ("returned_count", parsed.rows.len().into()),
                        ("total_count", parsed.total_items.into()),
                        ("raw_limit", MAX_RESULTS.into()),
                        ("byte_limit", MAX_REPLY_BYTES.into()),
                        ("actual_sort", parsed.actual_sort.into()),
                    ],
                );
            }
            return Ok(parsed);
        }
    })();
    // Reject late original replies before a cancellation send can reenter WndProc.
    endpoint.deactivate();
    if matches!(
        result,
        Err(EverythingError::Cancelled
            | EverythingError::Timeout
            | EverythingError::Disconnected
            | EverythingError::Busy
            | EverythingError::NotResponding
            | EverythingError::SendFailed)
    ) {
        let cancellation = encode_query("nothing:", endpoint.reply_hwnd(), cancel_id)?;
        // Exactly one bounded replacement query; never wait for its reply or retry it.
        let _ = endpoint.send(&cancellation, CANCEL_SEND_TIMEOUT);
    }
    result
}

pub fn validate_query(query: &str) -> Result<(), EverythingError> {
    if query.contains('\0') {
        return Err(EverythingError::EmbeddedNul);
    }
    if query.trim().is_empty() {
        return Err(EverythingError::EmptyQuery);
    }
    if query.encode_utf16().take(MAX_QUERY_UNITS).count() >= MAX_QUERY_UNITS {
        return Err(EverythingError::QueryTooLong);
    }
    Ok(())
}

/// Encode fields explicitly; never serialize native HWND/usize/Rust struct layout.
fn encode_query(query: &str, reply_hwnd: u32, reply_id: u32) -> Result<Vec<u8>, EverythingError> {
    validate_query(query)?;
    let units = query.encode_utf16().count();
    let size = units
        .checked_add(1)
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| n.checked_add(QUERY_HEADER_BYTES))
        .ok_or(EverythingError::QueryTooLong)?;
    let mut bytes = Vec::with_capacity(size);
    // Zero flags = case-insensitive, partial filename match, independent of UI toggles.
    for field in [
        reply_hwnd,
        reply_id,
        0,
        0,
        MAX_RESULTS,
        FULL_PATH,
        NAME_ASCENDING,
    ] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    for unit in query.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(bytes)
}

fn dword(bytes: &[u8], offset: usize) -> Result<u32, EverythingError> {
    let end = offset
        .checked_add(4)
        .ok_or(EverythingError::Malformed("DWORD overflow"))?;
    let data: [u8; 4] = bytes
        .get(offset..end)
        .ok_or(EverythingError::Malformed("missing DWORD"))?
        .try_into()
        .map_err(|_| EverythingError::Malformed("missing DWORD"))?;
    Ok(u32::from_le_bytes(data))
}

/// Validate the entire reply before exposing any row. No I/O, lossy UTF-16, partial
/// success, metadata purge, or protocol fallback occurs at this boundary.
fn parse_reply(bytes: &[u8]) -> Result<EverythingReply, EverythingError> {
    if bytes.len() > MAX_REPLY_BYTES {
        return Err(EverythingError::ReplyTooLarge);
    }
    if bytes.len() < REPLY_HEADER_BYTES || bytes.len() % 2 != 0 {
        return Err(EverythingError::Malformed("header or odd byte count"));
    }
    let total_items = dword(bytes, 0)?;
    let count = dword(bytes, 4)?;
    if count > MAX_RESULTS || count > total_items {
        return Err(EverythingError::Malformed("result count"));
    }
    if dword(bytes, 8)? != 0 || dword(bytes, 12)? != FULL_PATH {
        return Err(EverythingError::Malformed("offset or request flags"));
    }
    let actual_sort = dword(bytes, 16)?;
    let table_end = (count as usize)
        .checked_mul(ITEM_BYTES)
        .and_then(|n| n.checked_add(REPLY_HEADER_BYTES))
        .ok_or(EverythingError::Malformed("item table overflow"))?;
    if table_end > bytes.len() {
        return Err(EverythingError::Malformed("missing item table"));
    }
    let mut fields = Vec::with_capacity(count as usize);
    let mut ranges = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let item = REPLY_HEADER_BYTES + index * ITEM_BYTES;
        let flags = dword(bytes, item)?;
        if flags & !(ITEM_FOLDER | ITEM_ROOT) != 0 {
            return Err(EverythingError::Malformed("unknown item flags"));
        }
        let offset = dword(bytes, item + 4)? as usize;
        if offset < table_end || offset % 2 != 0 {
            return Err(EverythingError::Malformed("item data offset"));
        }
        let length = dword(bytes, offset)? as usize;
        let start = offset
            .checked_add(4)
            .ok_or(EverythingError::Malformed("string offset overflow"))?;
        let end = length
            .checked_add(1)
            .and_then(|n| n.checked_mul(2))
            .and_then(|n| start.checked_add(n))
            .ok_or(EverythingError::Malformed("string length overflow"))?;
        let text = bytes
            .get(start..end)
            .ok_or(EverythingError::Malformed("truncated string"))?;
        if text[text.len() - 2..] != [0, 0] {
            return Err(EverythingError::Malformed("missing string terminator"));
        }
        ranges.push((offset, end));
        fields.push((flags, start, end));
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|r| r[0].1 > r[1].0) {
        return Err(EverythingError::Malformed("overlapping item data"));
    }
    // Validate nonoverlap BEFORE decoding/allocating paths. Otherwise 10,000 item
    // offsets could all reference one 16MiB string and amplify untrusted allocation.
    let mut rows = Vec::with_capacity(count as usize);
    for (flags, start, end) in fields {
        let units: Vec<u16> = bytes[start..end - 2]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if units.contains(&0) {
            return Err(EverythingError::Malformed("embedded string terminator"));
        }
        let full_path =
            String::from_utf16(&units).map_err(|_| EverythingError::Malformed("invalid UTF-16"))?;
        rows.push(EverythingRow { full_path, flags });
    }
    Ok(EverythingReply {
        rows,
        total_items,
        actual_sort,
    })
}
