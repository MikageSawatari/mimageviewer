//! Windows ownership boundary: a nonsharing Chromium lock plus synchronized
//! process handles. No WebView controller is created by this module.

use super::{MigrationBackend, Owner, ProcessCandidate, io_error};
use std::ffi::{OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::ptr::null_mut;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows::Win32::UI::Shell::CommandLineToArgvW;
use windows::core::PCWSTR;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_MORE_FILES, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetLengthSid, GetTokenInformation, IsValidSid, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_FLAG_DELETE_ON_CLOSE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

pub(super) struct Backend {
    started: Instant,
    current_sid: Option<Vec<u8>>,
}

impl Backend {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            current_sid: unsafe { user_sid(GetCurrentProcess()) },
        }
    }
}

pub(super) struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this type only owns successful, non-pseudo API handles.
        unsafe { CloseHandle(self.0) };
    }
}

impl MigrationBackend for Backend {
    type Lease = File;
    type Process = OwnedHandle;

    fn diagnostic_hosts(&mut self) {
        // This independent snapshot also runs when lock acquisition will fail.
        // It neither returns a migration decision nor waits for any host.
        if let Err(reason) = diagnostic_hosts() {
            crate::logger::log(format!(
                "[EffeTune migration] host inventory unavailable: {reason}"
            ));
        }
    }

    fn lock(&mut self, path: &Path) -> Result<File, String> {
        // Existing files are opened without CREATE_ALWAYS, truncation, writes,
        // delete-on-close, or attribute changes. The original bytes survive.
        match OpenOptions::new().read(true).share_mode(0).open(path) {
            Ok(file) => Ok(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // CREATE_NEW never replaces a racing browser's lockfile.
                // DELETE access is explicit for FILE_FLAG_DELETE_ON_CLOSE.
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                    .share_mode(0)
                    .create_new(true)
                    .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
                    .open(path)
                    .map_err(|e| io_error("create temporary exclusive WebView2 lock", path, e))
            }
            Err(e) => Err(io_error("open exclusive WebView2 lock", path, e)),
        }
    }

    fn processes(&mut self) -> Result<Vec<ProcessCandidate<OwnedHandle>>, String> {
        // SAFETY: snapshot and process handles are immediately owned by RAII.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(format!(
                    "enumerate WebView2 processes: {}",
                    io::Error::last_os_error()
                ));
            }
            let snapshot = OwnedHandle(snapshot);
            let mut row: PROCESSENTRY32W = zeroed();
            row.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut next = Process32FirstW(snapshot.0, &mut row);
            let mut result = Vec::new();
            while next != 0 {
                let length = row
                    .szExeFile
                    .iter()
                    .position(|&ch| ch == 0)
                    .unwrap_or(row.szExeFile.len());
                let name = String::from_utf16_lossy(&row.szExeFile[..length]);
                if name.eq_ignore_ascii_case("msedgewebview2.exe") {
                    let query = open_process(PROCESS_QUERY_LIMITED_INFORMATION, row.th32ProcessID);
                    let command = match query.as_ref() {
                        Some(handle) => match command_line(handle.0) {
                            Ok(command) => Some(command),
                            Err(reason) => {
                                crate::logger::log(format!(
                                    "[EffeTune migration] WebView2 PID {} command read failed: {reason}",
                                    row.th32ProcessID
                                ));
                                None
                            }
                        },
                        None => {
                            crate::logger::log(format!(
                                "[EffeTune migration] WebView2 PID {} query handle unavailable: {}",
                                row.th32ProcessID,
                                io::Error::last_os_error()
                            ));
                            None
                        }
                    };
                    let owner = match (
                        self.current_sid.as_ref(),
                        query.as_ref().and_then(|handle| user_sid(handle.0)),
                    ) {
                        (Some(current), Some(other)) => {
                            if EqualSid(
                                current.as_ptr() as *mut c_void,
                                other.as_ptr() as *mut c_void,
                            ) != 0
                            {
                                Owner::SameUser
                            } else {
                                Owner::OtherUser
                            }
                        }
                        _ => Owner::Unknown,
                    };
                    // A terminating process can deny command-line/token reads
                    // while still granting SYNCHRONIZE. Do not discard it.
                    let synchronize = open_process(PROCESS_SYNCHRONIZE, row.th32ProcessID);
                    if synchronize.is_none() {
                        crate::logger::log(format!(
                            "[EffeTune migration] WebView2 PID {} synchronization handle unavailable: {}",
                            row.th32ProcessID,
                            io::Error::last_os_error()
                        ));
                    }
                    result.push(ProcessCandidate {
                        pid: row.th32ProcessID,
                        command,
                        owner,
                        synchronize,
                    });
                }
                next = Process32NextW(snapshot.0, &mut row);
            }
            if GetLastError() != ERROR_NO_MORE_FILES {
                return Err(format!(
                    "read WebView2 process inventory: {}",
                    io::Error::last_os_error()
                ));
            }
            Ok(result)
        }
    }

    fn wait(&mut self, handle: OwnedHandle, remaining: Duration) -> Result<bool, String> {
        // Round down and never exceed the shared deadline. A zero-ms final
        // check is fine; callers reject zero remaining before reaching here.
        let milliseconds = remaining.as_millis().min(u32::MAX as u128) as u32;
        match unsafe { WaitForSingleObject(handle.0, milliseconds) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(format!(
                "wait for WebView2 exit: {}",
                io::Error::last_os_error()
            )),
        }
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn matches_udf(&self, command: &str, expected: &Path) -> Result<bool, String> {
        matches_udf(command, expected)
    }
}

unsafe fn open_process(access: u32, pid: u32) -> Option<OwnedHandle> {
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if handle.is_null() {
        None
    } else {
        Some(OwnedHandle(handle))
    }
}

fn diagnostic_hosts() -> Result<(), String> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().to_string());
        }
        let snapshot = OwnedHandle(snapshot);
        let mut row: PROCESSENTRY32W = zeroed();
        row.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut next = Process32FirstW(snapshot.0, &mut row);
        let mut hosts = 0;
        while next != 0 {
            let length = row
                .szExeFile
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(row.szExeFile.len());
            let name = String::from_utf16_lossy(&row.szExeFile[..length]);
            if row.th32ProcessID != GetCurrentProcessId()
                && [
                    "mimageviewer-core.exe",
                    "mimageviewer.exe",
                    "mimageviewer-vst3-host.exe",
                ]
                .iter()
                .any(|expected| name.eq_ignore_ascii_case(expected))
            {
                hosts += 1;
                crate::logger::log(format!(
                    "[EffeTune migration] observed host {name} PID {} (diagnostic only)",
                    row.th32ProcessID
                ));
            }
            next = Process32NextW(snapshot.0, &mut row);
        }
        if GetLastError() != ERROR_NO_MORE_FILES {
            return Err(io::Error::last_os_error().to_string());
        }
        crate::logger::log(format!(
            "[EffeTune migration] other mIV/host inventory: {hosts} process(es) (diagnostic only)"
        ));
        Ok(())
    }
}

unsafe fn user_sid(process: HANDLE) -> Option<Vec<u8>> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let token = OwnedHandle(token);
    let mut bytes = 0;
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut bytes) };
    if bytes < size_of::<TOKEN_USER>() as u32 || bytes > 1024 * 1024 {
        return None;
    }
    let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    } == 0
    {
        return None;
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    if unsafe { IsValidSid(user.User.Sid) } == 0 {
        return None;
    }
    let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    Some(unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), length) }.to_vec())
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

unsafe fn command_line(process: HANDLE) -> Result<String, String> {
    // ProcessCommandLineInformation (60) copies UNICODE_STRING and its content
    // into our own buffer. No cross-bitness PEB offsets or ReadProcessMemory.
    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
    let mut bytes = 0;
    let initial_status = unsafe {
        NtQueryInformationProcess(
            process,
            PROCESS_COMMAND_LINE_INFORMATION,
            null_mut(),
            0,
            &mut bytes,
        )
    };
    if bytes < size_of::<UnicodeString>() as u32 || bytes > 1024 * 1024 {
        return Err(format!(
            "command-line size={bytes}, NtQueryInformationProcess status=0x{:08X}",
            initial_status as u32
        ));
    }
    let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            PROCESS_COMMAND_LINE_INFORMATION,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    };
    if status < 0 {
        // Includes STATUS_PROCESS_IS_TERMINATING: the caller still waits using
        // its separately acquired synchronization handle.
        return Err(format!(
            "NtQueryInformationProcess status=0x{:08X}",
            status as u32
        ));
    }
    let text = unsafe { &*buffer.as_ptr().cast::<UnicodeString>() };
    let first = buffer.as_ptr() as usize;
    let end = first + buffer.len() * size_of::<usize>();
    let begin = text.buffer as usize;
    if text.length % 2 != 0
        || begin % 2 != 0
        || begin < first
        || begin
            .checked_add(text.length as usize)
            .is_none_or(|last| last > end)
    {
        return Err("invalid command-line UNICODE_STRING buffer".into());
    }
    let utf16 = unsafe { std::slice::from_raw_parts(text.buffer, text.length as usize / 2) };
    let command =
        String::from_utf16(utf16).map_err(|e| format!("invalid command-line UTF-16: {e}"))?;
    if command.is_empty() || command.contains('\0') {
        Err("empty command line or embedded NUL".into())
    } else {
        Ok(command)
    }
}

/// Use Windows' own argv rules: quoted `--user-data-dir=...`, separate values,
/// and renderer/crashpad arguments all pass through the same exact comparison.
pub(super) fn matches_udf(command: &str, expected: &Path) -> Result<bool, String> {
    let command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let mut count = 0;
    let argv = unsafe { CommandLineToArgvW(PCWSTR(command.as_ptr()), &mut count) };
    if argv.is_null() {
        return Err(format!(
            "parse WebView2 argv: {}",
            io::Error::last_os_error()
        ));
    }
    struct Argv(*mut windows::core::PWSTR);
    impl Drop for Argv {
        fn drop(&mut self) {
            unsafe { LocalFree(Some(HLOCAL(self.0.cast()))) };
        }
    }
    let _argv_owner = Argv(argv);
    let args = unsafe { std::slice::from_raw_parts(argv, count.max(0) as usize) };
    let mut values = Vec::new();
    let mut index = 1;
    while index < args.len() {
        let argument =
            unsafe { args[index].to_string() }.map_err(|e| format!("decode WebView2 argv: {e}"))?;
        if argument.eq_ignore_ascii_case("--user-data-dir") {
            index += 1;
            if index >= args.len() {
                return Err("WebView2 --user-data-dir is missing its value".into());
            }
            values.push(
                unsafe { args[index].to_string() }
                    .map_err(|e| format!("decode WebView2 user-data-dir: {e}"))?,
            );
        } else if argument
            .get(..16)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("--user-data-dir="))
        {
            values.push(argument[16..].to_owned());
        }
        index += 1;
    }
    if values.is_empty() {
        return Ok(false);
    }
    if values.len() != 1 {
        return Err("WebView2 has ambiguous duplicate --user-data-dir switches".into());
    }
    if values[0].is_empty() {
        return Err("WebView2 --user-data-dir has an empty value".into());
    }
    let actual = normalized_path(Path::new(&values[0]))?;
    let expected = normalized_path(expected)?;
    Ok(unsafe { CompareStringOrdinal(&actual, &expected, true) } == CSTR_EQUAL)
}

fn normalized_path(path: &Path) -> Result<Vec<u16>, String> {
    // WebView arguments normally carry absolute paths. Resolving an observed
    // relative path against *our* CWD could falsely exclude a source process;
    // treat this as unreadable so the ownership/handle policy applies.
    let text = path.as_os_str().to_string_lossy().replace('/', "\\");
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err("relative WebView2 user-data-dir has unknown process working directory".into());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    // Convert verbatim DOS/UNC prefixes into the ordinary spelling too.
    let wide: Vec<u16> = normalized.as_os_str().encode_wide().collect();
    let text = OsString::from_wide(&wide).to_string_lossy().into_owned();
    let text = if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
    };
    Ok(text.trim_end_matches('\\').encode_utf16().collect())
}

#[cfg(test)]
pub(super) fn test_current_process_query() {
    let command =
        unsafe { command_line(GetCurrentProcess()) }.expect("native current-process command line");
    assert!(!command.is_empty());
    let sid = unsafe { user_sid(GetCurrentProcess()) }.expect("current user SID");
    assert!(unsafe { IsValidSid(sid.as_ptr() as *mut c_void) } != 0);
}
