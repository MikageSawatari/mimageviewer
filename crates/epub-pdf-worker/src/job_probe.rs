//! S1 Job Object acceptance probe. This binary is not distributed.
#![cfg(windows)]

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    mem::{size_of, zeroed},
    os::windows::process::CommandExt,
    path::Path,
    process::Command,
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
    webview_pids: BTreeSet<u32>,
    webview_outside_job: BTreeSet<u32>,
    gone: bool,
    success: bool,
    message: String,
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
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
    let processes = snapshot()?;
    ancestry.extend(processes.iter().map(|(&pid, p)| (pid, p.parent)));
    let webviews = descendants_of(summary.converter_pid, &processes, ancestry);
    let mut members = job_members(job)?;
    // A child can start between the snapshot and the first Job query.
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
        if !live.contains_key(&summary.converter_pid)
            && summary
                .webview_pids
                .iter()
                .all(|pid| !live.contains_key(pid))
        {
            summary.gone = true;
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("converter or WebView2 processes survived Job close".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
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
        if let Some(path) = state {
            summary.message = "ready".into();
            fs::write(
                path,
                serde_json::to_vec(&summary).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        drop(job);
        wait_gone(&mut summary, Duration::from_secs(20))?;
        summary.success = !summary.webview_pids.is_empty()
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
        let mut owner = Command::new(exe)
            .arg("owner")
            .arg(input)
            .arg(output_dir)
            .arg(&state)
            .arg(timeout.as_secs().to_string())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let mut ready = false;
        while Instant::now() < deadline {
            if let Ok(bytes) = fs::read(&state)
                && let Ok(value) = serde_json::from_slice::<Summary>(&bytes)
            {
                summary = value;
                if summary.message == "ready" {
                    ready = true;
                    break;
                }
            }
            if let Some(status) = owner.try_wait().map_err(|e| e.to_string())? {
                return Err(format!("owner exited early: {status}"));
            }
            thread::sleep(Duration::from_millis(100));
        }
        // Snapshot independently while the owner is alive, before terminating it.
        if ready {
            let processes = snapshot()?;
            summary.webview_pids.extend(descendants_of(
                summary.converter_pid,
                &processes,
                &HashMap::new(),
            ));
        }
        owner
            .kill()
            .map_err(|e| format!("TerminateProcess(owner): {e}"))?;
        owner.wait().map_err(|e| e.to_string())?;
        if !ready {
            return Err("owner did not observe WebView2 before deadline".into());
        }
        wait_gone(&mut summary, Duration::from_secs(20))?;
        summary.success = !summary.webview_pids.is_empty()
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
    println!("{}", serde_json::to_string(&summary).unwrap());
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
}
