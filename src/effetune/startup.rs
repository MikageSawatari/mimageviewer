//! One App-owned migration operation; atomics only project its host permission
//! to existing background consumers. No filesystem work runs on the UI thread.
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc,
};

mod path_identity;
use super::migration::{MigrationFailure, MigrationOutcome};

pub(crate) const FAILURE_NOTICE: &str = "音響調整のデータを引き継げませんでした。この mImageViewer も含め、トレイに常駐しているものもすべて終了してから、1 つだけ起動してください";

pub(crate) fn failure_notice(reason: &MigrationFailure) -> String {
    match reason {
        MigrationFailure::ProcessesStillRunning(pids) => format!(
            "{FAILURE_NOTICE}。終了後も失敗する場合は、タスク マネージャーで残っている msedgewebview2.exe (PID: {}) を確認してください",
            super::migration::process_ids(pids)
        ),
        MigrationFailure::Detail(_) | MigrationFailure::QuiescenceDeadlineExceeded => {
            FAILURE_NOTICE.into()
        }
    }
}

pub(crate) struct HostStartupGate(AtomicU8);

impl HostStartupGate {
    pub(crate) fn ready() -> Arc<Self> {
        Arc::new(Self(AtomicU8::new(1)))
    }
    fn pending() -> Arc<Self> {
        Arc::new(Self(AtomicU8::new(0)))
    }
    pub(crate) fn user_vst_allowed(&self) -> bool {
        self.0.load(Ordering::Acquire) != 0
    }
    pub(crate) fn sound_allowed(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }
    fn complete(&self, passed: bool) {
        self.0.store(if passed { 1 } else { 2 }, Ordering::Release);
    }
}

enum Phase {
    NotStarted,
    Pending(mpsc::Receiver<Result<MigrationOutcome, MigrationFailure>>),
    Complete,
}

pub(crate) struct StartupMigration {
    gate: Arc<HostStartupGate>,
    phase: Phase,
}

impl Default for StartupMigration {
    fn default() -> Self {
        // Existing App fixtures must never read the real APPDATA migration
        // source. Dedicated tests explicitly construct the pending owner.
        if cfg!(test) {
            Self {
                gate: HostStartupGate::ready(),
                phase: Phase::Complete,
            }
        } else {
            Self::pending()
        }
    }
}

impl StartupMigration {
    pub(crate) fn pending() -> Self {
        Self {
            gate: HostStartupGate::pending(),
            phase: Phase::NotStarted,
        }
    }
    pub(crate) fn gate(&self) -> Arc<HostStartupGate> {
        Arc::clone(&self.gate)
    }
    pub(crate) fn waiting(&self) -> bool {
        !self.gate.user_vst_allowed()
    }

    #[cfg(test)]
    pub(crate) fn pending_for_test() -> (
        Self,
        mpsc::Sender<Result<MigrationOutcome, MigrationFailure>>,
    ) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                gate: HostStartupGate::pending(),
                phase: Phase::Pending(rx),
            },
            tx,
        )
    }

    pub(crate) fn start(&mut self, data_dir: PathBuf, wake: Option<egui::Context>) {
        if !matches!(self.phase, Phase::NotStarted) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.phase = Phase::Pending(rx);
        let spawn = std::thread::Builder::new()
            .name("miv-sound-migration".into())
            .spawn(move || {
                let result = migration_request(data_dir)
                    .map_err(MigrationFailure::from)
                    .and_then(|request| super::migration::migrate(&request));
                let _ = tx.send(result);
                if let Some(ctx) = wake {
                    ctx.request_repaint();
                }
            });
        if let Err(error) = spawn {
            // Feed spawn failure through the same one-shot terminal path.
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Err(format!("migration worker spawn: {error}").into()));
            self.phase = Phase::Pending(rx);
        }
    }

    pub(crate) fn poll(&mut self) -> Option<Result<MigrationOutcome, MigrationFailure>> {
        let Phase::Pending(rx) = &self.phase else {
            return None;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("migration worker disconnected".into()),
        };
        self.gate.complete(result.is_ok());
        self.phase = Phase::Complete;
        Some(result)
    }
}

fn migration_request(data_dir: PathBuf) -> Result<super::migration::MigrationRequest, String> {
    let destination = super::sound_data_root(&data_dir)?;
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| cfg!(feature = "portable").then(PathBuf::new))
        .ok_or("APPDATA is unavailable")?;
    #[cfg(feature = "test-script")]
    let source_root = override_source(&std::env::args_os().collect::<Vec<_>>())?
        .unwrap_or_else(|| appdata.clone());
    #[cfg(not(feature = "test-script"))]
    let source_root = appdata.clone();
    let mode = if cfg!(feature = "portable") {
        super::migration::MigrationMode::Portable
    } else if path_identity::same_location(&data_dir, &appdata.join("mimageviewer"))? {
        super::migration::MigrationMode::Normal
    } else {
        super::migration::MigrationMode::Alternate
    };
    Ok(super::migration::MigrationRequest {
        source_root,
        destination,
        mode,
    })
}

#[cfg(feature = "test-script")]
fn override_source(args: &[std::ffi::OsString]) -> Result<Option<PathBuf>, String> {
    let Some(index) = args
        .iter()
        .position(|arg| arg == "--miv-sound-migration-source")
    else {
        return Ok(None);
    };
    let source = args
        .get(index + 1)
        .filter(|v| !v.to_string_lossy().starts_with("--"))
        .ok_or("migration source requires a path")?;
    let data_index = args
        .iter()
        .position(|arg| arg == "--data-dir")
        .ok_or("migration source requires an explicit isolated --data-dir")?;
    let destination = args
        .get(data_index + 1)
        .filter(|v| !v.to_string_lossy().starts_with("--"))
        .ok_or("--data-dir requires a path")?;
    let source = std::path::absolute(source).map_err(|e| e.to_string())?;
    let destination = std::path::absolute(destination).map_err(|e| e.to_string())?;
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is unavailable")?;
    if path_identity::overlapping_locations(&source, &appdata)?
        || path_identity::overlapping_locations(&destination, &appdata.join("mimageviewer"))?
    {
        return Err(
            "migration test arguments require synthetic source and isolated data-dir".into(),
        );
    }
    Ok(Some(source))
}

#[cfg(feature = "test-script")]
fn probe_value<'a>(
    args: &'a [std::ffi::OsString],
    name: &str,
) -> Result<Option<&'a std::ffi::OsStr>, String> {
    let mut positions = args.iter().enumerate().filter(|(_, arg)| *arg == name);
    let Some((index, _)) = positions.next() else {
        return Ok(None);
    };
    if positions.next().is_some() {
        return Err(format!("duplicate migration probe argument: {name}"));
    }
    args.get(index + 1)
        .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
        .map(|value| Some(value.as_os_str()))
        .ok_or_else(|| format!("{name} requires a value"))
}

#[cfg(feature = "test-script")]
fn source_close_arguments(
    args: &[std::ffi::OsString],
    checkpoints: Option<&std::path::Path>,
    appdata: &std::path::Path,
) -> Result<Option<(u32, PathBuf)>, String> {
    let pid = probe_value(args, "--miv-sound-migration-original-browser")?;
    let close = probe_value(args, "--miv-sound-migration-close-file")?;
    let (pid, close) = match (pid, close) {
        (None, None) => return Ok(None),
        (Some(pid), Some(close)) => (pid, close),
        _ => return Err("original-browser and close-file must be supplied together".into()),
    };
    if !args.iter().any(|arg| arg == "--miv-sound-migrate-only") {
        return Err("source close probe requires --miv-sound-migrate-only".into());
    }
    let pid = pid
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .ok_or("original browser PID must be a positive integer")?;
    let checkpoints = checkpoints.ok_or("source close probe requires migration checkpoints")?;
    let run_root = checkpoints
        .parent()
        .ok_or("migration checkpoints require a probe run parent directory")?;
    let close = std::path::absolute(close).map_err(|error| error.to_string())?;
    let close_parent = close
        .parent()
        .ok_or("source close file requires a parent")?;
    if path_identity::overlapping_locations(run_root, appdata)?
        || !path_identity::same_location(close_parent, run_root)?
    {
        return Err(
            "source close file must be directly inside the isolated probe run directory".into(),
        );
    }
    // This diagnostic may create only one new stop file. Reject reparse points
    // throughout the existing parent chain, and never replace an existing file.
    for ancestor in checkpoints.ancestors().chain(close_parent.ancestors()) {
        let metadata = std::fs::symlink_metadata(ancestor)
            .map_err(|error| format!("inspect probe directory {}: {error}", ancestor.display()))?;
        use std::os::windows::fs::MetadataExt;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err(format!(
                "probe path is not a plain directory: {}",
                ancestor.display()
            ));
        }
    }
    match std::fs::symlink_metadata(&close) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("source close file must not already exist".into()),
        Err(error) => return Err(format!("inspect source close file: {error}")),
    }
    Ok(Some((pid, close)))
}

#[cfg(feature = "test-script")]
fn flush_probe_evidence(rows: &mut Vec<serde_json::Value>) -> Result<(), String> {
    for row in rows.drain(..) {
        println!("{row}");
    }
    use std::io::Write;
    std::io::stdout()
        .flush()
        .map_err(|error| format!("flush migration evidence: {error}"))
}

/// Early, feature-gated diagnostic route. The migrate-only path never creates
/// an App, reads settings, starts a bridge, or initializes WebView2.
pub(crate) fn handle_test_arguments() -> Result<Option<i32>, String> {
    let args = std::env::args_os().collect::<Vec<_>>();
    let requested = args.iter().any(|arg| {
        arg == "--miv-sound-migrate-only"
            || arg == "--miv-sound-migration-source"
            || arg == "--miv-sound-migration-checkpoints"
            || arg == "--miv-sound-migration-original-browser"
            || arg == "--miv-sound-migration-close-file"
    });
    if !requested {
        return Ok(None);
    }
    #[cfg(not(feature = "test-script"))]
    {
        Err("mIV Sound migration test arguments require the test-script feature".into())
    }
    #[cfg(feature = "test-script")]
    {
        let source = override_source(&args)?;
        let checkpoints = probe_value(&args, "--miv-sound-migration-checkpoints")?
            .map(std::path::absolute)
            .transpose()
            .map_err(|error| error.to_string())?;
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or("APPDATA is unavailable")?;
        let close_arguments = source_close_arguments(&args, checkpoints.as_deref(), &appdata)?;
        if !args.iter().any(|arg| arg == "--miv-sound-migrate-only") {
            if args
                .iter()
                .any(|arg| arg == "--miv-sound-migration-checkpoints")
            {
                return Err("migration checkpoints require --miv-sound-migrate-only".into());
            }
            return Ok(None);
        }
        let source =
            source.ok_or("--miv-sound-migrate-only requires --miv-sound-migration-source")?;
        let data_index = args.iter().position(|arg| arg == "--data-dir").unwrap();
        let data_dir = std::path::absolute(&args[data_index + 1]).map_err(|e| e.to_string())?;
        let mut source_close = close_arguments
            .map(|(pid, close_file)| {
                super::migration::SourceCloseProbe::new(
                    pid,
                    close_file,
                    &source.join("mimageviewer-vst3-host.exe").join("EBWebView"),
                )
            })
            .transpose()?;
        crate::data_dir::init();
        crate::logger::init_for_worker("miv-sound-migration-probe");
        let request = super::migration::MigrationRequest {
            source_root: source,
            destination: super::sound_data_root(&data_dir)?,
            mode: super::migration::MigrationMode::Alternate,
        };
        let started = std::time::Instant::now();
        let close_mode = source_close.is_some();
        let mut buffer_until_wait_end = close_mode;
        let mut evidence = Vec::new();
        let result = super::migration::migrate_with_observer(&request, |phase| {
            // Native liveness is sampled before JSON formatting or output.
            // Buffer through the wait so stdout cannot consume the short
            // lock-release-to-process-exit interval being measured.
            let observation = source_close
                .as_ref()
                .map(|probe| probe.observation())
                .transpose()?;
            let mut row = serde_json::json!({"phase": phase, "elapsed_ms": started.elapsed().as_millis(), "source_root": request.source_root, "destination": request.destination});
            if let Some(serde_json::Value::Object(fields)) = observation {
                row.as_object_mut().unwrap().extend(fields);
            }
            evidence.push(row);
            if phase == "before_lock"
                && let Some(probe) = &mut source_close
            {
                // Keep this the final operation before the production lock call.
                return probe.prepare_close();
            }
            if phase == "process_wait_end" {
                buffer_until_wait_end = false;
            }
            if !buffer_until_wait_end {
                flush_probe_evidence(&mut evidence)?;
            }
            let checkpoint = matches!(phase, "copy_begin" | "copy_end")
                || (phase == "lock_acquired" && !close_mode)
                || (phase == "process_wait_end" && close_mode);
            if checkpoint && let Some(directory) = &checkpoints {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let release = directory.join(format!("{phase}.continue"));
                while !release.is_file() {
                    if std::time::Instant::now() >= deadline {
                        return Err(format!("test checkpoint timed out: {phase}"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            Ok(())
        });
        flush_probe_evidence(&mut evidence)?;
        println!(
            "{}",
            serde_json::json!({"phase":"result", "elapsed_ms":started.elapsed().as_millis(), "passed":result.is_ok(), "result":format!("{result:?}")})
        );
        Ok(Some(if result.is_ok() { 0 } else { 1 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "test-script")]
    fn close_probe_args(close: &std::path::Path) -> Vec<std::ffi::OsString> {
        vec![
            "core".into(),
            "--miv-sound-migrate-only".into(),
            "--miv-sound-migration-original-browser".into(),
            "1234".into(),
            "--miv-sound-migration-close-file".into(),
            close.as_os_str().to_owned(),
        ]
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn source_close_probe_requires_paired_flags_migrate_only_and_checkpoints() {
        let close = PathBuf::from("unused-stop");
        let mut args = close_probe_args(&close);
        assert!(source_close_arguments(&args, None, std::path::Path::new("unused")).is_err());
        args.remove(1);
        assert!(source_close_arguments(&args, None, std::path::Path::new("unused")).is_err());
        let mut args = close_probe_args(&close);
        args.truncate(4);
        assert!(source_close_arguments(&args, None, std::path::Path::new("unused")).is_err());
        let mut args = close_probe_args(&close);
        args[3] = "0".into();
        assert!(source_close_arguments(&args, None, std::path::Path::new("unused")).is_err());
        args[3] = "not-a-pid".into();
        assert!(source_close_arguments(&args, None, std::path::Path::new("unused")).is_err());
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn source_close_probe_accepts_only_a_new_direct_run_file_outside_appdata() {
        let fixture = tempfile::tempdir().unwrap();
        let appdata = fixture.path().join("real-appdata");
        let run_root = fixture.path().join("isolated-run");
        let checkpoints = run_root.join("checkpoints");
        std::fs::create_dir_all(&appdata).unwrap();
        std::fs::create_dir_all(&checkpoints).unwrap();
        let close = run_root.join("source.stop");
        let parsed =
            source_close_arguments(&close_probe_args(&close), Some(&checkpoints), &appdata)
                .unwrap()
                .unwrap();
        assert_eq!(parsed, (1234, close.clone()));
        assert!(!close.exists());
        for invalid in [
            fixture.path().join("outside.stop"),
            checkpoints.join("nested.stop"),
        ] {
            assert!(
                source_close_arguments(&close_probe_args(&invalid), Some(&checkpoints), &appdata)
                    .is_err()
            );
        }
        std::fs::write(&close, b"existing").unwrap();
        assert!(
            source_close_arguments(&close_probe_args(&close), Some(&checkpoints), &appdata)
                .is_err()
        );
        assert_eq!(std::fs::read(&close).unwrap(), b"existing");
        let real_checkpoints = appdata.join("checkpoints");
        std::fs::create_dir(&real_checkpoints).unwrap();
        assert!(
            source_close_arguments(
                &close_probe_args(&appdata.join("forbidden.stop")),
                Some(&real_checkpoints),
                &appdata
            )
            .is_err()
        );
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn source_close_probe_rejects_missing_and_duplicate_argument_values() {
        let close = PathBuf::from("unused-stop");
        let mut args = close_probe_args(&close);
        args.pop();
        assert!(probe_value(&args, "--miv-sound-migration-close-file").is_err());
        args.push("--another-flag".into());
        assert!(probe_value(&args, "--miv-sound-migration-close-file").is_err());
        let mut args = close_probe_args(&close);
        args.extend([
            "--miv-sound-migration-original-browser".into(),
            "5678".into(),
        ]);
        assert!(probe_value(&args, "--miv-sound-migration-original-browser").is_err());
    }

    #[test]
    fn relative_data_dir_migration_uses_the_bridge_sound_data_root() {
        let fixture = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let data_dir = PathBuf::from(fixture.path().file_name().unwrap()).join("isolated-profile");
        let request = migration_request(data_dir.clone()).unwrap();
        assert!(request.destination.is_absolute());
        assert_eq!(
            request.destination,
            super::super::sound_data_root(&data_dir).unwrap()
        );
        assert_eq!(
            request.destination,
            fixture.path().join("isolated-profile").join("effetune")
        );
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn synthetic_source_requires_isolated_explicit_data_dir() {
        fn args(values: &[&str]) -> Vec<std::ffi::OsString> {
            values.iter().map(|value| (*value).into()).collect()
        }
        assert!(override_source(&args(&["core", "--miv-sound-migration-source"])).is_err());
        assert!(
            override_source(&args(&["core", "--miv-sound-migration-source", "fixture"])).is_err()
        );
        let appdata = std::env::var("APPDATA").unwrap();
        assert!(
            override_source(&args(&[
                "core",
                "--miv-sound-migration-source",
                &appdata,
                "--data-dir",
                "fixture-output"
            ]))
            .is_err()
        );
        let real_data = PathBuf::from(&appdata).join("mimageviewer");
        assert!(
            override_source(&args(&[
                "core",
                "--miv-sound-migration-source",
                "fixture",
                "--data-dir",
                &real_data.to_string_lossy()
            ]))
            .is_err()
        );
        assert!(
            override_source(&args(&[
                "core",
                "--miv-sound-migration-source",
                "fixture",
                "--data-dir",
                "fixture-output"
            ]))
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn pending_and_failed_migration_gate_both_host_types_correctly() {
        let mut owner = StartupMigration::pending();
        let gate = owner.gate();
        assert!(!gate.user_vst_allowed());
        assert!(!gate.sound_allowed());
        let (tx, rx) = mpsc::channel();
        owner.phase = Phase::Pending(rx);
        tx.send(Err("fixture".into())).unwrap();
        assert!(owner.poll().unwrap().is_err());
        assert!(gate.user_vst_allowed());
        assert!(!gate.sound_allowed());
        assert!(owner.poll().is_none());
    }

    #[test]
    fn successful_migration_gate_is_published_once() {
        let mut owner = StartupMigration::pending();
        let (tx, rx) = mpsc::channel();
        owner.phase = Phase::Pending(rx);
        tx.send(Ok(super::super::migration::MigrationOutcome::NotApplicable))
            .unwrap();
        assert!(owner.poll().unwrap().is_ok());
        assert!(owner.gate.sound_allowed());
        assert!(owner.gate.user_vst_allowed());
        assert!(owner.poll().is_none());
    }
}
