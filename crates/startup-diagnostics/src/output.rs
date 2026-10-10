use super::{Event, Lane, Outcome, Owner, Role, Stage, event_span, owner, record_detail};
use fs4::fs_std::FileExt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE: u64 = 2 * 1024 * 1024;
// Event text/name are fixed-size. Four KiB covers their escaped JSON and the
// schema fields, leaving the last terminal row writable at the file cap.
const TERMINAL_RESERVE: u64 = 4096;
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
            // run owns the file and retention lease. Their drops precede ACK.
            let success = match run(&o) {
                Ok(success) => success,
                Err(_) => {
                    o.fail();
                    false
                }
            };
            o.complete_writer(success);
        })
        .map(|_| ())
}
fn run(o: &Owner) -> io::Result<bool> {
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
    let (mut file, _lease) = open_log(&path)?;
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
fn open_lease(path: &Path) -> io::Result<std::fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))
}
fn open_log(path: &Path) -> io::Result<(std::fs::File, std::fs::File)> {
    // The lease protects retention, not concurrent writes: create_new owns the
    // unique run/role/pid log. Acquire it before publishing the log pathname.
    // Never byte-lock JSONL: Windows shared locks forbid even the owner's writes,
    // and exclusive locks would prevent collecting an active/hung run's evidence.
    let lease = open_lease(path)?;
    if !FileExt::try_lock_exclusive(&lease)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "diagnostic log lease busy",
        ));
    }
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    Ok((file, lease))
}
fn write_line(
    sink: &mut impl Write,
    value: &serde_json::Value,
    bytes: &mut u64,
) -> io::Result<bool> {
    write_line_until(sink, value, bytes, MAX_FILE)
}
fn write_line_until(
    sink: &mut impl Write,
    value: &serde_json::Value,
    bytes: &mut u64,
    limit: u64,
) -> io::Result<bool> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    if *bytes + line.len() as u64 > limit {
        return Ok(false);
    }
    sink.write_all(&line)?;
    *bytes += line.len() as u64;
    Ok(true)
}
fn pump(o: &Owner, sink: &mut impl Write, bytes: &mut u64) -> io::Result<bool> {
    let mut dropped_written = 0;
    let mut complete = true;
    loop {
        // A fixed stack batch; the journal lock is gone before serializing or
        // invoking any sink (including a deliberately blocked test sink).
        let mut batch = [None; 64];
        let drained;
        {
            // This worker may wait for the short journal critical section,
            // while publishers retain their single nonblocking attempt.
            let mut j = o.journal.lock().unwrap();
            for place in &mut batch {
                *place = j.pending.pop_front();
                if place.is_none() {
                    break;
                }
            }
            drained = j.pending.is_empty() && o.stopped();
        }
        let full = batch[63].is_some();
        let mut wrote = false;
        for e in batch.into_iter().flatten() {
            if !write_line_until(sink, &event_json(&e), bytes, MAX_FILE - TERMINAL_RESERVE)? {
                o.dropped.fetch_add(1, Ordering::Relaxed);
                complete = false;
                continue;
            }
            wrote = true;
        }
        let dropped = o.dropped.load(Ordering::Relaxed);
        if dropped != dropped_written {
            if !write_line_until(
                sink,
                &serde_json::json!({"kind":"dropped","count":dropped}),
                bytes,
                MAX_FILE - TERMINAL_RESERVE,
            )? {
                complete = false;
            } else {
                wrote = true;
            }
            dropped_written = dropped;
        }
        if wrote {
            sink.flush()?;
        }
        if drained {
            if let Some(terminal) = o.terminal.get() {
                complete &= write_line(sink, &event_json(terminal), bytes)?;
            }
            sink.flush()?;
            return Ok(complete);
        }
        if !full && !o.stopped() {
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
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
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
        // New writers lease the sidecar; old v4.4.0 writers shared-lock JSONL.
        // Keep both nonblocking locks until removal, including the sidecar.
        let lease = open_lease(&path)?;
        if !matches!(FileExt::try_lock_exclusive(&lease), Ok(true)) {
            continue;
        }
        let file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !matches!(FileExt::try_lock_exclusive(&file), Ok(true)) {
            continue;
        }
        // Windows permits removing the pathname while our file is open with
        // std's default share-delete semantics; retaining the lock closes races.
        std::fs::remove_file(&path)?;
        std::fs::remove_file(path.with_extension("lock"))?;
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
    fn assert_real_writer_records(role: Role) {
        let directory = tempfile::tempdir().unwrap();
        let mut o = crate::tests::test_owner(role);
        o.sink = Some(directory.path().to_path_buf());
        let lane = match role {
            Role::Launcher => Lane::Launcher,
            Role::Core => Lane::Core,
        };
        o.publish(Event::new(0, lane, Stage::Entry, 1, 0));
        o.publish(Event::new(1, lane, Stage::Entry, 1, 0));
        o.shutdown
            .store(super::super::ProcessState::Stopped as u8, Ordering::Release);
        run(&o).unwrap();
        let path = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.is_empty());
        assert!(text.ends_with('\n'));
        let records = text
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["kind"], "header");
        assert_eq!(records[0]["role"], format!("{role:?}").to_lowercase());
        assert_eq!(records[1]["kind"], "begin");
        assert_eq!(records[2]["kind"], "end");
    }

    #[test]
    fn real_launcher_writer_records_parseable_timeline() {
        assert_real_writer_records(Role::Launcher);
    }

    #[test]
    fn real_core_writer_records_parseable_timeline() {
        assert_real_writer_records(Role::Core);
    }

    #[test]
    fn terminal_drains_more_than_one_batch_and_is_last_exactly_once() {
        let o = crate::tests::test_owner(Role::Core);
        for id in 0..192 {
            o.publish(Event::new(2, Lane::Core, Stage::Entry, id, id));
        }
        super::super::terminal_for(&o, Outcome::Error);
        super::super::terminal_for(&o, Outcome::Ok);
        let mut sink = Vec::new();
        assert!(pump(&o, &mut sink, &mut 0).unwrap());
        let text = String::from_utf8(sink).unwrap();
        let rows = text
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 193);
        for (id, row) in rows[..192].iter().enumerate() {
            assert_eq!(row["span"], id as u64);
        }
        assert_eq!(rows[192]["name"], "process.terminal");
        assert_eq!(rows[192]["outcome"], "Error");
    }

    #[test]
    fn file_cap_reserves_terminal_but_reports_incomplete_delivery() {
        let o = crate::tests::test_owner(Role::Core);
        for id in 0..crate::CAPACITY {
            o.publish(Event::new(2, Lane::Core, Stage::Entry, id as u64, 0));
        }
        // Publication succeeds even while terminal's ordinary journal is locked.
        let j = o.journal.lock().unwrap();
        super::super::terminal_for(&o, Outcome::Ok);
        drop(j);
        let mut bytes = MAX_FILE - TERMINAL_RESERVE;
        let mut sink = Vec::new();
        assert!(!pump(&o, &mut sink, &mut bytes).unwrap());
        assert!(bytes <= MAX_FILE);
        let text = String::from_utf8(sink).unwrap();
        let rows = text.lines().collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        let terminal: serde_json::Value = serde_json::from_str(rows[0]).unwrap();
        assert_eq!(terminal["name"], "process.terminal");
        assert_eq!(o.dropped.load(Ordering::Relaxed), crate::CAPACITY as u64);
    }

    #[test]
    fn real_writer_ack_follows_terminal_flush_and_lease_close() {
        let directory = tempfile::tempdir().unwrap();
        let mut o = crate::tests::test_owner(Role::Core);
        o.sink = Some(directory.path().to_path_buf());
        let o = Arc::new(o);
        for id in 0..192 {
            o.publish(Event::new(2, Lane::Core, Stage::Entry, id, 0));
        }
        super::super::terminal_for(&o, Outcome::Ok);
        spawn(o.clone()).unwrap();
        assert!(o.wait_writer_until(std::time::Instant::now() + Duration::from_secs(2)));
        let path =
            directory
                .path()
                .join(format!("{:016x}-core-{}.jsonl", o.run, std::process::id()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 194);
        let last: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last["name"], "process.terminal");
        let lease = open_lease(&path).unwrap();
        assert!(FileExt::try_lock_exclusive(&lease).unwrap());
    }

    #[test]
    fn failed_real_writer_signals_unsuccessful_completion() {
        let directory = tempfile::tempdir().unwrap();
        let file_path = directory.path().join("not-a-directory");
        std::fs::write(&file_path, b"untouched").unwrap();
        let mut o = crate::tests::test_owner(Role::Core);
        o.sink = Some(file_path.clone());
        let o = Arc::new(o);
        super::super::terminal_for(&o, Outcome::Ok);
        spawn(o.clone()).unwrap();
        assert!(!o.wait_writer_until(std::time::Instant::now() + Duration::from_secs(2)));
        assert_eq!(*o.writer_completion.0.lock().unwrap(), Some(false));
        assert!(o.take_notice());
        assert_eq!(std::fs::read(file_path).unwrap(), b"untouched");
    }

    #[test]
    fn completion_waits_for_final_flush_and_reports_flush_failure() {
        use std::sync::mpsc;
        struct FlushBlocked {
            flushes: usize,
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Write for FlushBlocked {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                if self.flushes == 2 {
                    self.entered.send(()).unwrap();
                    self.release.recv().unwrap();
                }
                Ok(())
            }
        }
        let o = Arc::new(crate::tests::test_owner(Role::Core));
        o.publish(Event::new(2, Lane::Core, Stage::Entry, 1, 0));
        super::super::terminal_for(&o, Outcome::Ok);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = o.clone();
        let thread = std::thread::spawn(move || {
            let mut sink = FlushBlocked {
                flushes: 0,
                entered: entered_tx,
                release: release_rx,
            };
            let result = pump(&writer, &mut sink, &mut 0);
            drop(sink);
            writer.complete_writer(result.as_ref().is_ok_and(|success| *success));
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!o.wait_writer_until(std::time::Instant::now() + Duration::from_millis(20)));
        assert!(o.writer_completion.0.lock().unwrap().is_none());
        release_tx.send(()).unwrap();
        assert!(o.wait_writer_until(std::time::Instant::now() + Duration::from_secs(2)));
        thread.join().unwrap();
        struct FailedFlush;
        impl Write for FailedFlush {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("injected flush failure"))
            }
        }
        assert!(pump(&o, &mut FailedFlush, &mut 0).is_err());
    }

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
            release: Option<mpsc::Receiver<()>>,
        }
        impl Write for Blocked {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                if let Some(release) = self.release.take() {
                    self.entered.send(()).unwrap();
                    release.recv().unwrap();
                }
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
            let _ = writer.writer_wake.set(std::thread::current());
            let mut sink = Blocked {
                entered: entered_tx,
                release: Some(release_rx),
            };
            let result = pump(&writer, &mut sink, &mut 0);
            writer.complete_writer(result.as_ref().is_ok_and(|success| *success));
            result
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
        super::super::terminal_for(&o, Outcome::Ok);
        assert!(!o.wait_writer_until(std::time::Instant::now() + Duration::from_millis(20)));
        assert!(o.writer_completion.0.lock().unwrap().is_none());
        release_tx.send(()).unwrap();
        thread.join().unwrap().unwrap();
        assert!(o.wait_writer_until(std::time::Instant::now() + Duration::from_secs(2)));
        struct Failed;
        impl Write for Failed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("injected"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let o = crate::tests::test_owner(Role::Core);
        o.publish(Event::new(0, Lane::Core, Stage::SettingsOpen, 2, 0));
        assert!(pump(&o, &mut Failed, &mut 0).is_err());
        o.publish(Event::new(1, Lane::Core, Stage::SettingsOpen, 2, 1));
        assert!(o.journal.try_lock().is_ok());
    }
    #[test]
    fn retention_preserves_legacy_shared_lock_and_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut active = None;
        for run in 0..12u64 {
            let path = directory.path().join(format!("{run:016x}-core-1.jsonl"));
            let file = std::fs::File::create(path).unwrap();
            file.set_modified(UNIX_EPOCH + Duration::from_secs(run + 1))
                .unwrap();
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
        retain(directory.path(), 11).unwrap();
        assert!(
            !directory
                .path()
                .join("0000000000000000-core-1.jsonl")
                .exists()
        );
        assert!(
            !directory
                .path()
                .join("0000000000000000-core-1.lock")
                .exists()
        );
    }

    #[test]
    fn retention_leases_preserve_readable_writable_launcher_and_core_logs() {
        for role in ["launcher", "core"] {
            let directory = tempfile::tempdir().unwrap();
            let active_path = directory.path().join(format!("{0:016x}-{role}-1.jsonl", 0));
            let (mut active, lease) = open_log(&active_path).unwrap();
            let mut bytes = 0;
            let header = serde_json::json!({"kind":"header","role":role});
            write_line(&mut active, &header, &mut bytes).unwrap();
            active.flush().unwrap();
            active.set_modified(UNIX_EPOCH).unwrap();
            // Another process can collect evidence while the writer is alive.
            let read_header = std::fs::read_to_string(&active_path).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&read_header).unwrap(),
                header
            );
            // Same-shaped launcher/core paths belonging to one run count once.
            for run in 1..12u64 {
                for other_role in ["launcher", "core"] {
                    let path = directory
                        .path()
                        .join(format!("{run:016x}-{other_role}-1.jsonl"));
                    let (mut file, _lease) = open_log(&path).unwrap();
                    file.write_all(b"{\"kind\":\"header\"}\n").unwrap();
                    file.set_modified(UNIX_EPOCH + Duration::from_secs(run))
                        .unwrap();
                }
            }
            let unrelated = directory.path().join("user.lock");
            std::fs::write(&unrelated, b"user data").unwrap();
            retain(directory.path(), 11).unwrap();
            assert!(active_path.exists());
            assert!(active_path.with_extension("lock").exists());
            for other_role in ["launcher", "core"] {
                let old = directory
                    .path()
                    .join(format!("{0:016x}-{other_role}-1.jsonl", 1));
                assert!(!old.exists());
                assert!(!old.with_extension("lock").exists());
                for run in 2..12u64 {
                    assert!(
                        directory
                            .path()
                            .join(format!("{run:016x}-{other_role}-1.jsonl"))
                            .exists()
                    );
                }
            }
            // Retention did not lock the log body or stop subsequent events.
            write_line(&mut active, &serde_json::json!({"kind":"end"}), &mut bytes).unwrap();
            active.flush().unwrap();
            let records = std::fs::read_to_string(&active_path).unwrap();
            assert_eq!(records.lines().count(), 2);
            for line in records.lines() {
                serde_json::from_str::<serde_json::Value>(line).unwrap();
            }
            active.set_modified(UNIX_EPOCH).unwrap();
            drop(active);
            drop(lease);
            retain(directory.path(), 11).unwrap();
            assert!(!active_path.exists());
            assert!(!active_path.with_extension("lock").exists());
            assert_eq!(std::fs::read(unrelated).unwrap(), b"user data");
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 41);
        }
    }

    #[test]
    fn log_ownership_cannot_overwrite_existing_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("0000000000000001-core-1.jsonl");
        let (mut file, lease) = open_log(&path).unwrap();
        file.write_all(b"{\"kind\":\"header\"}\n").unwrap();
        assert_eq!(
            open_log(&path).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(file);
        drop(lease);
        assert_eq!(
            open_log(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"kind\":\"header\"}\n");
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
