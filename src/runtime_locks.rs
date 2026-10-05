//! Cooperative launcher/core/collector ownership. Locks are held by File lifetime.
//! Path/reparse pre-checks do not defend against a same-user process swapping a
//! parent after inspection (accepted 2026-10-05); such a process can delete the
//! user's files directly. Never trust a missing lease as proof of no legacy reader.
//! All cooperative lock identities live in runtime/.locks, outside resource
//! trees. They are tiny, permanent files: unlinking one would split ownership.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

pub const IN_USE: &str = ".in-use.lock";
pub const EXTRACTION: &str = ".extract.lock";
pub const PUBLISHER: &str = ".effetune.lock";
pub const LOCKS: &str = ".locks";
pub const READY_ENV: &str = "MIV_RUNTIME_LEASE_READY";

pub fn open(directory: &Path, name: &str) -> io::Result<File> {
    open_file(&lock_path(directory, name)?, false)
}

/// Resource existence is deliberately unnecessary: after a resource disappears,
/// an acquirer must still contend on the collector's original lock identity.
pub fn lock_path(directory: &Path, name: &str) -> io::Result<PathBuf> {
    let (version_dir, suffix) = if directory
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("effetune"))
    {
        if name != IN_USE {
            return Err(io::Error::other("generation only has an in-use lock"));
        }
        let generation = safe_component(directory)?;
        (
            directory
                .parent()
                .unwrap()
                .parent()
                .ok_or_else(|| io::Error::other("no version directory"))?,
            format!("gen-{generation}"),
        )
    } else {
        let suffix = match name {
            IN_USE => "in-use",
            EXTRACTION => "extract",
            PUBLISHER => "effetune",
            _ => return Err(io::Error::other("unknown runtime lock kind")),
        };
        (directory, suffix.to_owned())
    };
    let version = safe_component(version_dir)?;
    let runtime = fs::canonicalize(
        version_dir
            .parent()
            .ok_or_else(|| io::Error::other("no runtime parent"))?,
    )?;
    checked_directory(&runtime)?;
    let locks = runtime.join(LOCKS);
    match fs::create_dir(&locks) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    checked_directory(&locks)?;
    let locks = fs::canonicalize(locks)?;
    if locks.parent() != Some(runtime.as_path()) {
        return Err(io::Error::other("lock directory escaped runtime"));
    }
    Ok(locks.join(format!("{version}.{suffix}")))
}

fn safe_component(path: &Path) -> io::Result<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-+_".contains(&b))
        })
        .ok_or_else(|| io::Error::other("invalid runtime lock identity"))
}

/// Honor released publishers too. Only a whole-version collector permits
/// deletion of this legacy in-tree file; its permanent external lock stays held.
pub fn open_legacy_publisher(version: &Path, allow_delete: bool) -> io::Result<File> {
    checked_directory(version)?;
    open_file(&version.join(PUBLISHER), allow_delete)
}

pub fn exclusive_legacy_publisher(version: &Path, allow_delete: bool) -> io::Result<Option<File>> {
    match fs::symlink_metadata(version.join(PUBLISHER)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    exclusive_file(open_legacy_publisher(version, allow_delete)?).map(Some)
}

fn open_file(path: &Path, allow_delete: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
        options.share_mode(if allow_delete { 1 | 2 | 4 } else { 1 | 2 });
    }
    let file = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            checked_file(&metadata)?;
            options.open(path)?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match options.write(true).create_new(true).open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    checked_file(&fs::symlink_metadata(&path)?)?;
                    options.write(false).create_new(false).open(path)?
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
    exclusive_file(open(directory, name)?)
}

fn exclusive_file(file: File) -> io::Result<File> {
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

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let version = temp.path().join("runtime/4.3.0");
        fs::create_dir_all(&version).unwrap();
        (temp, version)
    }

    #[test]
    fn multiple_readers_exclude_a_collector_until_last_reader_drops() {
        let (_temp, version) = fixture();
        let first = shared(&version).unwrap();
        let second = shared(&version).unwrap();
        assert!(exclusive(&version, IN_USE).is_err());
        drop(first);
        assert!(exclusive(&version, IN_USE).is_err());
        drop(second);
        let collector = exclusive(&version, IN_USE).unwrap();
        assert!(shared(&version).is_err());
        drop(collector);
        assert!(shared(&version).is_ok());
    }

    #[test]
    fn existing_read_only_lease_needs_no_write_access() {
        let (_temp, version) = fixture();
        drop(shared(&version).unwrap());
        let path = lock_path(&version, IN_USE).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        let original = permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let reader = shared(&version).unwrap();
        let second = shared(&version).unwrap();
        assert!(exclusive(&version, IN_USE).is_err());
        drop((reader, second));
        fs::set_permissions(path, original).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn handoff_overlaps_reader_leases_and_returns_before_child_exit() {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let (_temp, version) = fixture();
        let launcher = shared(&version).unwrap();
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
        let core = shared(&version).unwrap();
        signal_named(std::ffi::OsStr::new(&handoff.name)).unwrap();
        handoff.wait(&mut child).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        drop(launcher);
        assert!(exclusive(&version, IN_USE).is_err());
        drop(core);
        assert!(exclusive(&version, IN_USE).is_ok());
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

    #[cfg(windows)]
    #[test]
    fn permanent_lock_handle_denies_delete_and_works_before_resource_creation() {
        let (temp, version) = fixture();
        fs::remove_dir(&version).unwrap();
        let guard = shared(&version).unwrap();
        let path = lock_path(&version, IN_USE).unwrap();
        assert_eq!(
            path,
            fs::canonicalize(temp.path().join("runtime"))
                .unwrap()
                .join(".locks/4.3.0.in-use")
        );
        assert!(fs::remove_file(&path).is_err());
        fs::create_dir(&version).unwrap();
        assert!(exclusive(&version, IN_USE).is_err());
        drop(guard);
        assert!(exclusive(&version, IN_USE).is_ok());
    }
}
