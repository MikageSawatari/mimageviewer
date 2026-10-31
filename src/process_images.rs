//! Worker-only process executable snapshot, shared by all Windows build flavors.
use std::{io, path::PathBuf};

#[cfg(windows)]
pub(crate) fn snapshot() -> io::Result<Vec<(u32, PathBuf)>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::K32EnumProcesses;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    // Bounded storage, one enumeration, one limited-rights image query per PID.
    // No module enumeration, VM reads, subprocesses, waits or retry loops.
    let mut pids = vec![0u32; 16384];
    let capacity = (pids.len() * std::mem::size_of::<u32>()) as u32;
    let mut bytes = 0;
    if unsafe { K32EnumProcesses(pids.as_mut_ptr(), capacity, &mut bytes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if bytes >= capacity {
        return Err(io::Error::other("process snapshot truncated"));
    }
    let mut buffer = vec![0u16; 32768];
    let mut images = Vec::new();
    for pid in pids
        .into_iter()
        .take(bytes as usize / std::mem::size_of::<u32>())
    {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            continue; // Protected system process, or already exited.
        }
        let mut size = buffer.len() as u32;
        let found =
            unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        unsafe { CloseHandle(handle) };
        if found != 0 {
            images.push((
                pid,
                std::ffi::OsString::from_wide(&buffer[..size as usize]).into(),
            ));
        }
    }
    Ok(images)
}

#[cfg(not(windows))]
pub(crate) fn snapshot() -> io::Result<Vec<(u32, PathBuf)>> {
    Ok(Vec::new())
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn all_windows_build_flavors_enumerate_the_current_image() {
        assert!(
            super::snapshot()
                .unwrap()
                .iter()
                .any(|(pid, _)| *pid == std::process::id())
        );
    }
}
