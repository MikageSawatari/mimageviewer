use super::tests::{editor_input, loaded_project, rendered_editor_text};
use super::*;
use crate::persistence::{Command, Event, Persistence};
use crate::startup::{OpenOnStartup, RuntimeAdmission, StartupArgs, parse_args};

fn app() -> MusicVideoLabApp {
    MusicVideoLabApp::new(Arc::new(Assets::default()))
}

fn fake_host(
    app: &mut MusicVideoLabApp,
    enabled: bool,
) -> (mpsc::Receiver<Command>, mpsc::Sender<Event>) {
    let (tx, commands) = mpsc::channel();
    let (events, rx) = mpsc::channel();
    app.host.persistence = Some(Persistence { tx, rx });
    app.host.recent = enabled.then(Vec::new);
    (commands, events)
}

fn added(commands: &mpsc::Receiver<Command>) -> Vec<PathBuf> {
    commands
        .try_iter()
        .filter_map(|cmd| match cmd {
            Command::Add(path) => Some(path),
            _ => None,
        })
        .collect()
}

#[test]
fn p4c_argument_error_is_rendered_with_empty_project() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::project::test_context();
    let mut app = app();
    let args = parse_args(
        ["--audio", "a", "--project", "b"].map(Into::into),
        temp.path(),
    );
    let error = args.error.clone().unwrap();
    app.configure_startup(
        args,
        RuntimeAdmission::acquire(|| Ok(None)).into(),
        None,
        None,
        &ctx,
    );
    assert_eq!(app.project, default_project(DEFAULT_DURATION_SECS));
    assert!(app.audio_pending.is_none());
    assert!(matches!(app.project_load, ProjectLoadState::Idle));
    assert_eq!(app.status, error);
    rendered_editor_text(&mut app, &ctx, &error, false);
}

#[test]
fn p4c_lease_failure_blocks_editing_and_guidance_is_rendered() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::project::test_context();
    let mut app = app();
    let runtime = RuntimeAdmission::acquire(|| Err(std::io::Error::other("lease test failure")));
    app.configure_startup(
        StartupArgs {
            open: Some(OpenOnStartup::Audio(temp.path().join("song.wav"))),
            ..Default::default()
        },
        runtime.into(),
        None,
        None,
        &ctx,
    );
    assert!(!app.editing_admitted());
    assert!(app.audio_pending.is_none());
    app.accept_document_intent(DocumentIntent::New);
    app.apply_chrome_actions(
        &ctx,
        vec![
            ChromeAction::NewProject,
            ChromeAction::OpenRecent(temp.path().join("project.json")),
            ChromeAction::Seek(5.0),
        ],
    );
    editor_input(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
    );
    assert_eq!(app.project, default_project(DEFAULT_DURATION_SECS));
    assert_eq!(app.position_secs, 0.0);
    assert!(!app.intent_pending());
    let error = app.status.clone();
    assert!(error.contains("mIV から音楽ビデオエディタを開き直してください。"));
    rendered_editor_text(&mut app, &ctx, &error, false);
    assert!(app.preview_pending.is_none());
}

#[test]
fn p4c_startup_project_and_audio_use_existing_load_routes_missing_is_visible() {
    let temp = tempfile::tempdir().unwrap();
    for audio in [false, true] {
        let ctx = crate::project::test_context();
        let mut app = app();
        let path = temp
            .path()
            .join(if audio { "missing.wav" } else { "missing.json" });
        let args = StartupArgs {
            open: Some(if audio {
                OpenOnStartup::Audio(path.clone())
            } else {
                OpenOnStartup::Project(path.clone())
            }),
            ..Default::default()
        };
        app.configure_startup(
            args,
            RuntimeAdmission::acquire(|| Ok(None)).into(),
            None,
            None,
            &ctx,
        );
        if audio {
            let pending = app.audio_pending.as_ref().expect("existing H1 audio route");
            assert_eq!(pending.request.path, path);
            let result = pending.rx.recv_timeout(Duration::from_secs(15)).unwrap();
            let (tx, rx) = mpsc::channel();
            app.audio_pending.as_mut().unwrap().rx = rx;
            tx.send(result).unwrap();
            app.poll_loads(&ctx);
        } else {
            let ProjectLoadState::Loading(pending) = &app.project_load else {
                panic!("existing project worker");
            };
            let result = pending.rx.recv_timeout(Duration::from_secs(15)).unwrap();
            let (tx, rx) = mpsc::channel();
            let ProjectLoadState::Loading(pending) = &mut app.project_load else {
                unreachable!()
            };
            pending.rx = rx;
            tx.send(result).unwrap();
            app.poll_projects(&ctx);
        }
        assert!(app.project.audio.is_none());
        assert!(app.document.path().is_none());
        assert!(app.status.contains("missing"), "{}", app.status);
        let notice = app.host.notices.last().unwrap().clone();
        rendered_editor_text(&mut app, &ctx, &notice, false);
    }
}

#[test]
fn p4c_recent_adoption_only_excludes_start_cancel_failure_and_discard() {
    let ctx = crate::project::test_context();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("accepted.json");
    let mut app = app();
    let (commands, _events) = fake_host(&mut app, true);
    // All results go through the actual project completion/choice handlers.
    for outcome in ["cancel", "failure", "discard", "accept", "apply"] {
        let (tx, rx) = mpsc::channel();
        app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
            started_at: app.document.edit_generation(),
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            queued: None,
        });
        assert!(added(&commands).is_empty());
        if outcome == "cancel" {
            let mut loaded = loaded_project(path.to_str().unwrap(), 2.0);
            loaded.path = path.clone();
            tx.send(Ok(loaded)).unwrap();
            app.accept_document_intent(DocumentIntent::New);
            assert!(matches!(app.project_load, ProjectLoadState::Idle));
            app.poll_projects(&ctx);
            app.document_intent = DocumentIntentState::Idle;
        } else if outcome == "failure" {
            tx.send(Err("failure".into())).unwrap();
            app.poll_projects(&ctx);
        } else {
            if matches!(outcome, "discard" | "apply") {
                app.document.note_edit();
            }
            tx.send(Ok(loaded_project(path.to_str().unwrap(), 2.0)))
                .unwrap();
            app.poll_projects(&ctx);
            if outcome == "discard" {
                app.apply_chrome_actions(&ctx, vec![ChromeAction::DiscardProject]);
            }
            if outcome == "apply" {
                app.apply_chrome_actions(&ctx, vec![ChromeAction::ApplyProject]);
            }
        }
        assert_eq!(
            added(&commands),
            if matches!(outcome, "accept" | "apply") {
                vec![path.clone()]
            } else {
                vec![]
            },
            "{outcome}"
        );
    }
    app.accept_document_intent(DocumentIntent::New);
    app.finish_document_intent_frame(&ctx);
    assert!(added(&commands).is_empty(), "new is not a recent project");
}

#[test]
fn p4c_recent_open_uses_dirty_intent_cancel_and_remove() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::project::test_context();
    let mut app = app();
    let (commands, _events) = fake_host(&mut app, true);
    app.project.display_gain_db = 3.0;
    app.document.note_edit();
    let missing = temp.path().join("missing.json");
    app.apply_chrome_actions(&ctx, vec![ChromeAction::OpenRecent(missing.clone())]);
    assert!(
        matches!(app.document_intent, DocumentIntentState::Pending { intent: DocumentIntent::Open(ref p), .. } if p == &missing)
    );
    app.finish_document_intent_frame(&ctx);
    assert!(matches!(app.project_load, ProjectLoadState::Idle));
    assert!(added(&commands).is_empty());
    app.apply_chrome_actions(&ctx, vec![ChromeAction::CancelIntent]);
    assert!(!app.intent_pending());
    app.apply_chrome_actions(&ctx, vec![ChromeAction::RemoveRecent(missing.clone())]);
    assert!(
        commands
            .try_iter()
            .any(|cmd| matches!(cmd, Command::Remove(path) if path == missing))
    );
    assert_eq!(app.project.display_gain_db, 3.0);
}

#[test]
fn p4c_save_success_remembers_and_recent_write_failure_does_not_undo_save() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::project::test_context();
    let mut app = app();
    std::fs::write(temp.path().join("musicvideo"), b"block recent writes").unwrap();
    let (service, guard) = Persistence::start(Some(temp.path().into())).unwrap();
    app.host.persistence = Some(service);
    app.host.recent = Some(vec![]);
    let path = temp.path().join("saved.json");
    app.project.display_gain_db = 3.0;
    app.document.note_edit();
    app.start_project_save(path.clone(), &ctx);
    let Some(ProjectSavePending::Writing(pending)) = &app.project_save_pending else {
        panic!("save worker");
    };
    let result = pending.rx.recv_timeout(Duration::from_secs(15)).unwrap();
    assert!(result.is_ok());
    let (tx, rx) = mpsc::channel();
    let Some(ProjectSavePending::Writing(pending)) = &mut app.project_save_pending else {
        unreachable!()
    };
    pending.rx = rx;
    tx.send(result).unwrap();
    app.poll_projects(&ctx);
    assert_eq!(app.document.path(), Some(path.as_path()));
    assert!(!app.document.is_dirty(&app.project));
    assert!(app.status.contains("保存しました"));
    drop(guard);
    app.poll_host();
    assert!(app.host.recent.as_ref().unwrap().contains(&path));
    assert!(
        app.host
            .notices
            .iter()
            .any(|msg| msg.contains("最近使ったプロジェクトを保存できません"))
    );
    assert!(!app.document.is_dirty(&app.project));
    assert_eq!(load_project(&path).unwrap().display_gain_db, 3.0);
    assert!(
        std::fs::read_to_string(temp.path().join("logs/musicvideo.log"))
            .unwrap()
            .contains("最近使ったプロジェクトを保存できません")
    );
}

#[test]
fn p4c_failed_save_and_disabled_recent_do_not_remember() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::project::test_context();
    let mut app = app();
    let (commands, _events) = fake_host(&mut app, true);
    let (tx, rx) = mpsc::channel();
    app.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
        checkpoint: app
            .document
            .begin_save(temp.path().join("failed.json"), &app.project),
        rx,
    }));
    tx.send(Err("save failure".into())).unwrap();
    app.poll_projects(&ctx);
    assert!(added(&commands).is_empty());
    app.host.recent = None;
    app.apply_loaded_project(loaded_project(
        temp.path().join("loaded.json").to_str().unwrap(),
        2.0,
    ));
    assert!(added(&commands).is_empty());
}

#[test]
fn p4c_worker_disconnect_is_notified_and_status_logging_is_queued() {
    let mut app = app();
    let (commands, events) = fake_host(&mut app, true);
    app.status = "queued diagnostic".into();
    app.log_status();
    app.log_status();
    assert_eq!(
        commands
            .try_iter()
            .filter(|cmd| matches!(cmd, Command::Log(text) if text == "queued diagnostic"))
            .count(),
        1
    );
    drop(events);
    app.poll_host();
    assert!(app.host.persistence.is_none());
    assert!(
        app.host
            .notices
            .iter()
            .any(|text| text.contains("ワーカーが終了"))
    );
}
