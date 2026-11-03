//! Suspended Windows child startup with private standard-I/O inheritance.
//!
//! Callers run this on worker threads. Every child enters a kill-on-close Job
//! before its first instruction; dropping the child or using any kill handle
//! terminates the whole process tree, without waiting on the calling thread.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{
    AsHandle as _, AsRawHandle as _, BorrowedHandle, FromRawHandle as _, OwnedHandle,
};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
};
use windows::core::{PCWSTR, PWSTR};

#[derive(Clone, Copy)]
pub(crate) enum StdinMode {
    Null,
    Pipe,
}

/// Sharing this handle keeps the Job alive, but does not prevent JobChild's
/// explicit termination on drop. No kill path waits for child/thread cleanup.
#[derive(Clone)]
pub struct KillHandle(Arc<Job>);

struct Job {
    handle: OwnedHandle,
    killed: AtomicBool,
}

impl KillHandle {
    /// Request immediate termination of the worker tree without waiting for
    /// process exit or for the client's I/O threads to be collected.
    pub fn kill(&self) -> io::Result<()> {
        self.0.killed.store(true, Ordering::Release);
        unsafe { TerminateJobObject(HANDLE(self.0.handle.as_raw_handle()), 1) }.map_err(win_error)
    }

    /// Whether any owner has requested termination of this worker tree.
    pub fn is_killed(&self) -> bool {
        self.0.killed.load(Ordering::Acquire)
    }
}

pub(crate) struct JobChild {
    kill: KillHandle,
    process: OwnedHandle,
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

impl JobChild {
    pub(crate) fn kill_handle(&self) -> KillHandle {
        self.kill.clone()
    }

    pub(crate) fn process_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }

    pub(crate) fn exit_code(&self) -> io::Result<i32> {
        let mut code = 0;
        unsafe { GetExitCodeProcess(HANDLE(self.process.as_raw_handle()), &mut code) }
            .map_err(win_error)?;
        Ok(code as i32)
    }

    pub(crate) fn take_stdin(&mut self) -> io::Result<File> {
        self.stdin
            .take()
            .ok_or_else(|| io::Error::other("stdin unavailable or taken"))
    }

    pub(crate) fn take_stdout(&mut self) -> io::Result<File> {
        self.stdout
            .take()
            .ok_or_else(|| io::Error::other("stdout taken"))
    }

    pub(crate) fn take_stderr(&mut self) -> io::Result<File> {
        self.stderr
            .take()
            .ok_or_else(|| io::Error::other("stderr taken"))
    }
}

impl Drop for JobChild {
    fn drop(&mut self) {
        let _ = self.kill.kill();
    }
}

fn win_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "embedded NUL in child startup",
        ));
    }
    value.push(0);
    Ok(value)
}

/// Quote in UTF-16 so paths containing unpaired surrogates are not changed.
fn quote(value: &OsStr, out: &mut Vec<u16>) -> io::Result<()> {
    let value = wide(value)?;
    out.push(b'"' as u16);
    let mut slashes = 0;
    for &unit in &value[..value.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        let count = if unit == b'"' as u16 {
            slashes * 2 + 1
        } else {
            slashes
        };
        out.extend(std::iter::repeat_n(b'\\' as u16, count));
        slashes = 0;
        out.push(unit);
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    out.push(b'"' as u16);
    Ok(())
}

fn child_environment(environment: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
    let mut pairs: Vec<_> = environment.iter().collect();
    pairs.sort_by_key(|(key, _)| key.to_string_lossy().to_ascii_uppercase());
    let mut block = Vec::new();
    for (key, value) in pairs {
        let key = wide(key)?;
        let value = wide(value)?;
        block.extend_from_slice(&key[..key.len() - 1]);
        block.push(b'=' as u16);
        block.extend(value);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// The child endpoint alone remains inheritable. The explicit handle list
/// below prevents another helper child inheriting it during concurrent spawn.
fn pipe(parent_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: true.into(),
        ..Default::default()
    };
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe { CreatePipe(&mut read, &mut write, Some(&attrs), 0) }.map_err(win_error)?;
    let read = unsafe { OwnedHandle::from_raw_handle(read.0) };
    let write = unsafe { OwnedHandle::from_raw_handle(write.0) };
    let (parent, child) = if parent_reads {
        (read, write)
    } else {
        (write, read)
    };
    unsafe {
        SetHandleInformation(
            HANDLE(parent.as_raw_handle()),
            HANDLE_FLAG_INHERIT.0,
            Default::default(),
        )
    }
    .map_err(win_error)?;
    Ok((parent, child))
}

/// Owns aligned backing storage and deletes the initialized attribute list.
struct AttributeList {
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl AttributeList {
    fn handles(handles: &[HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        // The sizing call deliberately returns ERROR_INSUFFICIENT_BUFFER.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes) }
            .map_err(win_error)?;
        let owner = Self {
            _storage: storage,
            list,
        };
        unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                std::mem::size_of_val(handles),
                None,
                None,
            )
        }
        .map_err(win_error)?;
        Ok(owner)
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

/// Before assignment succeeds, closing the Job cannot kill the suspended
/// child. This guard covers every failure after CreateProcessW succeeds.
struct StartingProcess(Option<OwnedHandle>);

impl Drop for StartingProcess {
    fn drop(&mut self) {
        if let Some(process) = &self.0 {
            let _ = unsafe { TerminateProcess(HANDLE(process.as_raw_handle()), 1) };
        }
    }
}

/// `args` excludes argv[0]; `environment` is the complete child environment.
pub(crate) fn spawn(
    executable: &Path,
    args: &[OsString],
    environment: &[(OsString, OsString)],
    stdin_mode: StdinMode,
) -> io::Result<JobChild> {
    let exe = wide(executable.as_os_str())?;
    let mut cmd = Vec::new();
    quote(executable.as_os_str(), &mut cmd)?;
    for arg in args {
        cmd.push(b' ' as u16);
        quote(arg, &mut cmd)?;
    }
    cmd.push(0);
    let env = child_environment(environment)?;

    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(win_error)?;
    let job = unsafe { OwnedHandle::from_raw_handle(job.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            HANDLE(job.as_raw_handle()),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .map_err(win_error)?;
    let kill = KillHandle(Arc::new(Job {
        handle: job,
        killed: AtomicBool::new(false),
    }));
    let (stdout, child_stdout) = pipe(true)?;
    let (stderr, child_stderr) = pipe(true)?;
    let (stdin, child_stdin) = match stdin_mode {
        StdinMode::Pipe => {
            let (parent, child) = pipe(false)?;
            (Some(File::from(parent)), child)
        }
        StdinMode::Null => {
            let child: OwnedHandle = File::open("NUL")?.into();
            unsafe {
                SetHandleInformation(
                    HANDLE(child.as_raw_handle()),
                    HANDLE_FLAG_INHERIT.0,
                    HANDLE_FLAG_INHERIT,
                )
            }
            .map_err(win_error)?;
            (None, child)
        }
    };
    let handles = [
        HANDLE(child_stdin.as_raw_handle()),
        HANDLE(child_stdout.as_raw_handle()),
        HANDLE(child_stderr.as_raw_handle()),
    ];
    let attributes = AttributeList::handles(&handles)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.list;
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR(exe.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            Some(env.as_ptr().cast()),
            PCWSTR::null(),
            &startup.StartupInfo,
            &mut pi,
        )
    }
    .map_err(win_error)?;
    let mut process = StartingProcess(Some(unsafe { OwnedHandle::from_raw_handle(pi.hProcess.0) }));
    let thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread.0) };
    unsafe { AssignProcessToJobObject(HANDLE(kill.0.handle.as_raw_handle()), pi.hProcess) }
        .map_err(win_error)?;
    if unsafe { ResumeThread(pi.hThread) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    drop(thread);
    drop(child_stdin);
    drop(child_stdout);
    drop(child_stderr);
    Ok(JobChild {
        kill,
        process: process.0.take().expect("starting process owned"),
        stdin,
        stdout: Some(File::from(stdout)),
        stderr: Some(File::from(stderr)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Win32::Foundation::{GetHandleInformation, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;

    fn shell(script: &str, stdin: StdinMode) -> JobChild {
        let exe = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "C:\\Windows".into())
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let args = ["-NoProfile", "-NonInteractive", "-Command", script].map(OsString::from);
        spawn(&exe, &args, &std::env::vars_os().collect::<Vec<_>>(), stdin).unwrap()
    }

    fn assert_not_inheritable(file: &File) {
        let mut flags = 0;
        unsafe { GetHandleInformation(HANDLE(file.as_raw_handle()), &mut flags) }.unwrap();
        assert_eq!(flags & HANDLE_FLAG_INHERIT.0, 0);
    }

    fn wait(child: &JobChild) {
        assert_eq!(
            unsafe { WaitForSingleObject(HANDLE(child.process_handle().as_raw_handle()), 10_000) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn job_process_parent_pipes_are_private_and_roundtrip() {
        let mut child = shell(
            "$line = [Console]::ReadLine(); [Console]::Out.Write($line); [Console]::Error.Write('diagnostic')",
            StdinMode::Pipe,
        );
        let mut stdin = child.take_stdin().unwrap();
        let mut stdout = child.take_stdout().unwrap();
        let mut stderr = child.take_stderr().unwrap();
        for file in [&stdin, &stdout, &stderr] {
            assert_not_inheritable(file);
        }
        stdin.write_all(b"roundtrip\n").unwrap();
        drop(stdin);
        wait(&child);
        assert_eq!(child.exit_code().unwrap(), 0);
        let mut out = String::new();
        stdout.read_to_string(&mut out).unwrap();
        assert_eq!(out, "roundtrip");
        let mut err = String::new();
        stderr.read_to_string(&mut err).unwrap();
        assert_eq!(err, "diagnostic");
    }

    #[test]
    fn job_process_unrelated_inheritable_pipe_is_not_inherited() {
        // This pipe exists before startup, just as another simultaneously
        // starting worker's child stdout endpoint can exist. A broad inherit
        // would keep its writer alive until this child exits.
        let (read, unrelated_child_write) = pipe(true).unwrap();
        let mut child = shell("[Console]::ReadLine() | Out-Null", StdinMode::Pipe);
        let stdin = child.take_stdin().unwrap();
        drop(unrelated_child_write);
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut file = File::from(read);
            let mut byte = [0];
            let _ = tx.send(file.read(&mut byte));
        });
        let result = rx.recv_timeout(Duration::from_secs(5));
        child.kill_handle().kill().unwrap();
        wait(&child);
        drop(stdin);
        reader.join().unwrap();
        assert!(
            matches!(result, Ok(Ok(0))),
            "unexpected inherited writer: {result:?}"
        );
    }

    #[test]
    fn job_process_null_stdin_and_drop_kill_shared_job() {
        let mut null_child = shell(
            "if ($null -ne [Console]::ReadLine()) { exit 9 }",
            StdinMode::Null,
        );
        assert!(null_child.take_stdin().is_err());
        wait(&null_child);
        assert_eq!(null_child.exit_code().unwrap(), 0);

        let mut child = shell("[Console]::ReadLine() | Out-Null", StdinMode::Pipe);
        let stdin = child.take_stdin().unwrap();
        let killer = child.kill_handle();
        let process = child.process_handle().try_clone_to_owned().unwrap();
        assert!(!killer.is_killed());
        drop(child);
        assert!(killer.is_killed());
        assert_eq!(
            unsafe { WaitForSingleObject(HANDLE(process.as_raw_handle()), 10_000) },
            WAIT_OBJECT_0
        );
        drop(stdin);
    }

    #[test]
    fn job_process_rejects_nul_before_startup() {
        let result = spawn(
            Path::new("invalid.exe"),
            &[OsString::from("a\0b")],
            &[],
            StdinMode::Null,
        );
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidInput));
    }
}
