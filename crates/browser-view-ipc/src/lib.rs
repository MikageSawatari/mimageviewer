//! Browser open transport only. No settings, image decoding, or GUI dependencies.
//! An acknowledgement means channel delivery, never application admission/display.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
};

pub const PIPE_NAME: &str = r"\\.\pipe\mImageViewerBrowserView_v1";
pub const PORTABLE_PIPE_NAME: &str = r"\\.\pipe\mImageViewerBrowserView_portable_v1";
pub const CLI_FLAG: &str = "--browser-view";
pub const PROTOCOL_VERSION: u32 = 1;
pub const APPLICATION_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_FRAME_BYTES: usize = 128 * 1024;
pub const RESTART_MESSAGE: &str = "mIV を再起動してください";

#[cfg(windows)]
pub mod transport;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub maximized: bool,
}
impl Placement {
    pub fn validate(self) -> io::Result<()> {
        let width = i64::from(self.right) - i64::from(self.left);
        let height = i64::from(self.bottom) - i64::from(self.top);
        if !(1..=65_536).contains(&width) || !(1..=65_536).contains(&height) {
            return Err(invalid("invalid browser rectangle"));
        }
        Ok(())
    }
    pub fn cli_value(self) -> String {
        format!(
            "{},{},{},{},{}",
            self.left,
            self.top,
            self.right,
            self.bottom,
            u8::from(self.maximized)
        )
    }
    pub fn parse(value: &str) -> io::Result<Self> {
        let parts: Vec<_> = value.split(',').collect();
        if parts.len() != 5 {
            return Err(invalid(
                "browser rectangle needs left,top,right,bottom,maximized",
            ));
        }
        let number = |i: usize| {
            parts[i]
                .parse::<i32>()
                .map_err(|_| invalid("invalid rectangle coordinate"))
        };
        let placement = Self {
            left: number(0)?,
            top: number(1)?,
            right: number(2)?,
            bottom: number(3)?,
            maximized: match parts[4] {
                "0" => false,
                "1" => true,
                _ => return Err(invalid("invalid maximized value")),
            },
        };
        placement.validate()?;
        Ok(placement)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub protocol_version: u32,
    pub application_version: String,
    pub placement: Placement,
    pub path: PathBuf,
}
impl Request {
    pub fn new(placement: Placement, path: PathBuf) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            application_version: APPLICATION_VERSION.into(),
            placement,
            path,
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        self.placement.validate()?;
        let path = self
            .path
            .to_str()
            .ok_or_else(|| invalid("path is not Unicode"))?;
        if path.is_empty()
            || path.contains('\0')
            || path.encode_utf16().count() > 32_767
            || !self.path.is_absolute()
        {
            return Err(invalid("invalid browser path"));
        }
        Ok(())
    }
}

// Share option-value knowledge when finding the browser switch. A switch-looking value
// and a literal path after -- must never accidentally activate this lane.
pub fn option_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--browser-view"
            | "--data-dir"
            | "--gpu-backend"
            | "--gpu-test-fail-device"
            | "--window-size"
            | "--perf-log-path"
            | "--play-test"
            | "--play-duration"
            | "--play-test-start"
            | "--dcomp-presenter-test"
            | "--dcomp-duration"
            | "--dcomp-window-size"
            | "--dcomp-sync-interval"
            | "--dcomp-start"
            | "--test-script"
            | "--test-evidence-dir"
            | "--settings-override"
            | "--miv-sound-migration-source"
            | "--miv-sound-migration-checkpoints"
            | "--miv-sound-migration-original-browser"
            | "--miv-sound-migration-close-file"
    )
}
/// Arguments exclude argv[0]. All preceding ordinary options remain the caller's concern.
pub fn parse_cli(args: &[OsString]) -> io::Result<Option<Request>> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--" {
            return Ok(None);
        }
        if args[i] == CLI_FLAG {
            if args.len() != i + 4 || args.get(i + 2).is_none_or(|v| v != "--") {
                return Err(invalid("expected --browser-view <rectangle> -- <path>"));
            }
            let placement = Placement::parse(
                args[i + 1]
                    .to_str()
                    .ok_or_else(|| invalid("invalid rectangle"))?,
            )?;
            if args[i + 3].is_empty() {
                return Err(invalid("empty browser path"));
            }
            let path = std::path::absolute(PathBuf::from(&args[i + 3]))?;
            let request = Request::new(placement, path);
            request.validate()?;
            return Ok(Some(request));
        }
        let Some(flag) = args[i].to_str().filter(|flag| flag.starts_with("--")) else {
            return Ok(None);
        };
        i += if option_takes_value(flag)
            || (flag == "--perf-log"
                && args
                    .get(i + 1)
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| !v.starts_with("--")))
        {
            2
        } else {
            1
        };
    }
    Ok(None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    VersionMismatch,
    InvalidRequest,
    ChannelClosed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub protocol_version: u32,
    pub application_version: String,
    pub status: Status,
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(invalid("browser frame too large"));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)
}
pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid("invalid browser frame length"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub fn serve(
    stream: &mut (impl Read + Write),
    deliver: impl FnOnce(Request) -> bool,
) -> io::Result<()> {
    let request: Request = read_frame(stream)?;
    let status = if request.protocol_version != PROTOCOL_VERSION
        || request.application_version != APPLICATION_VERSION
    {
        Status::VersionMismatch
    } else if request.validate().is_err() {
        Status::InvalidRequest
    } else if deliver(request) {
        Status::Queued
    } else {
        Status::ChannelClosed
    };
    write_frame(
        stream,
        &Response {
            protocol_version: PROTOCOL_VERSION,
            application_version: APPLICATION_VERSION.into(),
            status,
        },
    )
}

/// Native operations can be replaced without granting foreground rights in tests.
pub trait Connection: Read + Write {
    fn server_pid(&mut self) -> io::Result<u32>;
    fn allow_foreground(&mut self, pid: u32) -> io::Result<()>;
}
pub fn grant_child_foreground(
    pid: u32,
    grant: impl FnOnce(u32) -> io::Result<()>,
    mut log: impl FnMut(&str),
) {
    if let Err(error) = grant(pid) {
        log(&format!(
            "browser_view: child foreground grant failed: {error}"
        ));
    }
}
pub fn deliver(
    stream: &mut impl Connection,
    request: &Request,
    mut log: impl FnMut(&str),
) -> io::Result<()> {
    request.validate()?;
    match stream.server_pid() {
        Ok(pid) if pid != 0 => {
            if let Err(error) = stream.allow_foreground(pid) {
                log(&format!(
                    "browser_view: server foreground grant failed: {error}"
                ));
            }
        }
        _ => log("browser_view: server PID unavailable; foreground grant skipped"),
    }
    write_frame(stream, request)?;
    let response: Response = read_frame(stream)?;
    if response.protocol_version != PROTOCOL_VERSION
        || response.application_version != APPLICATION_VERSION
        || response.status != Status::Queued
    {
        return Err(invalid(&format!(
            "browser delivery refused: protocol={}, application={}, status={:?}; restart mIV",
            response.protocol_version, response.application_version, response.status
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupRoute {
    StartCore,
    Forwarded,
}
/// Both launcher probing and core mutex-race handling use this terminal decision.
/// No legacy transport, activation event, retry, or second spawn exists on failure.
pub fn route(
    core_running: bool,
    forward: impl FnOnce() -> io::Result<()>,
) -> io::Result<StartupRoute> {
    if !core_running {
        return Ok(StartupRoute::StartCore);
    }
    forward()?;
    Ok(StartupRoute::Forwarded)
}

#[cfg(test)]
mod tests;
