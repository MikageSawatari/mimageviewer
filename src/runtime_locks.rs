//! Cooperative launcher/core/collector ownership. Locks are held by File lifetime.
//! Path/reparse pre-checks do not defend against a same-user process swapping a
//! parent after inspection (accepted 2026-10-05); such a process can delete the
//! user's files directly. Never trust a missing lease as proof of no legacy reader.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

pub const IN_USE: &str = ".in-use.lock";
pub const EXTRACTION: &str = ".extract.lock";
pub const PUBLISHER: &str = ".effetune.lock";
pub const READY_ENV: &str = "MIV_RUNTIME_LEASE_READY";

pub fn open(directory: &Path, name: &str) -> io::Result<File> {
    checked_directory(directory)?;
    let path = directory.join(name);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            checked_file(&metadata)?;
            options.open(&path)?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match options.write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    checked_file(&fs::symlink_metadata(&path)?)?;
                    options.write(false).create_new(false).open(&path)?
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    checked_file(&file.metadata()?)?;
    Ok(file)
}

fn checked_file(metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.is_file() || is_reparse(metadata) {
        return Err(io::Error::other("runtime lock is not a regular file"));
    }
    Ok(())
}

pub fn checked_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(io::Error::other(
            "runtime directory is invalid or a reparse point",
        ));
    }
    Ok(())
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub fn shared(directory: &Path) -> io::Result<File> {
    let file = open(directory, IN_USE)?;
    if !FileExt::try_lock_shared(&file)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "runtime is being collected",
        ));
    }
    Ok(file)
}

pub fn exclusive(directory: &Path, name: &str) -> io::Result<File> {
    let file = open(directory, name)?;
    if !FileExt::try_lock_exclusive(&file)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "runtime is in use",
        ));
    }
    Ok(file)
}

/// A successful resolution is inseparable from its shared generation lease.
#[derive(Debug)]
pub struct PinnedGeneration {
    pub path: PathBuf,
    _lease: File,
}

impl PinnedGeneration {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        checked_directory(&path)?;
        let path = fs::canonicalize(path)?;
        let lease = shared(&path)?;
        Ok(Self {
            path,
            _lease: lease,
        })
    }
}

#[cfg(windows)]
pub fn signal_ready() -> io::Result<()> {
    let Some(name) = std::env::var_os(READY_ENV) else {
        return Ok(());
    };
    signal_named(&name)
}

#[cfg(windows)]
fn signal_named(name: &std::ffi::OsStr) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    use windows::core::PCWSTR;
    let wide: Vec<_> = name.encode_wide().chain(Some(0)).collect();
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(wide.as_ptr())) }
        .map_err(io::Error::other)?;
    let result = unsafe { SetEvent(event) }.map_err(io::Error::other);
    let _ = unsafe { CloseHandle(event) };
    result
}

#[cfg(windows)]
pub struct Handoff {
    event: windows::Win32::Foundation::HANDLE,
    name: String,
}

#[cfg(windows)]
impl Handoff {
    pub fn new() -> io::Result<Self> {
        use windows::Win32::System::Threading::CreateEventW;
        use windows::core::PCWSTR;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let name = format!(
            "Local\\mimageviewer-runtime-ready-{}-{nonce}",
            std::process::id()
        );
        let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let event = unsafe { CreateEventW(None, true, false, PCWSTR(wide.as_ptr())) }
            .map_err(io::Error::other)?;
        Ok(Self { event, name })
    }

    pub fn configure(&self, command: &mut std::process::Command) {
        command.env(READY_ENV, &self.name);
    }

    /// The core never waits. Keep launcher leases until overlap is acknowledged,
    /// or until the child exits. There is deliberately no timeout releasing pins.
    pub fn wait(&self, child: &mut std::process::Child) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::{HANDLE, WAIT_FAILED};
        use windows::Win32::System::Threading::{INFINITE, WaitForMultipleObjects};
        let handles = [self.event, HANDLE(child.as_raw_handle())];
        let result = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if result == WAIT_FAILED {
            let error = io::Error::last_os_error();
            // Preserve pins on a failed handshake until process exit, not a timer.
            child.wait()?;
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Handoff {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.event) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_readers_exclude_a_collector_until_last_reader_drops() {
        let temp = tempfile::tempdir().unwrap();
        let first = shared(temp.path()).unwrap();
        let second = shared(temp.path()).unwrap();
        assert!(exclusive(temp.path(), IN_USE).is_err());
        drop(first);
        assert!(exclusive(temp.path(), IN_USE).is_err());
        drop(second);
        let collector = exclusive(temp.path(), IN_USE).unwrap();
        assert!(shared(temp.path()).is_err());
        drop(collector);
        assert!(shared(temp.path()).is_ok());
    }

    #[test]
    fn existing_read_only_lease_needs_no_write_access() {
        let temp = tempfile::tempdir().unwrap();
        drop(shared(temp.path()).unwrap());
        let path = temp.path().join(IN_USE);
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        let original = permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let reader = shared(temp.path()).unwrap();
        let second = shared(temp.path()).unwrap();
        assert!(exclusive(temp.path(), IN_USE).is_err());
        drop((reader, second));
        fs::set_permissions(path, original).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn handoff_overlaps_reader_leases_and_returns_before_child_exit() {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let temp = tempfile::tempdir().unwrap();
        let launcher = shared(temp.path()).unwrap();
        let handoff = Handoff::new().unwrap();
        // Disposable console child, hidden; never invokes the product or desktop.
        let mut child = Command::new("cmd.exe")
            .args(["/d", "/c", "set /p runtime_handoff_input="])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let core = shared(temp.path()).unwrap();
        signal_named(std::ffi::OsStr::new(&handoff.name)).unwrap();
        handoff.wait(&mut child).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        drop(launcher);
        assert!(exclusive(temp.path(), IN_USE).is_err());
        drop(core);
        assert!(exclusive(temp.path(), IN_USE).is_ok());
        drop(child.stdin.take());
        child.wait().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn handoff_without_ack_returns_when_child_exits() {
        use std::os::windows::process::CommandExt;
        let handoff = Handoff::new().unwrap();
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "exit 0"])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        handoff.wait(&mut child).unwrap();
        assert!(child.wait().unwrap().success());
    }
}
