use super::*;
use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

struct Fixture {
    _temp: tempfile::TempDir,
    request: MigrationRequest,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let request = MigrationRequest {
            source_root: temp.path().join("old-appdata"),
            destination: temp.path().join("data").join("effetune"),
            mode: MigrationMode::Normal,
        };
        fs::create_dir_all(&request.source_root).unwrap();
        Self {
            _temp: temp,
            request,
        }
    }

    fn source(&self, relative: &str, contents: &[u8]) {
        write(&self.request.source_root.join(relative), contents);
    }

    fn destination(&self, relative: &str, contents: &[u8]) {
        write(&self.request.destination.join(relative), contents);
    }

    fn config(&self) {
        self.source("Frieve/EffeTunePlugin/config.json", br#"{"gain":0.5}"#);
    }

    fn webview(&self) {
        self.source(
            "mimageviewer-vst3-host.exe/EBWebView/Default/Local Storage/user",
            b"user data",
        );
    }

    fn run(&self, backend: &mut FakeBackend) -> Result<MigrationOutcome, String> {
        migrate_using(&self.request, backend, |_| Ok(()))
    }

    fn no_decision(&self) {
        assert!(!self.request.destination.join(DONE).exists());
        assert!(!self.request.destination.join(NOT_APPLICABLE).exists());
    }

    fn no_partial_copy(&self) {
        self.no_decision();
        assert!(!self.request.destination.join("plugin").exists());
        assert!(!self.request.destination.join("webview").exists());
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !path.exists() {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                result.insert(path.strip_prefix(root).unwrap().to_path_buf(), Vec::new());
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[derive(Default)]
struct FakeBackend {
    held: Rc<Cell<bool>>,
    time: Duration,
    inventories: VecDeque<Vec<ProcessCandidate<u32>>>,
    waits: VecDeque<(Duration, bool)>,
    waited: Vec<u32>,
    lock_calls: usize,
    diagnostic_calls: usize,
    lock_error: bool,
    inventory_error: bool,
    wait_error: bool,
}

struct FakeLease(Rc<Cell<bool>>);

impl Drop for FakeLease {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl MigrationBackend for FakeBackend {
    type Lease = FakeLease;
    type Process = u32;

    fn diagnostic_hosts(&mut self) {
        self.diagnostic_calls += 1;
    }

    fn lock(&mut self, _: &Path) -> Result<FakeLease, String> {
        self.lock_calls += 1;
        if self.lock_error {
            Err("exclusive lock denied".into())
        } else {
            assert!(!self.held.replace(true));
            Ok(FakeLease(self.held.clone()))
        }
    }

    fn processes(&mut self) -> Result<Vec<ProcessCandidate<u32>>, String> {
        assert!(self.held.get());
        if self.inventory_error {
            Err("inventory failed".into())
        } else {
            Ok(self.inventories.pop_front().unwrap_or_default())
        }
    }

    fn wait(&mut self, handle: u32, remaining: Duration) -> Result<bool, String> {
        assert!(self.held.get());
        self.waited.push(handle);
        if self.wait_error {
            return Err("wait handle failed".into());
        }
        let (elapsed, exited) = self
            .waits
            .pop_front()
            .unwrap_or((Duration::from_millis(20), true));
        self.time += elapsed.min(remaining);
        Ok(elapsed <= remaining && exited)
    }

    fn elapsed(&self) -> Duration {
        self.time
    }

    fn matches_udf(&self, command: &str, _: &Path) -> Result<bool, String> {
        match command {
            "source" => Ok(true),
            "other" => Ok(false),
            _ => Err("unreadable arguments".into()),
        }
    }
}

fn candidate(pid: u32, command: Option<&str>, owner: Owner, handle: bool) -> ProcessCandidate<u32> {
    ProcessCandidate {
        pid,
        command: command.map(str::to_owned),
        owner,
        synchronize: handle.then_some(pid),
    }
}

#[test]
fn migration_normal_decision_matrix_uses_only_selected_source_data() {
    for (source, expected) in [
        (None, MigrationOutcome::NotApplicable),
        (
            Some("Frieve/EffeTunePlugin/config.json"),
            MigrationOutcome::Done,
        ),
        (
            Some("Frieve/EffeTunePlugin/sfz-references.json"),
            MigrationOutcome::Done,
        ),
        (
            Some("Frieve/EffeTunePlugin/effetune_presets.json"),
            MigrationOutcome::Done,
        ),
        (
            Some("effetune/effetune_plugin_presets.json"),
            MigrationOutcome::Done,
        ),
        (
            Some("effetune/unrelated.json"),
            MigrationOutcome::NotApplicable,
        ),
    ] {
        let f = Fixture::new();
        if let Some(source) = source {
            f.source(source, b"{}");
        }
        assert_eq!(
            f.run(&mut FakeBackend::default()).unwrap(),
            expected,
            "{source:?}"
        );
        assert!(
            f.request
                .destination
                .join(if expected == MigrationOutcome::Done {
                    DONE
                } else {
                    NOT_APPLICABLE
                })
                .exists()
        );
    }
    let f = Fixture::new();
    f.webview();
    assert_eq!(
        f.run(&mut FakeBackend::default()).unwrap(),
        MigrationOutcome::Done
    );
}

#[test]
fn migration_alternate_requires_state_and_portable_is_never_eligible() {
    for mode in [MigrationMode::Alternate, MigrationMode::Portable] {
        for has_state in [false, true] {
            let mut f = Fixture::new();
            f.request.mode = mode;
            f.config();
            if has_state {
                f.destination("mixwright-state.json", b"original state");
            }
            let outcome = f.run(&mut FakeBackend::default()).unwrap();
            let eligible = mode == MigrationMode::Alternate && has_state;
            assert_eq!(
                outcome,
                if eligible {
                    MigrationOutcome::Done
                } else {
                    MigrationOutcome::NotApplicable
                }
            );
            assert_eq!(f.request.destination.join("plugin").exists(), eligible);
            if has_state {
                assert_eq!(
                    fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
                    b"original state"
                );
            }
        }
    }
    let mut f = Fixture::new();
    f.request.mode = MigrationMode::Alternate;
    f.destination("mixwright-state.json", b"original state");
    assert_eq!(
        f.run(&mut FakeBackend::default()).unwrap(),
        MigrationOutcome::Done
    );
    assert!(f.request.destination.join(DONE).is_file());
    assert!(!f.request.destination.join("plugin").exists());
    assert!(!f.request.destination.join("webview").exists());
}

#[test]
fn migration_existing_either_decision_prevents_all_source_work() {
    for mark in [DONE, NOT_APPLICABLE] {
        let f = Fixture::new();
        f.destination(mark, b"decision");
        f.destination("plugin/keep", b"committed plugin data");
        f.destination("webview/keep", b"committed webview data");
        f.config();
        f.webview();
        let before = snapshot(&f.request.destination);
        let mut backend = FakeBackend {
            lock_error: true,
            ..Default::default()
        };
        assert_eq!(
            f.run(&mut backend).unwrap(),
            MigrationOutcome::AlreadyDecided
        );
        assert_eq!(before, snapshot(&f.request.destination));
        assert_eq!(backend.lock_calls, 0);
        assert_eq!(backend.diagnostic_calls, 0);
    }
}

#[test]
fn migration_directory_at_decision_name_is_not_a_valid_decision() {
    for mark in [DONE, NOT_APPLICABLE] {
        let f = Fixture::new();
        f.config();
        fs::create_dir_all(f.request.destination.join(mark)).unwrap();
        let before = snapshot(&f.request.destination);
        assert!(f.run(&mut FakeBackend::default()).is_err());
        assert_eq!(before, snapshot(&f.request.destination));
    }
}

#[test]
fn migration_destination_must_be_a_directory_even_for_ineligible_modes() {
    for mode in [
        MigrationMode::Normal,
        MigrationMode::Alternate,
        MigrationMode::Portable,
    ] {
        let mut f = Fixture::new();
        f.request.mode = mode;
        write(&f.request.destination, b"existing destination file");
        assert!(f.run(&mut FakeBackend::default()).is_err());
        assert_eq!(
            fs::read(&f.request.destination).unwrap(),
            b"existing destination file"
        );
    }
}

#[test]
fn migration_json_only_never_touches_webview_lock_or_processes() {
    let f = Fixture::new();
    f.config();
    f.source(
        "Frieve/EffeTunePlugin/sfz-references.json",
        br#"["C:\\sfz\\piano.sfz"]"#,
    );
    f.destination("mixwright-state.json", b"preserve me");
    let before = snapshot(&f.request.source_root);
    let mut backend = FakeBackend {
        lock_error: true,
        ..Default::default()
    };
    assert_eq!(f.run(&mut backend).unwrap(), MigrationOutcome::Done);
    assert_eq!(backend.lock_calls, 0);
    assert!(!f.request.destination.join("webview").exists());
    assert_eq!(before, snapshot(&f.request.source_root));
    assert_eq!(
        fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
        b"preserve me"
    );
}

#[test]
fn migration_presets_choose_one_directory_even_when_preferred_is_empty() {
    for preferred_has_preset in [false, true] {
        let f = Fixture::new();
        f.config();
        f.source("Frieve/EffeTunePlugin/effetune_presets.json", b"[1]");
        f.source("Frieve/EffeTunePlugin/effetune_plugin_presets.json", b"[2]");
        fs::create_dir_all(f.request.source_root.join("effetune")).unwrap();
        if preferred_has_preset {
            f.source("effetune/effetune_presets.json", b"[3]");
        }
        assert_eq!(
            f.run(&mut FakeBackend::default()).unwrap(),
            MigrationOutcome::Done
        );
        let plugin = f.request.destination.join("plugin");
        assert_eq!(
            plugin.join("effetune_presets.json").exists(),
            preferred_has_preset
        );
        if preferred_has_preset {
            assert_eq!(
                fs::read(plugin.join("effetune_presets.json")).unwrap(),
                b"[3]"
            );
        }
        assert!(!plugin.join("effetune_plugin_presets.json").exists());
    }
}

#[test]
fn migration_legacy_preset_fallback_copies_both_known_files_and_no_other_json() {
    let f = Fixture::new();
    for name in [
        "config.json",
        "sfz-references.json",
        "effetune_presets.json",
        "effetune_plugin_presets.json",
        "unrelated.json",
    ] {
        f.source(&format!("Frieve/EffeTunePlugin/{name}"), b"{}");
    }
    assert_eq!(
        f.run(&mut FakeBackend::default()).unwrap(),
        MigrationOutcome::Done
    );
    assert_eq!(
        fs::read_dir(f.request.destination.join("plugin"))
            .unwrap()
            .count(),
        4
    );
    assert!(!f.request.destination.join("plugin/unrelated.json").exists());
}

#[test]
fn migration_webview_exclusions_are_exact_and_original_tree_is_unchanged() {
    let f = Fixture::new();
    f.webview();
    let excluded = [
        "EBWebView/lockfile",
        "EBWebView/Crashpad/reports/x",
        "EBWebView/CrashpadMetrics.pma",
        "EBWebView/CrashpadMetrics-active.pma",
        "EBWebView/BrowserMetrics/x",
        "EBWebView/DeferredBrowserMetrics/x",
        "EBWebView/EBWebViewMetrics/x",
    ];
    let included = [
        "EBWebView/CrashpadMetrics-other.pma",
        "EBWebView/CrashpadMetrics.pma.backup",
        "EBWebView/BrowserMetricsExtra/x",
        "EBWebView/Default/Crashpad/x",
        "EBWebView/Default/lockfile",
        "lockfile",
        "Crashpad/x",
        "EBWebView/Default/IndexedDB/visualizer",
        "EBWebView/Default/File System/ir",
    ];
    for path in excluded.iter().chain(included.iter()) {
        f.source(
            &format!("mimageviewer-vst3-host.exe/{path}"),
            path.as_bytes(),
        );
    }
    let original = snapshot(&f.request.source_root);
    let mut backend = FakeBackend::default();
    let held = backend.held.clone();
    let mut phases = Vec::new();
    let outcome = migrate_using(&f.request, &mut backend, |phase| {
        assert_eq!(held.get(), phase != "before_lock", "lease state at {phase}");
        phases.push(phase.to_owned());
        Ok(())
    })
    .unwrap();
    assert_eq!(outcome, MigrationOutcome::Done);
    assert_eq!(
        phases,
        [
            "before_lock",
            "lock_acquired",
            "process_wait_begin",
            "process_wait_end",
            "copy_begin",
            "copy_end"
        ]
    );
    assert!(!held.get());
    let target = f.request.destination.join("webview");
    for path in excluded {
        assert!(!target.join(path).exists(), "excluded {path}");
    }
    for path in included {
        assert_eq!(
            fs::read(target.join(path)).unwrap(),
            path.as_bytes(),
            "included {path}"
        );
    }
    assert_eq!(original, snapshot(&f.request.source_root));
}

#[test]
fn migration_invalid_copied_json_cleans_only_its_new_directories() {
    for name in [
        "config.json",
        "sfz-references.json",
        "effetune_presets.json",
        "effetune_plugin_presets.json",
    ] {
        let f = Fixture::new();
        f.config();
        f.source(&format!("Frieve/EffeTunePlugin/{name}"), b"not JSON");
        f.webview();
        f.destination("mixwright-state.json", b"state");
        f.destination("other/keep", b"unrelated");
        let before = snapshot(&f.request.source_root);
        let error = f.run(&mut FakeBackend::default()).unwrap_err();
        assert!(error.contains("invalid copied JSON"), "{error}");
        f.no_partial_copy();
        assert_eq!(before, snapshot(&f.request.source_root));
        assert_eq!(
            fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
            b"state"
        );
        assert_eq!(
            fs::read(f.request.destination.join("other/keep")).unwrap(),
            b"unrelated"
        );
    }
}

#[test]
fn migration_copy_read_failure_cleans_new_plugin_directory() {
    let f = Fixture::new();
    f.config();
    let source = f
        .request
        .source_root
        .join("Frieve/EffeTunePlugin/config.json");
    let outcome = migrate_using(&f.request, &mut FakeBackend::default(), |phase| {
        if phase == "copy_begin" {
            fs::remove_file(&source).unwrap();
        }
        Ok(())
    });
    assert!(outcome.is_err());
    f.no_partial_copy();
}

#[test]
fn migration_interrupted_copy_is_discarded_and_reimported_on_next_start() {
    let f = Fixture::new();
    f.config();
    f.webview();
    f.destination("mixwright-state.json", b"opaque original state");
    f.destination("other/keep", b"unrelated data");
    let original = snapshot(&f.request.source_root);
    // An unwind skips the ordinary Result-error cleanup, like a process exit
    // between copying and publishing the decision (without killing a process).
    let interrupted = std::panic::catch_unwind(|| {
        migrate_using(&f.request, &mut FakeBackend::default(), |phase| {
            if phase == "copy_end" {
                assert!(!f.request.destination.join(DONE).exists());
                panic!("simulate exit before migration commit");
            }
            Ok(())
        })
    });
    assert!(interrupted.is_err());
    f.no_decision();
    assert!(f.request.destination.join("plugin/config.json").exists());
    assert!(f.request.destination.join("webview/EBWebView").exists());
    f.destination("plugin/stale", b"incomplete prior attempt");
    f.destination("webview/stale/nested", b"incomplete prior attempt");
    f.source("Frieve/EffeTunePlugin/config.json", br#"{"gain":0.75}"#);
    assert_eq!(
        f.run(&mut FakeBackend::default()).unwrap(),
        MigrationOutcome::Done
    );
    assert!(f.request.destination.join(DONE).is_file());
    assert!(!f.request.destination.join("plugin/stale").exists());
    assert!(!f.request.destination.join("webview/stale").exists());
    assert_eq!(
        fs::read(f.request.destination.join("plugin/config.json")).unwrap(),
        br#"{"gain":0.75}"#
    );
    assert_eq!(
        fs::read(
            f.request
                .destination
                .join("webview/EBWebView/Default/Local Storage/user")
        )
        .unwrap(),
        b"user data"
    );
    assert_eq!(
        fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
        b"opaque original state"
    );
    assert_eq!(
        fs::read(f.request.destination.join("other/keep")).unwrap(),
        b"unrelated data"
    );
    let mut expected = original;
    expected.insert(
        PathBuf::from("Frieve/EffeTunePlugin/config.json"),
        br#"{"gain":0.75}"#.to_vec(),
    );
    assert_eq!(snapshot(&f.request.source_root), expected);
}

#[test]
fn migration_ineligible_decision_also_discards_only_uncommitted_copy_roots() {
    for mode in [
        MigrationMode::Normal,
        MigrationMode::Alternate,
        MigrationMode::Portable,
    ] {
        let mut f = Fixture::new();
        f.request.mode = mode;
        f.destination("plugin/stale", b"partial");
        f.destination("webview/stale", b"partial");
        f.destination("other/keep", b"unrelated");
        if mode == MigrationMode::Portable {
            f.destination("mixwright-state.json", b"opaque state");
        }
        assert_eq!(
            f.run(&mut FakeBackend::default()).unwrap(),
            MigrationOutcome::NotApplicable
        );
        assert!(!f.request.destination.join("plugin").exists());
        assert!(!f.request.destination.join("webview").exists());
        assert_eq!(
            fs::read(f.request.destination.join("other/keep")).unwrap(),
            b"unrelated"
        );
        if mode == MigrationMode::Portable {
            assert_eq!(
                fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
                b"opaque state"
            );
        }
    }
}

#[test]
fn migration_cleanup_rejects_source_overlap_before_deleting_leftovers() {
    let mut f = Fixture::new();
    f.request.destination = f.request.source_root.join("effetune");
    f.destination("plugin/stale", b"preserve old source");
    let before = snapshot(&f.request.source_root);
    assert!(
        f.run(&mut FakeBackend::default())
            .unwrap_err()
            .contains("overlap")
    );
    assert_eq!(snapshot(&f.request.source_root), before);
    f.no_decision();
}

#[cfg(windows)]
#[test]
fn migration_locked_leftover_fails_without_decision_and_retries_next_start() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    f.config();
    f.destination("plugin/busy", b"interrupted copy");
    f.destination("mixwright-state.json", b"preserve original state");
    let original = snapshot(&f.request.source_root);
    let handle = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(f.request.destination.join("plugin/busy"))
        .unwrap();
    assert!(
        f.run(&mut FakeBackend::default())
            .unwrap_err()
            .contains("remove interrupted migration copy")
    );
    f.no_decision();
    assert_eq!(
        fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
        b"preserve original state"
    );
    assert_eq!(snapshot(&f.request.source_root), original);
    drop(handle);
    assert_eq!(
        f.run(&mut FakeBackend::default()).unwrap(),
        MigrationOutcome::Done
    );
    assert!(!f.request.destination.join("plugin/busy").exists());
    assert_eq!(
        fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
        b"preserve original state"
    );
    assert_eq!(snapshot(&f.request.source_root), original);
}

#[test]
fn migration_missing_ebwebview_does_not_create_source_directories() {
    let f = Fixture::new();
    f.config();
    fs::create_dir_all(f.request.source_root.join("mimageviewer-vst3-host.exe")).unwrap();
    let before = snapshot(&f.request.source_root);
    assert!(f.run(&mut FakeBackend::default()).is_err());
    assert_eq!(before, snapshot(&f.request.source_root));
    f.no_partial_copy();
}

#[test]
fn migration_lock_inventory_and_wait_failures_publish_no_decision() {
    for failure in 0..3 {
        let f = Fixture::new();
        f.config();
        f.webview();
        let mut backend = FakeBackend {
            lock_error: failure == 0,
            inventory_error: failure == 1,
            wait_error: failure == 2,
            inventories: [vec![candidate(42, Some("source"), Owner::SameUser, true)]].into(),
            ..Default::default()
        };
        assert!(f.run(&mut backend).is_err());
        assert_eq!(backend.diagnostic_calls, 1);
        assert!(!backend.held.get());
        f.no_partial_copy();
    }
}

#[test]
fn migration_process_policy_covers_quiet_terminating_unknown_and_other_users() {
    for (command, owner, handle, should_wait, success) in [
        (Some("other"), Owner::SameUser, false, false, true),
        (Some("source"), Owner::SameUser, true, true, true),
        (None, Owner::SameUser, true, true, true), // terminating/read failure
        (None, Owner::Unknown, true, true, true),
        (Some("malformed"), Owner::Unknown, true, true, true),
        (None, Owner::OtherUser, false, false, true),
        (Some("source"), Owner::OtherUser, true, true, true), // exact path still counts
        (None, Owner::Unknown, false, false, false),
        (None, Owner::SameUser, false, false, false),
        (Some("source"), Owner::SameUser, false, false, false),
    ] {
        let f = Fixture::new();
        f.webview();
        let mut backend = FakeBackend {
            inventories: [vec![candidate(7, command, owner, handle)]].into(),
            ..Default::default()
        };
        let result = f.run(&mut backend);
        assert_eq!(result.is_ok(), success, "{command:?} {owner:?} {handle}");
        assert_eq!(!backend.waited.is_empty(), should_wait);
        assert!(!backend.held.get());
        if !success {
            f.no_partial_copy();
        }
    }
}

#[test]
fn migration_unreadable_source_process_that_stays_alive_fails_and_can_retry_next_start() {
    let f = Fixture::new();
    f.webview();
    let mut backend = FakeBackend {
        inventories: [vec![candidate(8, None, Owner::SameUser, true)]].into(),
        waits: [(Duration::from_secs(5), false)].into(),
        ..Default::default()
    };
    assert!(f.run(&mut backend).unwrap_err().contains("5 second"));
    f.no_partial_copy();
    assert_eq!(
        f.run(&mut FakeBackend::default()).unwrap(),
        MigrationOutcome::Done
    );
}

#[test]
fn migration_process_wait_uses_one_shared_five_second_deadline_across_batches() {
    let f = Fixture::new();
    f.webview();
    let mut backend = FakeBackend {
        inventories: [
            vec![candidate(1, Some("source"), Owner::SameUser, true)],
            vec![candidate(2, Some("source"), Owner::SameUser, true)],
        ]
        .into(),
        waits: [
            (Duration::from_secs(3), true),
            (Duration::from_secs(3), true),
        ]
        .into(),
        ..Default::default()
    };
    assert!(f.run(&mut backend).is_err());
    assert_eq!(backend.time, Duration::from_secs(5));
    assert_eq!(backend.waited, [1, 2]);
    f.no_partial_copy();
}

#[test]
fn migration_reenumerates_after_terminating_children_exit_before_copy() {
    let f = Fixture::new();
    f.webview();
    let mut backend = FakeBackend {
        inventories: [
            vec![candidate(1, None, Owner::Unknown, true)],
            vec![candidate(2, Some("source"), Owner::SameUser, true)],
            vec![],
        ]
        .into(),
        ..Default::default()
    };
    assert_eq!(f.run(&mut backend).unwrap(), MigrationOutcome::Done);
    assert_eq!(backend.waited, [1, 2]);
}

#[test]
fn migration_rejects_overlapping_source_and_destination_without_copying() {
    let mut f = Fixture::new();
    f.webview();
    f.request.destination = f
        .request
        .source_root
        .join("mimageviewer-vst3-host.exe/new-data");
    let before = snapshot(&f.request.source_root);
    assert!(
        f.run(&mut FakeBackend::default())
            .unwrap_err()
            .contains("overlap")
    );
    assert_eq!(before, snapshot(&f.request.source_root));
}

#[test]
fn migration_probe_checkpoint_failure_cleans_copies_without_publishing_a_decision() {
    for fail_at in [
        "before_lock",
        "lock_acquired",
        "process_wait_begin",
        "process_wait_end",
        "copy_begin",
        "copy_end",
    ] {
        let f = Fixture::new();
        f.config();
        f.webview();
        let mut backend = FakeBackend::default();
        let outcome = migrate_using(&f.request, &mut backend, |phase| {
            if phase == fail_at {
                Err("probe checkpoint failed".into())
            } else {
                Ok(())
            }
        });
        assert!(outcome.unwrap_err().contains("probe checkpoint failed"));
        f.no_partial_copy();
        assert!(!backend.held.get());
    }
}

#[test]
fn migration_observer_identifies_the_actual_process_wait_before_copying() {
    for wait_error in [false, true] {
        let f = Fixture::new();
        f.webview();
        let mut backend = FakeBackend {
            inventories: [vec![candidate(77, Some("source"), Owner::SameUser, true)]].into(),
            wait_error,
            ..Default::default()
        };
        let mut phases = Vec::new();
        let result = migrate_using(&f.request, &mut backend, |phase| {
            phases.push(phase.to_owned());
            Ok(())
        });
        assert_eq!(backend.waited, [77]);
        if wait_error {
            assert!(result.is_err());
            assert_eq!(phases.last().unwrap(), "process_wait_pid_begin:77");
            f.no_partial_copy();
        } else {
            assert_eq!(result.unwrap(), MigrationOutcome::Done);
            assert_eq!(
                phases,
                [
                    "before_lock",
                    "lock_acquired",
                    "process_wait_begin",
                    "process_wait_pid_begin:77",
                    "process_wait_pid_end:77",
                    "process_wait_end",
                    "copy_begin",
                    "copy_end",
                ]
            );
        }
    }
}

#[test]
fn migration_mark_publication_failure_cleans_both_copies_and_preserves_other_data() {
    let f = Fixture::new();
    f.config();
    f.webview();
    f.destination("mixwright-state.json", b"original state");
    let obstruction = f.request.destination.join(DONE);
    let result = migrate_using(&f.request, &mut FakeBackend::default(), |phase| {
        if phase == "copy_end" {
            // A deliberately injected external obstruction, not a decision
            // created by the engine, makes create_new fail atomically.
            fs::create_dir(&obstruction).unwrap();
        }
        Ok(())
    });
    assert!(result.unwrap_err().contains("publish migration decision"));
    assert!(!f.request.destination.join("plugin").exists());
    assert!(!f.request.destination.join("webview").exists());
    assert_eq!(
        fs::read(f.request.destination.join("mixwright-state.json")).unwrap(),
        b"original state"
    );
    assert!(obstruction.is_dir());
    fs::remove_dir(obstruction).unwrap();
    f.no_decision();
}

#[cfg(windows)]
#[test]
fn migration_windows_lock_preserves_existing_file_and_denies_competing_opens() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("lockfile");
    fs::write(&path, b"existing singleton bytes").unwrap();
    let mut backend = windows::Backend::new();
    let lease = backend.lock(&path).unwrap();
    assert!(
        OpenOptions::new()
            .read(true)
            .share_mode(7)
            .open(&path)
            .is_err()
    );
    assert!(backend.lock(&path).is_err());
    drop(lease);
    assert_eq!(fs::read(path).unwrap(), b"existing singleton bytes");
}

#[cfg(windows)]
#[test]
fn migration_windows_missing_lock_is_create_new_and_delete_on_close() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("lockfile");
    let mut backend = windows::Backend::new();
    let lease = backend.lock(&path).unwrap();
    assert!(backend.lock(&path).is_err());
    drop(lease);
    assert!(!path.exists());
}

#[cfg(windows)]
#[test]
fn migration_windows_active_external_lock_fails_without_mark_or_copies() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    f.config();
    f.webview();
    let path = f
        .request
        .source_root
        .join("mimageviewer-vst3-host.exe/EBWebView/lockfile");
    fs::write(&path, b"original lock").unwrap();
    let _external = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert!(
        migrate(&f.request)
            .unwrap_err()
            .contains("exclusive WebView2 lock")
    );
    f.no_partial_copy();
}

#[cfg(windows)]
#[test]
fn migration_windows_webview_copy_failure_cleans_both_new_trees() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    f.config();
    f.webview();
    let file = f
        .request
        .source_root
        .join("mimageviewer-vst3-host.exe/EBWebView/Default/Local Storage/user");
    let _exclusive = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(file)
        .unwrap();
    assert!(f.run(&mut FakeBackend::default()).is_err());
    f.no_partial_copy();
}

#[cfg(windows)]
#[test]
fn migration_windows_argv_matches_only_one_exact_normalized_user_data_dir() {
    let expected = Path::new(r"C:\Legacy Data\EBWebView");
    for (arguments, matches) in [
        (r#"--user-data-dir="C:\Legacy Data\EBWebView""#, true),
        (
            r#"--type=renderer --user-data-dir "C:\Legacy Data\EBWebView""#,
            true,
        ),
        (
            r#"--type=crashpad-handler --USER-DATA-DIR="c:/legacy data/EBWebView/""#,
            true,
        ),
        (r#"--user-data-dir="C:\Legacy Data\a\..\EBWebView""#, true),
        (r#"--user-data-dir="\\?\C:\Legacy Data\EBWebView""#, true),
        (r#"--user-data-dir="C:\Legacy Data\EBWebView-other""#, false),
        (
            r#"--user-data-dir="C:\Legacy Data\EBWebView\Default""#,
            false,
        ),
        (r#"--database="C:\Legacy Data\EBWebView\Crashpad""#, false),
        (
            r#"--unrelated="--user-data-dir=C:\Legacy Data\EBWebView""#,
            false,
        ),
    ] {
        assert_eq!(
            windows::matches_udf(&format!("msedgewebview2.exe {arguments}"), expected).unwrap(),
            matches,
            "{arguments}"
        );
    }
    for arguments in [
        r#"--user-data-dir="C:\Legacy Data\EBWebView" --user-data-dir="C:\Legacy Data\EBWebView""#,
        r#"--user-data-dir="C:\Legacy Data\EBWebView" --user-data-dir=C:\Other"#,
        "--user-data-dir",
        "--user-data-dir=",
        "--user-data-dir=relative",
    ] {
        assert!(
            windows::matches_udf(&format!("msedgewebview2.exe {arguments}"), expected).is_err(),
            "must conservatively wait for malformed command: {arguments}"
        );
    }
}

#[cfg(windows)]
#[test]
fn migration_windows_native_command_line_reader_and_current_sid_work_without_webview() {
    windows::test_current_process_query();
}
