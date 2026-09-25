//! S1 Job Object acceptance probe. This binary is not distributed.
#![cfg(windows)]

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    mem::{size_of, zeroed},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_BAD_LENGTH, ERROR_MORE_DATA, ERROR_NO_MORE_FILES, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION,
                ResumeThread, STARTUPINFOW, TerminateProcess,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

#[derive(Default, Clone, Serialize, Deserialize)]
struct Summary {
    mode: String,
    converter_pid: u32,
    job_pids: BTreeSet<u32>,
    pre_cancel_job_pids: BTreeSet<u32>,
    pre_cancel_webview_pids: BTreeSet<u32>,
    webview_pids: BTreeSet<u32>,
    webview_outside_job: BTreeSet<u32>,
    gone: bool,
    pre_cancel_alive: bool,
    termination_ms: Option<u128>,
    success: bool,
    message: String,
}

#[derive(Serialize, Deserialize)]
struct OwnerState {
    summary: Summary,
    ancestry: HashMap<u32, u32>,
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct OwnerGuard(Child);
impl Drop for OwnerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// CreateProcessW requires the same quoting rules as CommandLineToArgvW.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in s.chars() {
        match ch {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(slashes));
                slashes = 0;
                out.push(ch);
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

fn job_members(job: HANDLE) -> Result<BTreeSet<u32>, String> {
    let mut capacity = 32usize;
    loop {
        let words = (size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()
            + (capacity - 1) * size_of::<usize>())
        .div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; words];
        let bytes = (storage.len() * size_of::<usize>()) as u32;
        let result = unsafe {
            QueryInformationJobObject(
                Some(job),
                JobObjectBasicProcessIdList,
                storage.as_mut_ptr().cast(),
                bytes,
                None,
            )
        };
        if let Err(error) = result {
            if ![ERROR_BAD_LENGTH.to_hresult(), ERROR_MORE_DATA.to_hresult()]
                .contains(&error.code())
                || capacity >= 16_384
            {
                return Err(format!("QueryInformationJobObject: {error}"));
            }
            capacity *= 2;
            continue;
        }
        let info = unsafe { &*(storage.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()) };
        if info.NumberOfAssignedProcesses as usize > capacity
            || info.NumberOfProcessIdsInList as usize > capacity
        {
            capacity = (info
                .NumberOfAssignedProcesses
                .max(info.NumberOfProcessIdsInList) as usize)
                + 16;
            continue;
        }
        let count = info.NumberOfProcessIdsInList as usize;
        let ids = unsafe { std::slice::from_raw_parts(info.ProcessIdList.as_ptr(), count) };
        return Ok(ids.iter().map(|id| *id as u32).collect());
    }
}

#[derive(Clone)]
struct Process {
    parent: u32,
    name: String,
}

fn snapshot() -> Result<HashMap<u32, Process>, String> {
    let handle = OwnedHandle(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(|e| e.to_string())?,
    );
    let mut item: PROCESSENTRY32W = unsafe { zeroed() };
    item.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut processes = HashMap::new();
    unsafe { Process32FirstW(handle.0, &mut item) }.map_err(|e| e.to_string())?;
    loop {
        let len = item
            .szExeFile
            .iter()
            .position(|x| *x == 0)
            .unwrap_or(item.szExeFile.len());
        processes.insert(
            item.th32ProcessID,
            Process {
                parent: item.th32ParentProcessID,
                name: String::from_utf16_lossy(&item.szExeFile[..len]),
            },
        );
        if let Err(error) = unsafe { Process32NextW(handle.0, &mut item) } {
            if error.code() != ERROR_NO_MORE_FILES.to_hresult() {
                return Err(format!("Process32NextW: {error}"));
            }
            break;
        }
    }
    Ok(processes)
}

fn descendants_of(
    converter: u32,
    processes: &HashMap<u32, Process>,
    ancestry: &HashMap<u32, u32>,
) -> BTreeSet<u32> {
    processes
        .iter()
        .filter_map(|(&pid, process)| {
            if !process.name.eq_ignore_ascii_case("msedgewebview2.exe") {
                return None;
            }
            let mut parent = process.parent;
            let mut visited = BTreeSet::new();
            while parent != 0 && visited.insert(parent) {
                if parent == converter {
                    return Some(pid);
                }
                parent = processes
                    .get(&parent)
                    .map(|p| p.parent)
                    .or_else(|| ancestry.get(&parent).copied())
                    .unwrap_or(0);
            }
            None
        })
        .collect()
}

fn observe(
    summary: &mut Summary,
    job: HANDLE,
    ancestry: &mut HashMap<u32, u32>,
) -> Result<(), String> {
    let mut members = job_members(job)?;
    let processes = snapshot()?;
    ancestry.extend(processes.iter().map(|(&pid, p)| (pid, p.parent)));
    let webviews = descendants_of(summary.converter_pid, &processes, ancestry);
    // Bracket the snapshot so a short-lived child can still be accounted for.
    members.extend(job_members(job)?);
    summary.job_pids.extend(&members);
    summary
        .webview_outside_job
        .extend(webviews.difference(&members));
    summary.webview_pids.extend(webviews);
    Ok(())
}

fn wait_gone(summary: &mut Summary, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let live = snapshot()?;
        if cancel_targets_gone(summary, &live) {
            summary.gone = true;
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("converter or WebView2 processes survived Job close".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn cancel_targets_gone(summary: &Summary, live: &HashMap<u32, Process>) -> bool {
    !live.contains_key(&summary.converter_pid)
        && summary
            .pre_cancel_webview_pids
            .iter()
            .all(|pid| !live.contains_key(pid))
}

fn verify_live_members(
    summary: &Summary,
    live: &HashMap<u32, Process>,
    members: &BTreeSet<u32>,
    ancestry: &HashMap<u32, u32>,
) -> Result<BTreeSet<u32>, String> {
    if summary.webview_pids.is_empty() {
        return Err("no WebView2 process was observed".into());
    }
    if !summary.webview_outside_job.is_empty() {
        return Err(format!(
            "WebView2 PIDs outside Job: {:?}",
            summary.webview_outside_job
        ));
    }
    if !live.contains_key(&summary.converter_pid) {
        return Err("converter already exited before cancel".into());
    }
    if !members.contains(&summary.converter_pid) {
        return Err("converter not in Job before cancel".into());
    }
    let current = current_webviews(summary, live, ancestry);
    if current.is_empty() {
        return Err("no WebView2 process is alive before cancel".into());
    }
    for pid in &current {
        if !members.contains(pid) {
            return Err(format!("WebView2 PID {pid} not in Job before cancel"));
        }
    }
    Ok(current)
}

fn current_webviews(
    summary: &Summary,
    live: &HashMap<u32, Process>,
    ancestry: &HashMap<u32, u32>,
) -> BTreeSet<u32> {
    let mut current = descendants_of(summary.converter_pid, live, ancestry);
    // An observed renderer can outlive the intermediate process in its parent chain.
    current.extend(summary.webview_pids.iter().copied().filter(|pid| {
        live.get(pid)
            .is_some_and(|process| process.name.eq_ignore_ascii_case("msedgewebview2.exe"))
    }));
    current
}

fn check_before_cancel(
    summary: &mut Summary,
    job: HANDLE,
    ancestry: &mut HashMap<u32, u32>,
) -> Result<(), String> {
    let mut members = job_members(job)?;
    let live = snapshot()?;
    ancestry.extend(live.iter().map(|(&pid, p)| (pid, p.parent)));
    members.extend(job_members(job)?);
    let current = current_webviews(summary, &live, ancestry);
    summary
        .webview_outside_job
        .extend(current.difference(&members));
    let current = verify_live_members(summary, &live, &members, ancestry)?;
    summary.webview_pids.extend(&current);
    summary.pre_cancel_webview_pids = current;
    summary.pre_cancel_job_pids = members.clone();
    summary.job_pids.extend(members);
    summary.pre_cancel_alive = true;
    Ok(())
}

fn start_converter(
    input: &Path,
    output_dir: &Path,
) -> Result<(OwnedHandle, OwnedHandle, OwnedHandle, u32), String> {
    fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
    let job =
        OwnedHandle(unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(|e| e.to_string())?);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .map_err(|e| e.to_string())?;
    let exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("mimageviewer-epub-pdf.exe");
    let exe_str = exe.to_string_lossy();
    let args = [
        exe_str.to_string(),
        "convert".into(),
        input.to_string_lossy().into_owned(),
        output_dir.join("result.pdf").to_string_lossy().into_owned(),
        "--work-dir".into(),
        output_dir.join("work").to_string_lossy().into_owned(),
        "--user-data-dir".into(),
        output_dir.join("user-data").to_string_lossy().into_owned(),
        "--timeout-secs".into(),
        "600".into(),
        "--progress-json".into(),
    ];
    let mut command = wide(&args.iter().map(|x| quote(x)).collect::<Vec<_>>().join(" "));
    let app = wide(&exe_str);
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR(app.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_SUSPENDED | CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &startup,
            &mut pi,
        )
    }
    .map_err(|e| format!("CreateProcessW: {e}"))?;
    let process = OwnedHandle(pi.hProcess);
    let thread_handle = OwnedHandle(pi.hThread);
    if let Err(e) = unsafe { AssignProcessToJobObject(job.0, process.0) } {
        let _ = unsafe { TerminateProcess(process.0, 1) };
        return Err(format!("AssignProcessToJobObject: {e}"));
    }
    if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
        return Err("ResumeThread failed".into());
    }
    Ok((job, process, thread_handle, pi.dwProcessId))
}

fn run_owner(input: &Path, output_dir: &Path, state: Option<&Path>, timeout: Duration) -> Summary {
    let mut summary = Summary {
        mode: if state.is_some() {
            "parent-kill"
        } else {
            "cancel"
        }
        .into(),
        ..Summary::default()
    };
    let result = (|| -> Result<(), String> {
        let (job, _process, _thread, pid) = start_converter(input, output_dir)?;
        summary.converter_pid = pid;
        let deadline = Instant::now() + timeout;
        let mut ancestry = HashMap::new();
        let mut first_webview = None;
        loop {
            observe(&mut summary, job.0, &mut ancestry)?;
            if !summary.webview_pids.is_empty() {
                first_webview.get_or_insert_with(Instant::now);
            }
            if first_webview.is_some_and(|time: Instant| time.elapsed() >= Duration::from_secs(2)) {
                break;
            }
            if Instant::now() >= deadline {
                return Err("WebView2 processes did not appear before deadline".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
        loop {
            observe(&mut summary, job.0, &mut ancestry)?;
            match check_before_cancel(&mut summary, job.0, &mut ancestry) {
                Ok(()) => break,
                Err(e)
                    if e == "no WebView2 process is alive before cancel"
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(path) = state {
            summary.message = "ready".into();
            loop {
                fs::write(
                    path,
                    serde_json::to_vec(&OwnerState {
                        summary: summary.clone(),
                        ancestry: ancestry.clone(),
                    })
                    .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                thread::sleep(Duration::from_millis(100));
                observe(&mut summary, job.0, &mut ancestry)?;
                match check_before_cancel(&mut summary, job.0, &mut ancestry) {
                    Ok(()) => {}
                    Err(e) if e == "no WebView2 process is alive before cancel" => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        let stopped_at = Instant::now();
        drop(job);
        let gone = wait_gone(&mut summary, Duration::from_secs(20));
        summary.termination_ms = Some(stopped_at.elapsed().as_millis());
        gone?;
        summary.success = summary.pre_cancel_alive
            && !summary.pre_cancel_webview_pids.is_empty()
            && summary.webview_outside_job.is_empty()
            && summary.gone;
        Ok(())
    })();
    if let Err(e) = result {
        summary.message = e;
    } else {
        summary.message = "all checks passed".into();
    }
    summary
}

fn run_parent_kill(input: &Path, output_dir: &Path, timeout: Duration) -> Summary {
    let state = output_dir.join("owner-state.json");
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::remove_file(&state);
    let mut summary = Summary {
        mode: "parent-kill".into(),
        ..Summary::default()
    };
    let result = (|| -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut owner = OwnerGuard(
            Command::new(exe)
                .arg("owner")
                .arg(input)
                .arg(output_dir)
                .arg(&state)
                .arg(timeout.as_secs().to_string())
                .creation_flags(CREATE_NO_WINDOW.0)
                .spawn()
                .map_err(|e| e.to_string())?,
        );
        let deadline = Instant::now() + timeout;
        let mut ready = false;
        let mut last_issue = "owner did not observe WebView2 before deadline".to_string();
        while Instant::now() < deadline {
            if let Some(status) = owner.0.try_wait().map_err(|e| e.to_string())? {
                return Err(format!("owner exited early: {status}"));
            }
            if let Ok(bytes) = fs::read(&state)
                && let Ok(value) = serde_json::from_slice::<OwnerState>(&bytes)
            {
                summary = value.summary;
                if summary.message == "ready" {
                    if !summary.webview_outside_job.is_empty() {
                        return Err(format!(
                            "WebView2 PIDs outside Job: {:?}",
                            summary.webview_outside_job
                        ));
                    }
                    // The owner refreshes Job membership; retry if a newly
                    // started renderer is newer than the state file.
                    let processes = snapshot()?;
                    if !processes.contains_key(&summary.converter_pid) {
                        return Err("converter already exited before parent-kill".into());
                    }
                    if !summary.pre_cancel_job_pids.contains(&summary.converter_pid) {
                        return Err("converter not in Job before parent-kill".into());
                    }
                    let current = current_webviews(&summary, &processes, &value.ancestry);
                    if current.is_empty() {
                        last_issue = "no WebView2 process is alive before parent-kill".into();
                    } else if !current.is_subset(&summary.pre_cancel_job_pids) {
                        last_issue = format!(
                            "current WebView2 PIDs not yet confirmed in Job: {:?}",
                            current
                                .difference(&summary.pre_cancel_job_pids)
                                .collect::<Vec<_>>()
                        );
                    } else {
                        summary.webview_pids.extend(&current);
                        summary.pre_cancel_webview_pids = current;
                        ready = true;
                        break;
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        if !ready {
            return Err(last_issue);
        }
        if owner.0.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err("owner already exited before parent-kill".into());
        }
        let stopped_at = Instant::now();
        owner
            .0
            .kill()
            .map_err(|e| format!("TerminateProcess(owner): {e}"))?;
        owner.0.wait().map_err(|e| e.to_string())?;
        let gone = wait_gone(&mut summary, Duration::from_secs(20));
        summary.termination_ms = Some(stopped_at.elapsed().as_millis());
        gone?;
        summary.success = summary.pre_cancel_alive
            && !summary.pre_cancel_webview_pids.is_empty()
            && summary.webview_outside_job.is_empty()
            && summary.gone;
        Ok(())
    })();
    if let Err(e) = result {
        summary.message = e;
    } else {
        summary.message = "all checks passed".into();
    }
    summary
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let summary = match args.as_slice() {
        [mode, input, output_dir, timeout] if mode == "cancel" => run_owner(Path::new(input), Path::new(output_dir), None, Duration::from_secs(timeout.parse().unwrap_or(60))),
        [mode, input, output_dir, timeout] if mode == "parent-kill" => run_parent_kill(Path::new(input), Path::new(output_dir), Duration::from_secs(timeout.parse().unwrap_or(60))),
        [mode, input, output_dir, state, timeout] if mode == "owner" => run_owner(Path::new(input), Path::new(output_dir), Some(Path::new(state)), Duration::from_secs(timeout.parse().unwrap_or(60))),
        _ => Summary { message: "usage: epub-pdf-job-probe <cancel|parent-kill> <large.epub> <output-dir> <timeout-secs>".into(), ..Summary::default() },
    };
    if args.first().is_some_and(|mode| mode == "owner") {
        eprintln!("{}", serde_json::to_string(&summary).unwrap());
    } else {
        println!("{}", serde_json::to_string(&summary).unwrap());
    }
    std::process::exit(if summary.success { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_webview_descendants() {
        let processes = HashMap::from([
            (
                10,
                Process {
                    parent: 1,
                    name: "mimageviewer-epub-pdf.exe".into(),
                },
            ),
            (
                11,
                Process {
                    parent: 10,
                    name: "msedgewebview2.exe".into(),
                },
            ),
            (
                12,
                Process {
                    parent: 11,
                    name: "msedgewebview2.exe".into(),
                },
            ),
            (
                13,
                Process {
                    parent: 2,
                    name: "msedgewebview2.exe".into(),
                },
            ),
        ]);
        assert_eq!(
            descendants_of(10, &processes, &HashMap::new()),
            BTreeSet::from([11, 12])
        );
    }

    #[test]
    fn quotes_windows_arguments() {
        assert_eq!(quote(r#"C:\a b\"#), r#""C:\a b\\""#);
        assert_eq!(quote(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn cancellation_requires_live_job_members() {
        let mut summary = Summary {
            converter_pid: 10,
            ..Summary::default()
        };
        let converter = Process {
            parent: 1,
            name: "mimageviewer-epub-pdf.exe".into(),
        };
        let webview = Process {
            parent: 10,
            name: "msedgewebview2.exe".into(),
        };
        let live = HashMap::from([(10, converter), (11, webview)]);
        let ancestry = HashMap::new();
        assert!(
            verify_live_members(&summary, &live, &BTreeSet::from([10, 11]), &ancestry).is_err()
        );
        summary.webview_pids.insert(11);
        assert!(
            verify_live_members(
                &summary,
                &HashMap::new(),
                &BTreeSet::from([10, 11]),
                &ancestry
            )
            .unwrap_err()
            .contains("converter already exited")
        );
        assert!(
            verify_live_members(&summary, &live, &BTreeSet::from([10]), &ancestry)
                .unwrap_err()
                .contains("not in Job")
        );
        assert_eq!(
            verify_live_members(&summary, &live, &BTreeSet::from([10, 11]), &ancestry).unwrap(),
            BTreeSet::from([11])
        );
        summary.webview_outside_job.insert(12);
        assert!(
            verify_live_members(&summary, &live, &BTreeSet::from([10, 11]), &ancestry)
                .unwrap_err()
                .contains("outside Job")
        );
    }

    #[test]
    fn short_lived_webview_does_not_block_cancel() {
        let summary = Summary {
            converter_pid: 10,
            webview_pids: BTreeSet::from([11, 12]),
            pre_cancel_webview_pids: BTreeSet::from([12]),
            ..Summary::default()
        };
        let live = HashMap::from([
            (
                10,
                Process {
                    parent: 1,
                    name: "converter.exe".into(),
                },
            ),
            (
                12,
                Process {
                    parent: 10,
                    name: "msedgewebview2.exe".into(),
                },
            ),
        ]);
        assert_eq!(
            verify_live_members(&summary, &live, &BTreeSet::from([10, 12]), &HashMap::new())
                .unwrap(),
            BTreeSet::from([12])
        );
        assert!(
            verify_live_members(
                &summary,
                &HashMap::from([(10, live[&10].clone())]),
                &BTreeSet::from([10]),
                &HashMap::new()
            )
            .unwrap_err()
            .contains("no WebView2 process is alive")
        );
        assert!(!cancel_targets_gone(&summary, &live));
        assert!(cancel_targets_gone(&summary, &HashMap::new()));
    }

    #[test]
    fn fresh_snapshot_finds_child_after_intermediate_parent_exits() {
        let summary = Summary {
            converter_pid: 10,
            ..Summary::default()
        };
        let live = HashMap::from([
            (
                10,
                Process {
                    parent: 1,
                    name: "converter.exe".into(),
                },
            ),
            (
                12,
                Process {
                    parent: 11,
                    name: "msedgewebview2.exe".into(),
                },
            ),
        ]);
        assert_eq!(
            current_webviews(&summary, &live, &HashMap::from([(11, 10)])),
            BTreeSet::from([12])
        );
    }
}
