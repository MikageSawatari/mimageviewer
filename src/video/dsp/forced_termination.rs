//! Diagnostic observations immediately before the existing host termination calls.

#[derive(Clone, Copy)]
pub(super) enum HostEnvironment {
    MivSound,
    UserVst,
}

impl From<&super::super::BridgeEnvironment> for HostEnvironment {
    fn from(environment: &super::super::BridgeEnvironment) -> Self {
        match environment {
            super::super::BridgeEnvironment::MivSound(_) => Self::MivSound,
            super::super::BridgeEnvironment::UserVst => Self::UserVst,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ForcedTerminationReason {
    DropAfterShutdown,
    DropWithoutShutdown,
    ExitFenceExpired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessLink {
    pid: u32,
    parent_pid: u32,
}

fn format_forced_termination(
    reason: ForcedTerminationReason,
    host_pid: u32,
    environment: HostEnvironment,
    shutdown_elapsed_ms: Option<u128>,
    webview2: Result<&[ProcessLink], &str>,
    try_wait_error: Option<&str>,
) -> String {
    let reason = match reason {
        ForcedTerminationReason::DropAfterShutdown => "drop_after_shutdown",
        ForcedTerminationReason::DropWithoutShutdown => "drop_without_shutdown",
        ForcedTerminationReason::ExitFenceExpired => "exit_fence_expired",
    };
    let environment = match environment {
        HostEnvironment::MivSound => "miv_sound",
        HostEnvironment::UserVst => "user_vst",
    };
    let elapsed = shutdown_elapsed_ms.map_or_else(|| "none".into(), |ms| ms.to_string());
    let webview2 = match webview2 {
        Ok(processes) => format!(
            "[{}]",
            processes
                .iter()
                .map(|process| format!("{}<-{}", process.pid, process.parent_pid))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Err(error) => format!("unavailable({})", error.replace(['\r', '\n'], " ")),
    };
    let mut line = format!(
        "[VST3] host forced termination reason={reason} host_pid={host_pid} environment={environment} shutdown_elapsed_ms={elapsed} webview2={webview2}"
    );
    if let Some(error) = try_wait_error {
        line.push_str(&format!(
            " try_wait=unavailable({})",
            error.replace(['\r', '\n'], " ")
        ));
    }
    line
}

struct ProcessRow {
    link: ProcessLink,
    is_webview2: bool,
}

/// Inspect both generations using the same inventory, independent of row order.
/// Intermediate children need not themselves be WebView2 executables.
fn webview2_descendants(host_pid: u32, rows: &[ProcessRow]) -> Vec<ProcessLink> {
    let children: std::collections::HashSet<u32> = rows
        .iter()
        .filter(|row| row.link.parent_pid == host_pid && row.link.pid != host_pid)
        .map(|row| row.link.pid)
        .collect();
    let mut descendants: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.is_webview2
                && row.link.pid != host_pid
                && (row.link.parent_pid == host_pid || children.contains(&row.link.parent_pid))
        })
        .map(|row| row.link)
        .collect();
    descendants.sort_unstable_by_key(|process| (process.parent_pid, process.pid));
    descendants
}

#[cfg(windows)]
fn snapshot_webview2_descendants(host_pid: u32) -> Result<Vec<ProcessLink>, String> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // Kept local: migration's handle owner and inventory also enforce user/UDF
    // ownership for recovery; this inventory only observes PID/parent/name.
    struct Snapshot(HANDLE);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: owns one successful ToolHelp snapshot handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    // SAFETY: valid snapshot flags; every iteration supplies a correctly sized,
    // initialized PROCESSENTRY32W. Snapshot closes the handle on every return.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!("snapshot: {}", std::io::Error::last_os_error()));
        }
        let snapshot = Snapshot(snapshot);
        let mut row: PROCESSENTRY32W = std::mem::zeroed();
        row.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut next = Process32FirstW(snapshot.0, &mut row);
        let mut rows = Vec::new();
        while next != 0 {
            let length = row
                .szExeFile
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(row.szExeFile.len());
            let name = String::from_utf16_lossy(&row.szExeFile[..length]);
            rows.push(ProcessRow {
                link: ProcessLink {
                    pid: row.th32ProcessID,
                    parent_pid: row.th32ParentProcessID,
                },
                is_webview2: name.eq_ignore_ascii_case("msedgewebview2.exe"),
            });
            next = Process32NextW(snapshot.0, &mut row);
        }
        let error = GetLastError();
        if error != ERROR_NO_MORE_FILES {
            return Err(format!(
                "process_inventory: {}",
                std::io::Error::from_raw_os_error(error as i32)
            ));
        }
        Ok(webview2_descendants(host_pid, &rows))
    }
}

#[cfg(windows)]
pub(super) fn log_forced_termination(
    reason: ForcedTerminationReason,
    host_pid: u32,
    environment: HostEnvironment,
    shutdown_elapsed_ms: Option<u128>,
    try_wait_error: Option<&str>,
) {
    let descendants = snapshot_webview2_descendants(host_pid);
    crate::logger::log(format_forced_termination(
        reason,
        host_pid,
        environment,
        shutdown_elapsed_ms,
        descendants.as_deref().map_err(String::as_str),
        try_wait_error,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_termination_drop_after_shutdown_with_webview2() {
        let processes = [
            ProcessLink {
                pid: 5678,
                parent_pid: 1234,
            },
            ProcessLink {
                pid: 5679,
                parent_pid: 5678,
            },
        ];
        assert_eq!(
            format_forced_termination(
                ForcedTerminationReason::DropAfterShutdown,
                1234,
                HostEnvironment::MivSound,
                Some(27),
                Ok(&processes),
                None,
            ),
            "[VST3] host forced termination reason=drop_after_shutdown host_pid=1234 environment=miv_sound shutdown_elapsed_ms=27 webview2=[5678<-1234,5679<-5678]"
        );
    }

    #[test]
    fn forced_termination_drop_without_shutdown_without_webview2() {
        assert_eq!(
            format_forced_termination(
                ForcedTerminationReason::DropWithoutShutdown,
                42,
                HostEnvironment::UserVst,
                None,
                Ok(&[]),
                None,
            ),
            "[VST3] host forced termination reason=drop_without_shutdown host_pid=42 environment=user_vst shutdown_elapsed_ms=none webview2=[]"
        );
    }

    #[test]
    fn forced_termination_exit_fence_expired_with_snapshot_error() {
        assert_eq!(
            format_forced_termination(
                ForcedTerminationReason::ExitFenceExpired,
                1234,
                HostEnvironment::MivSound,
                Some(5000),
                Err("snapshot: access denied (os error 5)"),
                None,
            ),
            "[VST3] host forced termination reason=exit_fence_expired host_pid=1234 environment=miv_sound shutdown_elapsed_ms=5000 webview2=unavailable(snapshot: access denied (os error 5))"
        );
    }

    #[test]
    fn forced_termination_drop_with_try_wait_and_inventory_errors_stays_one_line() {
        assert_eq!(
            format_forced_termination(
                ForcedTerminationReason::DropWithoutShutdown,
                42,
                HostEnvironment::UserVst,
                None,
                Err("process_inventory: read failed\n(os error 5)"),
                Some("query failed\r\n(os error 6)"),
            ),
            "[VST3] host forced termination reason=drop_without_shutdown host_pid=42 environment=user_vst shutdown_elapsed_ms=none webview2=unavailable(process_inventory: read failed (os error 5)) try_wait=unavailable(query failed  (os error 6))"
        );
    }

    #[test]
    fn forced_termination_exit_fence_without_shutdown() {
        assert_eq!(
            format_forced_termination(
                ForcedTerminationReason::ExitFenceExpired,
                42,
                HostEnvironment::UserVst,
                None,
                Ok(&[]),
                None,
            ),
            "[VST3] host forced termination reason=exit_fence_expired host_pid=42 environment=user_vst shutdown_elapsed_ms=none webview2=[]"
        );
    }

    #[test]
    fn webview2_descendants_include_two_generations_only_and_sort_links() {
        let row = |pid, parent_pid, is_webview2| ProcessRow {
            link: ProcessLink { pid, parent_pid },
            is_webview2,
        };
        let rows = [
            row(5679, 5678, true),
            row(9001, 9000, true), // another host
            row(5680, 5679, true), // third generation
            row(5678, 1234, true),
            row(6000, 1234, false), // non-WebView2 intermediate child
            row(6001, 6000, true),
        ];
        assert_eq!(
            webview2_descendants(1234, &rows),
            vec![
                ProcessLink {
                    pid: 5678,
                    parent_pid: 1234
                },
                ProcessLink {
                    pid: 5679,
                    parent_pid: 5678
                },
                ProcessLink {
                    pid: 6001,
                    parent_pid: 6000
                },
            ]
        );
        assert!(webview2_descendants(7, &rows).is_empty());
    }
}
