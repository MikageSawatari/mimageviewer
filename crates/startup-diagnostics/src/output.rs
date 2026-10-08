use super::{Event, Lane, Outcome, Owner, Role, Stage, event_span, owner, record_detail};
use fs4::fs_std::FileExt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE: u64 = 2 * 1024 * 1024;
pub(crate) fn select_sink(args: &[String], portable: bool) -> Option<PathBuf> {
    if let Some(path) = explicit_data_dir(args) {
        return Some(path.join("logs/startup"));
    }
    if portable {
        return std::env::current_exe()
            .ok()?
            .parent()
            .map(|p| p.join("data/logs/startup"));
    }
    std::env::var_os("LOCALAPPDATA")
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(p).join("mimageviewer/startup-logs"))
}
fn explicit_data_dir(args: &[String]) -> Option<PathBuf> {
    // Keep exactly the product parser's first two-argument match, including
    // empty values and option-looking values. No diagnostics-only CLI aliases.
    args.windows(2)
        .find(|w| w[0] == "--data-dir")
        .map(|w| PathBuf::from(&w[1]))
}
pub(crate) fn spawn(o: Arc<Owner>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("startup-log-writer".into())
        .spawn(move || {
            let _ = o.writer_wake.set(std::thread::current());
            if run(&o).is_err() {
                o.fail();
            }
        })
        .map(|_| ())
}
fn run(o: &Owner) -> io::Result<()> {
    let directory = o.sink.as_ref().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "diagnostic directory unavailable")
    })?;
    std::fs::create_dir_all(directory)?;
    let role = match o.role {
        Role::Launcher => "launcher",
        Role::Core => "core",
    };
    let path = directory.join(format!(
        "{:016x}-{role}-{}.jsonl",
        o.run,
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    if !FileExt::try_lock_shared(&file)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "new diagnostic log lock busy",
        ));
    }
    let header = serde_json::json!({
        "schema":1,"kind":"header","run":format!("{:016x}",o.run),"role":role,
        "pid":std::process::id(),"version":o.version,"qpc_origin":o.origin,"qpc_frequency":o.frequency,
        "wall_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
        "args":o.args,"log_path":path,"APPDATA":std::env::var_os("APPDATA").map(|v|v.to_string_lossy().into_owned()),
        "LOCALAPPDATA":std::env::var_os("LOCALAPPDATA").map(|v|v.to_string_lossy().into_owned())
    });
    let mut bytes = 0;
    write_line(&mut file, &header, &mut bytes)?;
    file.flush()?;
    // The new evidence is durable enough for a flush before best-effort pruning.
    if let Err(error) = retain(directory, o.run) {
        write_line(
            &mut file,
            &serde_json::json!({"kind":"retention_error","error":error.to_string()}),
            &mut bytes,
        )?;
        file.flush()?;
    }
    pump(o, &mut file, &mut bytes)
}
fn write_line(
    sink: &mut impl Write,
    value: &serde_json::Value,
    bytes: &mut u64,
) -> io::Result<bool> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    if *bytes + line.len() as u64 > MAX_FILE {
        return Ok(false);
    }
    sink.write_all(&line)?;
    *bytes += line.len() as u64;
    Ok(true)
}
fn pump(o: &Owner, sink: &mut impl Write, bytes: &mut u64) -> io::Result<()> {
    let mut dropped_written = 0;
    loop {
        // A fixed stack batch; the journal lock is gone before serializing or
        // invoking any sink (including a deliberately blocked test sink).
        let mut batch = [None; 64];
        if let Ok(mut j) = o.journal.try_lock() {
            for place in &mut batch {
                *place = j.pending.pop_front();
                if place.is_none() {
                    break;
                }
            }
        }
        let full = batch[63].is_some();
        let mut wrote = false;
        for e in batch.into_iter().flatten() {
            if !write_line(sink, &event_json(&e), bytes)? {
                sink.flush()?;
                return Ok(());
            }
            wrote = true;
        }
        let dropped = o.dropped.load(Ordering::Relaxed);
        if dropped != dropped_written {
            if !write_line(
                sink,
                &serde_json::json!({"kind":"dropped","count":dropped}),
                bytes,
            )? {
                return Ok(());
            }
            dropped_written = dropped;
            wrote = true;
        }
        if wrote {
            sink.flush()?;
        }
        if o.stopped() {
            return Ok(());
        }
        if !full {
            std::thread::park();
        }
    }
}
pub(crate) fn event_json(e: &Event) -> serde_json::Value {
    let kind = match e.kind & 0x7f {
        0 => "begin",
        1 => "end",
        2 => "milestone",
        3 => "detail",
        4 => "overdue",
        _ => "unknown",
    };
    serde_json::json!({"kind":kind,"inherited":e.kind&0x80!=0,"lane":format!("{:?}",e.lane),"pid":e.source_pid,"tid":e.source_tid,"role":format!("{:?}",e.source_role),"qpc_ticks":e.qpc_ticks,
        "stage":format!("{:?}",e.stage),"span":e.id,"parent":e.parent,"at_us":e.at,
        "elapsed_us":e.elapsed,"correlation":e.correlation,"outcome":format!("{:?}",e.outcome),"name":e.name(),"detail":e.message()})
}
fn log_run(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(".jsonl")?;
    let mut p = stem.split('-');
    let run = p.next()?;
    if run.len() != 16 || !run.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if !matches!(p.next()?, "launcher" | "core") {
        return None;
    }
    let pid = p.next()?;
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) || p.next().is_some() {
        return None;
    }
    u64::from_str_radix(run, 16).ok()
}
fn retain(directory: &Path, current_run: u64) -> io::Result<()> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if !kind.is_file() || kind.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(run) = log_run(&name.to_string_lossy()) else {
            continue;
        };
        let metadata = entry.metadata()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                continue;
            }
        }
        files.push((run, metadata.modified().unwrap_or(UNIX_EPOCH), entry.path()));
    }
    let mut runs = std::collections::BTreeMap::new();
    for (run, time, _) in &files {
        runs.entry(*run)
            .and_modify(|old| {
                if *old < *time {
                    *old = *time;
                }
            })
            .or_insert(*time);
    }
    let mut runs = runs.into_iter().collect::<Vec<_>>();
    runs.sort_by_key(|(_, time)| std::cmp::Reverse(*time));
    let mut keep = runs
        .into_iter()
        .take(10)
        .map(|(run, _)| run)
        .collect::<Vec<_>>();
    if !keep.contains(&current_run) {
        if keep.len() == 10 {
            keep.pop();
        }
        keep.push(current_run);
    }
    for (run, _, path) in files {
        if keep.contains(&run) {
            continue;
        }
        // Cooperative writers hold a shared lock. Never wait for active logs.
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        if !matches!(FileExt::try_lock_exclusive(&file), Ok(true)) {
            continue;
        }
        // Windows permits removing the pathname while our file is open with
        // std's default share-delete semantics; retaining the lock closes races.
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Pure string check, including extended-length UNC. No filesystem probing.
pub fn is_unc(path: &str) -> bool {
    let normalized = path.replace('/', "\\");
    normalized
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("\\\\?\\UNC\\"))
        || (normalized.starts_with("\\\\")
            && !normalized.starts_with("\\\\?\\")
            && !normalized.starts_with("\\\\.\\"))
}
pub(crate) fn environment(path: PathBuf, selection: String, runtime: Option<PathBuf>) {
    if owner().is_none() {
        return;
    }
    if std::thread::Builder::new().name("startup-environment".into()).spawn(move|| {
        let query=event_span(Lane::Metadata,Stage::EnvironmentQuery,0);
        record_path("data_dir",&path);record_detail("data_dir.selection",&selection);
        if let Some(runtime)=runtime {record_path("runtime.path",&runtime);}
        let unc=is_unc(&path.to_string_lossy());
        let reparse=ancestor_reparse(&path, path_is_reparse);
        let network=network_with_reparse(unc,drive_network(&path),reparse.as_ref().ok().copied());
        let redirected=if matches!(reparse,Ok(true)) {"yes"} else {"unknown"};
        record_detail("data_dir.environment",&format!("unc={unc};network={network};redirected={redirected};evidence=drive-type/ancestor-reparse;reparse-target=unqueried;unqueried-shell-location=unknown"));
        query.finish(Outcome::Ok);
    }).is_err() {if let Some(o)=owner() {o.fail();}}
}
fn record_path(name: &'static str, path: &Path) {
    let text = path.to_string_lossy();
    if text.len() <= 256 {
        record_detail(name, &text);
        return;
    }
    // Preserve long actual paths as ordered bounded records. This work is on
    // the environment worker, never under the publisher's lock.
    let mut start = 0;
    let mut index = 0;
    while start < text.len() && index < 160 {
        let mut end = (start + 224).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        record_detail(name, &format!("chunk={index};{}", &text[start..end]));
        start = end;
        index += 1;
    }
    if start < text.len() {
        record_detail(name, "path.truncated");
    }
}
#[cfg(windows)]
fn drive_network(path: &Path) -> &'static str {
    use std::os::windows::ffi::OsStrExt;
    let text = path.to_string_lossy();
    let text = text.strip_prefix("\\\\?\\").unwrap_or(&text);
    let b = text.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || !matches!(b[2], b'\\' | b'/') {
        return "unknown";
    }
    let root = PathBuf::from(format!("{}:\\", b[0] as char));
    let wide = root
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    network_for_drive_type(unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr())
    })
}
#[cfg(any(windows, test))]
fn network_for_drive_type(drive_type: u32) -> &'static str {
    match drive_type {
        4 => "yes",
        2 | 3 | 5 | 6 => "no",
        _ => "unknown",
    }
}
#[cfg(not(windows))]
fn drive_network(_path: &Path) -> &'static str {
    "unknown"
}
fn network_with_reparse(unc: bool, drive: &'static str, reparse: Option<bool>) -> &'static str {
    if unc || drive == "yes" {
        "yes"
    } else if drive == "no" && reparse == Some(false) {
        "no"
    } else {
        // A local drive letter does not prove a link's destination is local.
        // We deliberately do not resolve targets; failed ancestry probes are unknown too.
        "unknown"
    }
}
fn ancestor_reparse(
    path: &Path,
    mut probe: impl FnMut(&Path) -> std::io::Result<bool>,
) -> std::io::Result<bool> {
    for ancestor in path.ancestors() {
        if probe(ancestor)? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn path_is_reparse(path: &Path) -> std::io::Result<bool> {
    let metadata = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(metadata.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(metadata.file_type().is_symlink())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn local_drive_requires_confirmed_non_reparse_ancestry() {
        assert_eq!(network_with_reparse(false, "no", Some(false)), "no");
        assert_eq!(network_with_reparse(false, "no", Some(true)), "unknown");
        assert_eq!(network_with_reparse(false, "no", None), "unknown");
        assert_eq!(
            network_with_reparse(false, "unknown", Some(false)),
            "unknown"
        );
        assert_eq!(network_with_reparse(false, "yes", Some(true)), "yes");
        assert_eq!(network_with_reparse(true, "unknown", None), "yes");
    }
    #[cfg(windows)]
    #[test]
    fn local_link_data_with_unc_target_is_unknown_without_target_confirmation() {
        let path = Path::new(r"C:\link\data");
        let mut visited = Vec::new();
        // Synthetic junction C:\link -> \\server\share. No privilege, real link
        // creation, or network access is needed; targets are intentionally unqueried.
        let reparse = ancestor_reparse(path, |ancestor| {
            visited.push(ancestor.to_path_buf());
            Ok(ancestor == Path::new(r"C:\link"))
        })
        .unwrap();
        assert_eq!(visited, [path.to_path_buf(), PathBuf::from(r"C:\link")]);
        assert_eq!(
            network_with_reparse(false, network_for_drive_type(3), Some(reparse)),
            "unknown"
        );
        let failed = ancestor_reparse(path, |_| Err(std::io::ErrorKind::PermissionDenied.into()));
        assert_eq!(network_with_reparse(false, "no", failed.ok()), "unknown");
    }
    #[test]
    fn unc_forms_and_explicit_sink_do_not_probe_filesystem() {
        assert!(is_unc("\\\\server\\share"));
        assert!(is_unc("\\\\?\\UNC\\server\\share"));
        assert!(is_unc("//server/share"));
        assert!(!is_unc("\\\\?\\C:\\data"));
        assert!(!is_unc("C:\\data"));
        assert!(!is_unc("\\\\.\\pipe\\name"));
        assert_eq!(
            select_sink(
                &["app".into(), "--data-dir".into(), "disposable".into()],
                false
            ),
            Some(PathBuf::from("disposable/logs/startup"))
        );
        assert_eq!(drive_network(Path::new("relative")), "unknown");
        assert_eq!(network_for_drive_type(4), "yes"); // mapped remote drive
        assert_eq!(network_for_drive_type(3), "no");
        assert_eq!(network_for_drive_type(0), "unknown");
        assert_eq!(
            explicit_data_dir(&["app".into(), "--data-dir=disposable".into()]),
            None
        );
        assert_eq!(
            explicit_data_dir(&["app".into(), "--data-dir".into()]),
            None
        );
        assert_eq!(
            explicit_data_dir(&[
                "app".into(),
                "--data-dir".into(),
                "".into(),
                "--data-dir".into(),
                "second".into()
            ]),
            Some(PathBuf::new())
        );
        assert_eq!(
            select_sink(&["app".into(), "--data-dir".into(), "".into()], false),
            Some(PathBuf::from("logs/startup"))
        );
    }
    #[test]
    fn blocked_and_failed_writer_never_hold_publisher_lock() {
        use std::sync::mpsc;
        struct Blocked {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Write for Blocked {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let o = Arc::new(crate::tests::test_owner(Role::Core));
        o.publish(Event::new(0, Lane::Core, Stage::Settings, 1, 0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = o.clone();
        let thread = std::thread::spawn(move || {
            let mut sink = Blocked {
                entered: entered_tx,
                release: release_rx,
            };
            pump(&writer, &mut sink, &mut 0)
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Exercise the same span publisher used by the UI against the stopped
        // sink, rather than merely checking a lock in isolation.
        let (done_tx, done_rx) = mpsc::channel();
        let publisher = o.clone();
        std::thread::spawn(move || {
            let s = crate::new_span_for(Some(publisher), Lane::Core, Stage::AppCreate, 0, false);
            s.finish(Outcome::Ok);
            done_tx.send(()).unwrap();
        });
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        o.shutdown
            .store(super::super::ProcessState::Stopped as u8, Ordering::Release);
        release_tx.send(()).unwrap();
        thread.join().unwrap().unwrap();
        struct Failed;
        impl Write for Failed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("injected"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        o.publish(Event::new(0, Lane::Core, Stage::SettingsOpen, 2, 0));
        assert!(pump(&o, &mut Failed, &mut 0).is_err());
        o.publish(Event::new(1, Lane::Core, Stage::SettingsOpen, 2, 1));
        assert!(o.journal.try_lock().is_ok());
    }
    #[test]
    fn retention_preserves_active_writer_and_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut active = None;
        for run in 0..12u64 {
            let path = directory.path().join(format!("{run:016x}-core-1.jsonl"));
            let file = std::fs::File::create(path).unwrap();
            if run == 0 {
                FileExt::lock_shared(&file).unwrap();
                active = Some(file);
            }
        }
        let unrelated = directory.path().join("settings.db");
        std::fs::write(&unrelated, b"user data").unwrap();
        retain(directory.path(), 11).unwrap();
        assert!(
            directory
                .path()
                .join("0000000000000000-core-1.jsonl")
                .exists()
        );
        assert_eq!(std::fs::read(unrelated).unwrap(), b"user data");
        drop(active);
    }
    #[test]
    fn capped_sink_never_exceeds_two_mib() {
        let mut bytes = MAX_FILE - 2;
        let mut sink = Vec::new();
        assert!(
            !write_line(
                &mut sink,
                &serde_json::json!({"detail":"hello"}),
                &mut bytes
            )
            .unwrap()
        );
        assert!(sink.is_empty());
    }
}
