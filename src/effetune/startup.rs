//! One App-owned migration operation; atomics only project its host permission
//! to existing background consumers. No filesystem work runs on the UI thread.
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc,
};

mod path_identity;

pub(crate) const FAILURE_NOTICE: &str = "音響調整のデータを引き継げませんでした。この mImageViewer も含め、トレイに常駐しているものもすべて終了してから、1 つだけ起動してください";

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
    Pending(mpsc::Receiver<Result<super::migration::MigrationOutcome, String>>),
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
        mpsc::Sender<Result<super::migration::MigrationOutcome, String>>,
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
                    .and_then(|request| super::migration::migrate(&request));
                let _ = tx.send(result);
                if let Some(ctx) = wake {
                    ctx.request_repaint();
                }
            });
        if let Err(error) = spawn {
            // Feed spawn failure through the same one-shot terminal path.
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Err(format!("migration worker spawn: {error}")));
            self.phase = Phase::Pending(rx);
        }
    }

    pub(crate) fn poll(&mut self) -> Option<Result<super::migration::MigrationOutcome, String>> {
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
    let data_dir = std::path::absolute(data_dir).map_err(|e| e.to_string())?;
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
        destination: data_dir.join("effetune"),
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

/// Early, feature-gated diagnostic route. The migrate-only path never creates
/// an App, reads settings, starts a bridge, or initializes WebView2.
pub(crate) fn handle_test_arguments() -> Result<Option<i32>, String> {
    let args = std::env::args_os().collect::<Vec<_>>();
    let requested = args.iter().any(|arg| {
        arg == "--miv-sound-migrate-only"
            || arg == "--miv-sound-migration-source"
            || arg == "--miv-sound-migration-checkpoints"
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
        let checkpoints = args
            .iter()
            .position(|arg| arg == "--miv-sound-migration-checkpoints")
            .map(|index| {
                args.get(index + 1)
                    .map(PathBuf::from)
                    .ok_or("checkpoint directory requires a path")
            })
            .transpose()?;
        crate::data_dir::init();
        crate::logger::init_for_worker("miv-sound-migration-probe");
        let request = super::migration::MigrationRequest {
            source_root: source,
            destination: data_dir.join("effetune"),
            mode: super::migration::MigrationMode::Alternate,
        };
        let started = std::time::Instant::now();
        let result = super::migration::migrate_with_observer(&request, |phase| {
            println!(
                "{}",
                serde_json::json!({"phase": phase, "elapsed_ms": started.elapsed().as_millis(), "source_root": request.source_root, "destination": request.destination})
            );
            use std::io::Write;
            std::io::stdout()
                .flush()
                .map_err(|error| format!("flush migration evidence: {error}"))?;
            if matches!(phase, "lock_acquired" | "copy_begin" | "copy_end")
                && let Some(directory) = &checkpoints
            {
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
