//! The stdio MCP lane owns its child until exit, independently of GUI startup.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::{effetune_bundle, runtime_locks};

pub(crate) fn arguments(args: &[OsString]) -> Option<&[OsString]> {
    (args.first().map(OsString::as_os_str) == Some(OsStr::new("--mcp"))).then(|| &args[1..])
}

pub(crate) trait WaitForExit {
    fn wait_for_exit(&mut self) -> io::Result<i32>;
}

impl WaitForExit for Child {
    fn wait_for_exit(&mut self) -> io::Result<i32> {
        Ok(self.wait()?.code().unwrap_or(1))
    }
}

/// Injectable process and runtime boundaries keep tests away from real profiles
/// and product executables. The lexical owners below express the entire lifetime.
pub(crate) fn run<C: WaitForExit, M>(
    args: &[OsString],
    runtime: impl FnOnce() -> Result<PathBuf, String>,
    marker: impl FnOnce() -> Result<M, String>,
    spawn: impl FnOnce(&mut Command) -> io::Result<C>,
    mut stderr: impl FnMut(&str),
) -> Result<i32, String> {
    let _marker = match marker() {
        Ok(marker) => Some(marker),
        Err(error) => {
            stderr(&error_line(&format!("MCP running marker failed: {error}")));
            None
        }
    };
    let runtime_dir = runtime()?;
    let parent = runtime_dir.parent().ok_or("runtime has no parent")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create runtime parent failed: {error}"))?;
    let lock_dir = std::fs::canonicalize(parent)
        .map_err(|error| format!("runtime identity failed: {error}"))?
        .join(crate::VERSION);
    // Same permanent external lock identity/order as normal GUI startup. Acquire
    // IN_USE before creating the deletable version tree, and retain it to wait.
    let _lease = effetune_bundle::wait_for_shared_lock(
        runtime_locks::open(&lock_dir, runtime_locks::IN_USE)
            .map_err(|error| format!("runtime lease failed: {error}"))?,
        Duration::from_secs(60),
    )
    .map_err(|error| format!("runtime lease failed: {error}"))?;
    std::fs::create_dir_all(&runtime_dir)
        .map_err(|error| format!("create runtime directory failed: {error}"))?;
    let extraction = effetune_bundle::wait_for_publish_lock(
        runtime_locks::open(&lock_dir, runtime_locks::EXTRACTION)
            .map_err(|error| format!("extraction lock failed: {error}"))?,
        Duration::from_secs(60),
    )
    .map_err(|error| format!("extraction lock failed: {error}"))?;
    let (name, bytes, hash) = crate::MCP_ASSET;
    let path = runtime_dir.join(name);
    // Diagnostics is deliberately not initialized in this lane. The shared
    // extractor's spans are inert; no path-bearing startup writer is created.
    crate::ensure_asset(
        &path,
        bytes,
        hash,
        crate::requires_content_hash_on_launch(name),
    )
    .map_err(|error| format!("extract MCP failed: {error}"))?;
    let mut command = command(&path, args);
    let mut child = spawn(&mut command).map_err(|error| format!("spawn MCP failed: {error}"))?;
    drop(extraction);
    child
        .wait_for_exit()
        .map_err(|error| format!("wait MCP failed: {error}"))
}

fn command(path: &std::path::Path, args: &[OsString]) -> Command {
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    }
    command
}

fn error_line(error: &str) -> String {
    format!("mImageViewer MCP: {}", error.replace(['\r', '\n'], " "))
}

pub(crate) fn complete(result: Result<i32, String>, mut stderr: impl FnMut(&str)) -> i32 {
    match result {
        Ok(code) => code,
        Err(error) => {
            stderr(&error_line(&error));
            1
        }
    }
}

#[cfg(windows)]
#[path = "mcp_marker.rs"]
mod marker;
#[cfg(windows)]
pub(crate) use marker::Marker;

#[cfg(not(windows))]
pub(crate) struct Marker;
#[cfg(not(windows))]
impl Marker {
    pub(crate) fn create() -> Result<Self, String> {
        Ok(Self)
    }
}

#[cfg(test)]
#[path = "mcp_launch_tests.rs"]
mod tests;
