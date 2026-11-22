use super::*;
use backend::{Environment, Files, HOST_EXE, NativeFiles, REGISTRY_KEYS, Registry};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

fn environment(root: &Path) -> Environment {
    let data_dir = root.join("appdata").join("mimageviewer");
    fs::create_dir_all(&data_dir).unwrap();
    let launcher = root.join("launcher.exe");
    fs::write(&launcher, b"launcher").unwrap();
    let host_source = root.join(HOST_EXE);
    fs::write(&host_source, b"host-v1").unwrap();
    Environment {
        app_data: root.join("appdata"),
        data_dir,
        launcher: Some(launcher),
        host_source,
        default_output: root.join("Pictures").join("mimageviewer").join("browser"),
        portable: false,
        explicit_data_dir: false,
    }
}
#[derive(Default)]
struct MemoryRegistry {
    values: BTreeMap<String, PathBuf>,
    writes: usize,
}
impl Registry for MemoryRegistry {
    fn get(&mut self, key: &str) -> Result<Option<PathBuf>, String> {
        Ok(self.values.get(key).cloned())
    }
    fn set(&mut self, key: &str, value: &Path) -> Result<(), String> {
        self.values.insert(key.into(), value.into());
        self.writes += 1;
        Ok(())
    }
    fn remove(&mut self, key: &str) -> Result<(), String> {
        self.values.remove(key);
        Ok(())
    }
}
#[derive(Default)]
struct TestFiles {
    fail_config: bool,
    busy: Option<PathBuf>,
}
impl Files for TestFiles {
    fn copy_host(&mut self, source: &Path, target: &Path) -> io::Result<()> {
        NativeFiles.copy_host(source, target)
    }
    fn write_config(&mut self, target: &Path, bytes: &[u8]) -> io::Result<()> {
        if self.fail_config {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected config failure",
            ))
        } else {
            NativeFiles.write_config(target, bytes)
        }
    }
    fn remove_exe(&mut self, target: &Path) -> io::Result<()> {
        if self.busy.as_deref() == Some(target) {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected in-use image",
            ))
        } else {
            NativeFiles.remove_exe(target)
        }
    }
}
fn generation(env: &Environment) -> PathBuf {
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(env.manifest()).unwrap()).unwrap();
    PathBuf::from(manifest["path"].as_str().unwrap())
        .parent()
        .unwrap()
        .into()
}

#[test]
fn browser818_registration_default_portable_explicit_and_unknown_launcher_ownership() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    assert!(env.eligibility().is_ok());
    for case in 0..4 {
        let mut forbidden = env.clone();
        match case {
            0 => forbidden.portable = true,
            1 => forbidden.explicit_data_dir = true,
            2 => forbidden.launcher = None,
            _ => forbidden.data_dir = root.path().join("other-profile"),
        }
        let mut registry = MemoryRegistry::default();
        registry
            .values
            .insert(REGISTRY_KEYS[0].into(), PathBuf::from("keep-existing"));
        assert!(
            backend::prepare_with(
                forbidden.clone(),
                Selection {
                    enabled: true,
                    output_dir: None
                },
                &mut registry,
                &mut NativeFiles
            )
            .ready
            .is_err()
        );
        assert!(
            backend::register(
                &forbidden,
                &env.default_output,
                &mut registry,
                &mut NativeFiles
            )
            .is_err()
        );
        assert!(backend::unregister(&forbidden, &mut registry, &mut NativeFiles).is_err());
        assert_eq!(
            registry.values[REGISTRY_KEYS[0]],
            PathBuf::from("keep-existing")
        );
        assert_eq!(registry.writes, 0);
        assert!(!env.managed().exists());
    }
}

#[test]
fn browser818_registration_complete_generation_precedes_manifest_and_registry() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let mut registry = MemoryRegistry::default();
    let mut files = TestFiles {
        fail_config: true,
        ..Default::default()
    };
    assert!(backend::register(&env, &env.default_output, &mut registry, &mut files).is_err());
    assert!(!env.manifest().exists());
    assert_eq!(registry.writes, 0);
    files.fail_config = false;
    backend::register(&env, &env.default_output, &mut registry, &mut files).unwrap();
    let published = generation(&env);
    assert_eq!(fs::read(published.join(HOST_EXE)).unwrap(), b"host-v1");
    let config: mimageviewer_browser_host::Config =
        serde_json::from_slice(&fs::read(published.join("config.json")).unwrap()).unwrap();
    assert_eq!(config.launcher, env.launcher.clone().unwrap());
    assert_eq!(config.output_dir, env.default_output);
    assert_eq!(config.v, 1);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(env.manifest()).unwrap()).unwrap();
    assert_eq!(
        manifest["name"],
        mimageviewer_browser_host::protocol::NATIVE_HOST_NAME
    );
    assert_eq!(
        manifest["allowed_origins"],
        serde_json::json!(mimageviewer_browser_host::protocol::ORIGINS)
    );
    for key in REGISTRY_KEYS {
        assert_eq!(registry.values[key], env.manifest());
        // The uninstaller matches this exact backslash-only spelling.
        assert!(!registry.values[key].to_string_lossy().contains('/'));
    }
    assert!(!env.default_output.exists());
    files.fail_config = true;
    let previous = fs::read(env.manifest()).unwrap();
    assert!(
        backend::register(
            &env,
            &root.path().join("changed"),
            &mut registry,
            &mut files
        )
        .is_err()
    );
    assert_eq!(fs::read(env.manifest()).unwrap(), previous);
    assert!(published.join("config.json").exists());
}

#[test]
fn browser818_registration_startup_and_destination_updates_keep_busy_generation_config() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let mut registry = MemoryRegistry::default();
    let mut files = TestFiles::default();
    let first = backend::prepare_with(
        env.clone(),
        Selection {
            enabled: true,
            output_dir: None,
        },
        &mut registry,
        &mut files,
    )
    .ready
    .unwrap();
    assert!(first.registered);
    let old = generation(&env);
    let old_config = fs::read(old.join("config.json")).unwrap();
    let second = backend::prepare_with(
        env.clone(),
        first.selection.clone(),
        &mut registry,
        &mut files,
    )
    .ready
    .unwrap();
    assert!(second.registered);
    assert_eq!(generation(&env), old);
    assert_eq!(registry.writes, 2);
    files.busy = Some(old.join(HOST_EXE));
    let destination = root.path().join("different-images");
    backend::register(&env, &destination, &mut registry, &mut files).unwrap();
    let newer = generation(&env);
    assert_ne!(old, newer);
    assert_eq!(fs::read(old.join("config.json")).unwrap(), old_config);
    fs::write(&env.host_source, b"host-v2").unwrap();
    let updated = backend::prepare_with(
        env.clone(),
        Selection {
            enabled: true,
            output_dir: Some(destination),
        },
        &mut registry,
        &mut files,
    )
    .ready
    .unwrap();
    assert!(updated.registered);
    let newest = generation(&env);
    assert_ne!(newest, newer);
    assert_eq!(fs::read(newest.join(HOST_EXE)).unwrap(), b"host-v2");
    assert!(!newer.exists());
    assert_eq!(fs::read(old.join("config.json")).unwrap(), old_config);
}

#[test]
fn browser818_registration_remove_only_owned_keys_keeps_images_and_busy_config() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let mut registry = MemoryRegistry::default();
    let mut files = TestFiles::default();
    backend::register(&env, &env.default_output, &mut registry, &mut files).unwrap();
    let old = generation(&env);
    files.busy = Some(old.join(HOST_EXE));
    fs::create_dir_all(&env.default_output).unwrap();
    fs::write(env.default_output.join("keep.png"), b"image").unwrap();
    registry.values.insert(
        REGISTRY_KEYS[1].into(),
        root.path().join("other-manifest.json"),
    );
    backend::unregister(&env, &mut registry, &mut files).unwrap();
    assert!(!registry.values.contains_key(REGISTRY_KEYS[0]));
    assert_eq!(
        registry.values[REGISTRY_KEYS[1]],
        root.path().join("other-manifest.json")
    );
    assert!(!env.manifest().exists());
    assert!(old.join("config.json").exists());
    assert!(old.join(HOST_EXE).exists());
    assert_eq!(
        fs::read(env.default_output.join("keep.png")).unwrap(),
        b"image"
    );
    files.busy = None;
    backend::unregister(&env, &mut registry, &mut files).unwrap();
    assert!(!env.managed().exists());
    assert!(env.default_output.join("keep.png").exists());
}

#[test]
fn browser818_registration_managed_destination_is_rejected_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let mut registry = MemoryRegistry::default();
    for destination in [env.managed(), env.managed().join("new/images")] {
        assert!(backend::register(&env, &destination, &mut registry, &mut NativeFiles).is_err());
        assert!(!env.managed().exists());
        assert_eq!(registry.writes, 0);
    }
}

#[derive(Default)]
struct TestFolders {
    selected: Option<PathBuf>,
    opened: Vec<PathBuf>,
}
impl backend::Folders for TestFolders {
    fn choose(&mut self, _: &Path) -> Result<Option<PathBuf>, String> {
        Ok(self.selected.clone())
    }
    fn open(&mut self, path: &Path) -> Result<(), String> {
        self.opened.push(path.to_owned());
        Ok(())
    }
}
#[test]
fn browser818_registration_picker_apply_reset_open_and_cancel_share_worker_validation() {
    let root = tempfile::tempdir().unwrap();
    let ready = prepared(environment(root.path()));
    let mut registry = MemoryRegistry::default();
    let mut folders = TestFolders::default();
    folders.selected = Some(ready.env.managed().join("images"));
    assert!(
        backend::perform_with(
            ready.clone(),
            Action::ChooseFolder,
            &mut registry,
            &mut NativeFiles,
            &mut folders
        )
        .is_err()
    );
    assert!(!ready.env.managed().exists());
    assert_eq!(registry.writes, 0);
    folders.selected = Some(root.path().join("chosen"));
    let selected = backend::perform_with(
        ready.clone(),
        Action::ChooseFolder,
        &mut registry,
        &mut NativeFiles,
        &mut folders,
    )
    .unwrap();
    assert_eq!(selected.selection.unwrap().output_dir, folders.selected);
    assert_eq!(registry.writes, 0); // Disabled integration does not register.
    folders.selected = None;
    let cancelled = backend::perform_with(
        ready.clone(),
        Action::ChooseFolder,
        &mut registry,
        &mut NativeFiles,
        &mut folders,
    )
    .unwrap();
    assert!(cancelled.selection.is_none());
    assert_eq!(cancelled.prepared.selection, ready.selection);
    assert!(
        backend::perform_with(
            ready.clone(),
            Action::OpenFolder(None),
            &mut registry,
            &mut NativeFiles,
            &mut folders
        )
        .is_err()
    );
    fs::create_dir_all(&ready.env.default_output).unwrap();
    backend::perform_with(
        ready.clone(),
        Action::OpenFolder(None),
        &mut registry,
        &mut NativeFiles,
        &mut folders,
    )
    .unwrap();
    assert_eq!(folders.opened, vec![ready.env.default_output.clone()]);
    let enabled = backend::perform_with(
        ready.clone(),
        Action::Apply(Selection {
            enabled: true,
            output_dir: Some(root.path().join("custom")),
        }),
        &mut registry,
        &mut NativeFiles,
        &mut folders,
    )
    .unwrap()
    .prepared;
    let reset = backend::perform_with(
        enabled,
        Action::Apply(Selection {
            enabled: true,
            output_dir: None,
        }),
        &mut registry,
        &mut NativeFiles,
        &mut folders,
    )
    .unwrap()
    .prepared;
    let config: mimageviewer_browser_host::Config =
        serde_json::from_slice(&fs::read(generation(&ready.env).join("config.json")).unwrap())
            .unwrap();
    assert_eq!(config.output_dir, ready.env.default_output);
    let removed = backend::perform_with(
        reset,
        Action::Remove,
        &mut registry,
        &mut NativeFiles,
        &mut folders,
    )
    .unwrap();
    assert!(!removed.prepared.selection.enabled);
    assert!(registry.values.is_empty());
}

#[cfg(windows)]
#[test]
fn browser818_registration_cleanup_does_not_follow_junctions() {
    use std::os::windows::process::CommandExt;
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let old = env.managed().join("generation-old");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join(HOST_EXE), b"old").unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), b"image").unwrap();
    let alias = old.join("alias");
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&outside)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let cleaned = backend::cleanup(&env, None, &mut NativeFiles);
    if alias.exists() {
        fs::remove_dir(&alias).unwrap();
    }
    cleaned.unwrap();
    assert!(!old.exists());
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"image");
}

#[test]
fn browser818_registration_settings_old_defaults_roundtrip_and_partial_save() {
    let defaults = crate::settings::Settings::default();
    let mut old = serde_json::to_value(&defaults).unwrap();
    old.as_object_mut()
        .unwrap()
        .remove("browser_integration_enabled");
    old.as_object_mut()
        .unwrap()
        .remove("browser_view_output_dir");
    let restored: crate::settings::Settings = serde_json::from_value(old).unwrap();
    assert!(!restored.browser_integration_enabled);
    assert!(restored.browser_view_output_dir.is_none());
    let root = tempfile::tempdir().unwrap();
    let db = crate::settings_db::SettingsDb::create_new(root.path()).unwrap();
    db.save_full(&defaults).unwrap();
    let mut previous = db.load_into_settings().unwrap();
    previous.clipboard_capture_image_enabled = true;
    previous.clipboard_capture_min_short_side_px = 480;
    db.save_full(&previous).unwrap();
    let mut expected = serde_json::to_value(db.load_into_settings().unwrap()).unwrap();
    let output = Some(root.path().join("画像 with spaces"));
    db.save_browser_integration(true, &output).unwrap();
    expected["browser_integration_enabled"] = serde_json::json!(true);
    expected["browser_view_output_dir"] = serde_json::json!(output);
    let loaded = db.load_into_settings().unwrap();
    assert_eq!(serde_json::to_value(&loaded).unwrap(), expected);
    let roundtrip: crate::settings::Settings = serde_json::from_value(expected).unwrap();
    assert!(roundtrip.browser_integration_enabled);
    assert_eq!(roundtrip.browser_view_output_dir, output);
}

#[derive(Default)]
struct FakeBackend {
    persisted: Mutex<Vec<Selection>>,
}
impl Backend for FakeBackend {
    fn prepare(&self, _: Selection) -> Preparation {
        panic!("tests inject preparation, never environment or registry")
    }
    fn perform(&self, mut prepared: Prepared, action: Action) -> Result<Completion, String> {
        let Action::Apply(selection) = action else {
            panic!("unexpected test action")
        };
        prepared.selection = selection.clone();
        prepared.registered = selection.enabled;
        Ok(Completion {
            prepared,
            selection: Some(selection),
        })
    }
    fn persist(&self, selection: &Selection) -> Result<(), String> {
        self.persisted.lock().unwrap().push(selection.clone());
        Ok(())
    }
}
fn prepared(env: Environment) -> Prepared {
    Prepared {
        env,
        selection: Selection::default(),
        registered: false,
        message: None,
    }
}
fn service(ready: Prepared, backend: Arc<dyn Backend>) -> BrowserIntegration {
    BrowserIntegration {
        close: None,
        default_output: Some(ready.env.default_output.clone()),
        state: BrowserIntegrationState::Available(ready),
        backend,
    }
}
/// Observe the real worker completion before polling it, without timing/sleeps.
fn await_worker(owner: &mut BrowserIntegration) {
    let previous = std::mem::replace(
        &mut owner.state,
        BrowserIntegrationState::Unavailable {
            reason: "test observation".into(),
        },
    );
    let BrowserIntegrationState::Processing { prepared, kind, rx } = previous else {
        panic!("expected one worker")
    };
    let result = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(result).unwrap();
    owner.state = BrowserIntegrationState::Processing { prepared, kind, rx };
}

#[test]
fn browser818_registration_owner_rejects_overlap_and_adopts_before_async_settings_save() {
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::default());
    let mut owner = service(prepared(environment(root.path())), backend.clone());
    let ctx = egui::Context::default();
    let mut settings = crate::settings::Settings::default();
    let selection = Selection {
        enabled: true,
        output_dir: Some(root.path().join("images")),
    };
    assert!(owner.submit(Action::Apply(selection.clone()), &ctx));
    assert!(!owner.submit(Action::Remove, &ctx));
    assert!(owner.owns_selection(&selection));
    assert!(!owner.owns_selection(&Selection::default()));
    assert_eq!(owner.view(&settings).selection, selection);
    assert!(!settings.browser_integration_enabled);
    await_worker(&mut owner);
    assert!(matches!(
        owner.poll(&mut settings, &ctx),
        Some(Event::Changed(_, None))
    ));
    assert_eq!(Selection::from_settings(&settings), selection);
    assert!(!owner.submit(Action::Remove, &ctx)); // Includes settings persistence.
    await_worker(&mut owner);
    assert!(owner.poll(&mut settings, &ctx).is_none());
    assert!(matches!(owner.view(&settings).status, Status::Available));
    assert_eq!(*backend.persisted.lock().unwrap(), vec![selection]);
}

#[test]
fn browser818_registration_owner_failure_keeps_selection_and_allows_explicit_next_action() {
    let root = tempfile::tempdir().unwrap();
    let ready = prepared(environment(root.path()));
    let backend = Arc::new(FakeBackend::default());
    let mut owner = service(ready.clone(), backend);
    let (tx, rx) = mpsc::channel();
    tx.send(Err("injected failure".into())).unwrap();
    owner.state = BrowserIntegrationState::Processing {
        prepared: ready,
        kind: JobKind::Action(Action::Remove),
        rx,
    };
    let mut settings = crate::settings::Settings::default();
    let ctx = egui::Context::default();
    assert!(matches!(
        owner.poll(&mut settings, &ctx),
        Some(Event::Failed(_))
    ));
    assert!(!settings.browser_integration_enabled);
    assert!(matches!(owner.view(&settings).status, Status::Available));
    assert!(owner.submit(Action::Apply(Selection::default()), &ctx));
    await_worker(&mut owner);
    assert!(matches!(
        owner.poll(&mut settings, &ctx),
        Some(Event::Changed(_, None))
    ));
    await_worker(&mut owner);
    owner.poll(&mut settings, &ctx);
    assert!(matches!(owner.view(&settings).status, Status::Available));
}

#[test]
fn browser818_registration_preparing_and_unavailable_never_admit_or_retry_operations() {
    let root = tempfile::tempdir().unwrap();
    let ready = prepared(environment(root.path()));
    let mut owner = service(ready.clone(), Arc::new(FakeBackend::default()));
    owner.state = BrowserIntegrationState::Preparing(PreparationTask::NotStarted);
    let ctx = egui::Context::default();
    let mut settings = crate::settings::Settings::default();
    assert!(!owner.submit(Action::Remove, &ctx));
    let (tx, rx) = mpsc::channel();
    owner.state = BrowserIntegrationState::Preparing(PreparationTask::Running(rx));
    assert!(owner.poll(&mut settings, &ctx).is_none());
    tx.send(Preparation {
        default_output: Some(ready.env.default_output.clone()),
        ready: Err("injected unavailable".into()),
    })
    .ok()
    .unwrap();
    assert!(owner.poll(&mut settings, &ctx).is_none());
    assert!(matches!(
        owner.view(&settings).status,
        Status::Unavailable(_)
    ));
    assert_eq!(owner.default_output(), Some(ready.env.default_output));
    for _ in 0..3 {
        assert!(!owner.submit(Action::Remove, &ctx));
        assert!(owner.poll(&mut settings, &ctx).is_none());
    }
}

#[test]
fn browser818_registration_close_continuation_spans_action_and_persistence() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = service(
        prepared(environment(root.path())),
        Arc::new(FakeBackend::default()),
    );
    let ctx = egui::Context::default();
    let selection = Selection {
        enabled: true,
        output_dir: None,
    };
    let mut settings = crate::settings::Settings::default();
    assert!(owner.submit(Action::Apply(selection.clone()), &ctx));
    assert!(owner.defer_root_close(RootCloseIntent::WindowClose));
    assert!(owner.defer_root_close(RootCloseIntent::ApplicationQuit));
    assert!(owner.defer_root_close(RootCloseIntent::WindowClose));
    assert_eq!(owner.take_ready_close(), None);
    await_worker(&mut owner);
    assert!(matches!(
        owner.poll(&mut settings, &ctx),
        Some(Event::Changed(_, None))
    ));
    assert_eq!(Selection::from_settings(&settings), selection);
    assert_eq!(owner.take_ready_close(), None);
    assert!(!owner.submit(Action::Remove, &ctx));
    await_worker(&mut owner);
    assert!(owner.poll(&mut settings, &ctx).is_none());
    assert_eq!(
        owner.take_ready_close(),
        Some(RootCloseIntent::ApplicationQuit)
    );
    assert_eq!(owner.take_ready_close(), None);
}

#[test]
fn browser818_registration_close_continuation_covers_startup_preparation() {
    let root = tempfile::tempdir().unwrap();
    let ready = prepared(environment(root.path()));
    let mut owner = service(ready, Arc::new(FakeBackend::default()));
    owner.state = BrowserIntegrationState::Preparing(PreparationTask::NotStarted);
    assert!(owner.defer_root_close(RootCloseIntent::ApplicationQuit));
    assert_eq!(owner.take_ready_close(), None);
    let ctx = egui::Context::default();
    let mut settings = crate::settings::Settings::default();
    // Inject the startup worker's channel at the next phase boundary. A rejected
    // Close must not permanently disable an otherwise available integration.
    let (tx, rx) = mpsc::channel();
    owner.state = BrowserIntegrationState::Preparing(PreparationTask::Running(rx));
    assert!(owner.defer_root_close(RootCloseIntent::ApplicationQuit));
    assert!(owner.poll(&mut settings, &ctx).is_none());
    tx.send(Preparation {
        default_output: None,
        ready: Err("injected startup failure".into()),
    })
    .ok()
    .unwrap();
    assert!(owner.poll(&mut settings, &ctx).is_none());
    assert_eq!(
        owner.take_ready_close(),
        Some(RootCloseIntent::ApplicationQuit)
    );
    assert_eq!(owner.take_ready_close(), None);
}

#[test]
fn browser818_registration_error_or_disconnect_retires_only_its_close_continuation() {
    let root = tempfile::tempdir().unwrap();
    for disconnected in [false, true] {
        let mut ready = prepared(environment(root.path()));
        ready.selection.enabled = true;
        let mut owner = service(ready.clone(), Arc::new(FakeBackend::default()));
        let (tx, rx) = mpsc::channel();
        if !disconnected {
            tx.send(Err("injected action failure".into())).unwrap();
        }
        drop(tx);
        owner.state = BrowserIntegrationState::Processing {
            prepared: ready,
            kind: JobKind::Action(Action::Remove),
            rx,
        };
        let mut settings = crate::settings::Settings::default();
        settings.browser_integration_enabled = true;
        assert!(owner.defer_root_close(RootCloseIntent::ApplicationQuit));
        assert!(matches!(
            owner.poll(&mut settings, &egui::Context::default()),
            Some(Event::Failed(_))
        ));
        assert!(settings.browser_integration_enabled);
        assert_eq!(
            owner.take_ready_close(),
            Some(RootCloseIntent::ApplicationQuit)
        );
        assert_eq!(owner.take_ready_close(), None);
    }
}

#[test]
fn browser818_registration_uninstaller_owns_only_matching_manifest_and_does_not_follow_links() {
    let script = include_str!("../../installer/mimageviewer.iss");
    for key in REGISTRY_KEYS {
        assert!(key.ends_with(mimageviewer_browser_host::protocol::NATIVE_HOST_NAME));
        assert!(script.contains(key));
    }
    assert!(script.contains("CompareText(Value, Manifest) = 0"));
    assert!(
        script.contains("if (FindRec.Attributes and $400) = 0 then RemoveBrowserHostTree(Child)")
    );
    let post = script
        .split("if CurUninstallStep = usPostUninstall then")
        .nth(1)
        .unwrap();
    assert!(
        post.find("RemoveBrowserRegistration;").unwrap()
            < post.find("MsgResult := MsgBox").unwrap()
    );
}

mod gpu_tests {
    use super::*;

    struct LifecycleBackend {
        entered: mpsc::Sender<&'static str>,
        proceed: Mutex<mpsc::Receiver<()>>,
        registration: Mutex<Selection>,
        picked: Selection,
    }
    impl Backend for LifecycleBackend {
        fn prepare(&self, _: Selection) -> Preparation {
            panic!("the lifecycle fixture already prepared its isolated environment")
        }
        fn perform(&self, mut prepared: Prepared, action: Action) -> Result<Completion, String> {
            self.entered.send("action").unwrap();
            self.proceed.lock().unwrap().recv().unwrap();
            let selection = match action {
                Action::Apply(selection) => selection,
                Action::Remove => Selection {
                    enabled: false,
                    ..prepared.selection.clone()
                },
                Action::ChooseFolder => self.picked.clone(),
                Action::OpenFolder(_) => panic!("not a registration change"),
            };
            *self.registration.lock().unwrap() = selection.clone();
            prepared.selection = selection.clone();
            prepared.registered = selection.enabled;
            Ok(Completion {
                prepared,
                selection: Some(selection),
            })
        }
        fn persist(&self, selection: &Selection) -> Result<(), String> {
            self.entered.send("persist").unwrap();
            self.proceed.lock().unwrap().recv().unwrap();
            crate::settings::save_browser_integration_worker(
                selection.enabled,
                &selection.output_dir,
            )
        }
    }

    fn close_input() -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        input
    }

    fn saved_selection() -> Selection {
        Selection::from_settings(
            &crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap(),
        )
    }

    fn registration_close_round_trip(completed_before_close: bool) {
        // Each row uses actual worker completions and the real two-key DB save;
        // registry effects are an injected value, never the user's registration.
        for operation in ["enable", "remove", "destination", "picker"] {
            let mut app = crate::app::tests::phase_c_support::setup_app();
            app.settings.minimize_to_tray_on_close = false;
            let initial = Selection {
                enabled: operation != "enable",
                output_dir: Some(app.tmp.path().join("old-images")),
            };
            let target = Selection {
                enabled: operation != "remove",
                output_dir: if matches!(operation, "destination" | "picker") {
                    Some(app.tmp.path().join("new-images"))
                } else {
                    initial.output_dir.clone()
                },
            };
            initial.apply(&mut app.settings);
            assert!(app.settings.save_checked());
            assert_eq!(saved_selection(), initial);
            let mut ready = prepared(environment(app.tmp.path()));
            ready.selection = initial.clone();
            ready.registered = initial.enabled;
            let (entered_tx, entered_rx) = mpsc::channel();
            let (proceed_tx, proceed_rx) = mpsc::channel();
            let registration = Arc::new(LifecycleBackend {
                entered: entered_tx,
                proceed: Mutex::new(proceed_rx),
                registration: Mutex::new(initial.clone()),
                picked: target.clone(),
            });
            app.browser_integration = service(ready, registration.clone());
            let ctx = egui::Context::default();
            let action = match operation {
                "remove" => Action::Remove,
                "picker" => Action::ChooseFolder,
                _ => Action::Apply(target.clone()),
            };
            assert!(app.browser_integration.submit(action, &ctx));
            assert_eq!(
                entered_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap(),
                "action"
            );
            if completed_before_close {
                proceed_tx.send(()).unwrap();
                await_worker(&mut app.browser_integration);
                assert_eq!(*registration.registration.lock().unwrap(), target);
                assert_eq!(Selection::from_settings(&app.settings), initial);
            }
            // Exercise both explicit Quit and ordinary non-tray Close.
            let intent = if operation == "remove" {
                app.shutdown_requested
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                RootCloseIntent::ApplicationQuit
            } else {
                RootCloseIntent::WindowClose
            };
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
            let (output, _) = backend.update_logic_only(&mut app.app, close_input());
            assert!(!backend.should_close());
            assert!(
                output.viewport_commands[&egui::ViewportId::ROOT]
                    .contains(&egui::ViewportCommand::CancelClose)
            );
            assert_eq!(app.browser_integration.close, Some(intent));
            assert!(
                !app.browser_integration
                    .submit(Action::Remove, &egui::Context::default())
            );
            if !completed_before_close {
                assert_eq!(Selection::from_settings(&app.settings), initial);
                proceed_tx.send(()).unwrap();
                await_worker(&mut app.browser_integration);
                let (output, _) = backend.update_logic_only(&mut app.app, Default::default());
                assert!(
                    !output
                        .viewport_commands
                        .get(&egui::ViewportId::ROOT)
                        .is_some_and(|commands| commands.contains(&egui::ViewportCommand::Close))
                );
            }
            assert_eq!(
                entered_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap(),
                "persist"
            );
            assert_eq!(Selection::from_settings(&app.settings), target);
            assert_eq!(
                saved_selection(),
                initial,
                "the save worker is deliberately held"
            );
            let (_, _) = backend.update_logic_only(&mut app.app, close_input());
            assert!(
                !backend.should_close(),
                "an additional Close cannot skip persistence"
            );
            proceed_tx.send(()).unwrap();
            await_worker(&mut app.browser_integration);
            let (output, _) = backend.update_logic_only(&mut app.app, Default::default());
            assert!(
                !backend.should_close(),
                "the resumed Close is the next delivery"
            );
            assert_eq!(
                output.viewport_commands[&egui::ViewportId::ROOT]
                    .iter()
                    .filter(|command| matches!(command, egui::ViewportCommand::Close))
                    .count(),
                1
            );
            assert_eq!(app.browser_integration.close, None);
            assert_eq!(saved_selection(), target);
            backend.update_logic_only(&mut app.app, close_input());
            assert!(backend.should_close());
            eframe::App::on_exit(&mut app.app, None);
            assert_eq!(
                saved_selection(),
                target,
                "exit must not overwrite the result with old settings"
            );
            assert_eq!(*registration.registration.lock().unwrap(), target);
        }
    }

    #[test]
    fn browser818_registration_completed_worker_unpolled_quit_saves_matching_database() {
        registration_close_round_trip(true);
    }

    #[test]
    fn browser818_registration_inflight_worker_close_waits_for_adoption_and_database() {
        registration_close_round_trip(false);
    }

    #[test]
    fn browser818_registration_exit_fence_adopts_unpolled_result_before_final_save() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.settings.browser_integration_enabled = false;
        assert!(app.settings.save_checked());
        let fake = Arc::new(FakeBackend::default());
        app.browser_integration = service(prepared(environment(app.tmp.path())), fake.clone());
        let target = Selection {
            enabled: true,
            output_dir: Some(app.tmp.path().join("images")),
        };
        assert!(
            app.browser_integration
                .submit(Action::Apply(target.clone()), &egui::Context::default())
        );
        await_worker(&mut app.browser_integration);
        assert!(!app.settings.browser_integration_enabled);
        eframe::App::on_exit(&mut app.app, None);
        assert_eq!(saved_selection(), target);
        assert!(
            fake.persisted.lock().unwrap().is_empty(),
            "the final full save owns persistence"
        );
    }

    #[test]
    fn browser818_registration_tray_hide_keeps_operation_without_a_quit_continuation() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.settings.minimize_to_tray_on_close = true;
        app.tray_controller = Some(crate::tray::TrayController::controller_for_test());
        let mut ready = prepared(environment(app.tmp.path()));
        ready.selection.enabled = true;
        app.settings.browser_integration_enabled = true;
        let mut completed = ready.clone();
        completed.selection.enabled = false;
        let target = completed.selection.clone();
        let fake = Arc::new(FakeBackend::default());
        app.browser_integration = service(ready.clone(), fake);
        let (tx, rx) = mpsc::channel();
        app.browser_integration.state = BrowserIntegrationState::Processing {
            prepared: ready,
            kind: JobKind::Action(Action::Remove),
            rx,
        };
        let ctx = egui::Context::default();
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        backend.update_logic_only(&mut app.app, close_input());
        assert!(!backend.should_close());
        assert!(!app.window_visible);
        assert_eq!(app.browser_integration.close, None);
        tx.send(Ok(Completion {
            prepared: completed,
            selection: Some(target.clone()),
        }))
        .unwrap();
        app.poll_browser_integration(&ctx);
        await_worker(&mut app.browser_integration);
        let mut output = ctx.run_pass(Default::default(), |ctx| app.poll_browser_integration(ctx));
        output.textures_delta.clear(); // assertions must not double-panic on drop
        assert!(
            !output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|viewport| viewport.commands.contains(&egui::ViewportCommand::Close))
        );
        assert_eq!(app.browser_integration.close, None);
        assert_eq!(Selection::from_settings(&app.settings), target);
    }
    #[test]
    fn browser818_registration_root_collects_completion_after_preferences_close() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let backend = Arc::new(FakeBackend::default());
        app.browser_integration = service(prepared(environment(app.tmp.path())), backend.clone());
        let ctx = egui::Context::default();
        app.open_preferences_page(
            crate::ui_dialogs::preferences::PreferencesPage::BrowserIntegration,
        );
        app.ensure_preferences_state_for_test();
        assert!(app.pref_state.is_some());
        let requested = Selection {
            enabled: true,
            output_dir: None,
        };
        let state = app.pref_state.as_mut().unwrap();
        requested.apply(&mut state.settings);
        state.browser_integration_action = Some(Action::Apply(requested.clone()));
        app.submit_browser_integration_preferences(&ctx);
        let state = app.pref_state.as_ref().unwrap();
        assert_eq!(
            Selection::from_settings(&state.settings),
            Selection::from_settings(&app.settings)
        );
        assert_eq!(state.browser_integration_view.selection, requested);
        assert_eq!(state.browser_integration_view.status, Status::Processing);
        app.pref_state = None;
        app.show_preferences = false;
        await_worker(&mut app.browser_integration);
        app.poll_browser_integration(&ctx);
        assert!(app.settings.browser_integration_enabled);
        assert!(app.pref_state.is_none());
        await_worker(&mut app.browser_integration);
        app.poll_browser_integration(&ctx);
        assert!(app.pref_state.is_none());
        assert_eq!(backend.persisted.lock().unwrap().len(), 1);
    }

    #[test]
    fn browser818_preferences_import_uses_worker_without_reverting_other_preferences() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let backend = Arc::new(FakeBackend::default());
        app.browser_integration = service(prepared(environment(app.tmp.path())), backend.clone());
        let ctx = egui::Context::default();
        app.open_preferences_page(
            crate::ui_dialogs::preferences::PreferencesPage::BrowserIntegration,
        );
        app.ensure_preferences_state_for_test();
        let state = app.pref_state.as_mut().unwrap();
        state.settings.browser_integration_enabled = true;
        state.settings.clipboard_capture_min_short_side_px = 420;
        app.apply_preferences_dialog(&ctx);
        assert_eq!(app.settings.clipboard_capture_min_short_side_px, 420);
        assert!(!app.settings.browser_integration_enabled);
        await_worker(&mut app.browser_integration);
        app.poll_browser_integration(&ctx);
        assert!(app.settings.browser_integration_enabled);
        assert_eq!(app.settings.clipboard_capture_min_short_side_px, 420);
        await_worker(&mut app.browser_integration);
        app.poll_browser_integration(&ctx);
        assert_eq!(backend.persisted.lock().unwrap().len(), 1);
    }
}
