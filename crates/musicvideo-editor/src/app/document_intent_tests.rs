//! Controlled channel delivery at the production handler/frame boundary. No native GUI.
use super::document_intent::IntentPhase;
use super::tests::{editor_input, loaded_project, pointer_input, rendered_editor_text};
use super::*;

fn app() -> MusicVideoLabApp {
    let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
    app.document
        .replace_with_loaded("original.json".into(), &app.project);
    app.project.display_gain_db = 3.0;
    app.document.note_edit();
    app
}

fn saving(app: &mut MusicVideoLabApp) -> mpsc::Sender<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    app.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
        checkpoint: app.document.begin_save("saved.json".into(), &app.project),
        rx,
    }));
    tx
}

fn frame(app: &mut MusicVideoLabApp, ctx: &egui::Context) -> egui::FullOutput {
    editor_input(app, ctx, vec![])
}

fn confirmed(app: &MusicVideoLabApp) -> bool {
    matches!(
        app.document_intent,
        DocumentIntentState::Pending {
            phase: IntentPhase::Confirm,
            ..
        }
    )
}

fn lyrics_app() -> (MusicVideoLabApp, LayerId, Vec<LyricLine>) {
    let mut app = app();
    let mut layer = musicvideo_core::new_lyrics_layer(&app.project);
    let id = layer.id;
    let lines = vec![
        LyricLine {
            text: "A".into(),
            time_secs: Some(1.0),
        },
        LyricLine {
            text: "B".into(),
            time_secs: Some(2.0),
        },
    ];
    if let LayerKind::Lyrics(lyrics) = &mut layer.kind {
        lyrics.lines = lines.clone();
    }
    app.project.layers.push(layer);
    app.editor_state.reset_for_project(&app.project);
    app.editor_state
        .select_layer(&app.project, app.project.layers.len() - 1);
    (app, id, lines)
}

fn pending_lyrics(
    app: &mut MusicVideoLabApp,
    ctx: &egui::Context,
) -> (egui::Id, egui::text::CCursorRange) {
    let pos = rendered_editor_text(app, ctx, "A\nB", true);
    editor_input(app, ctx, pointer_input(pos, true));
    editor_input(app, ctx, pointer_input(pos, false));
    let widget = ctx.memory(|memory| memory.focused()).unwrap();
    let selection =
        egui::text::CCursorRange::two(egui::text::CCursor::new(1), egui::text::CCursor::new(2));
    let mut native = egui::text_edit::TextEditState::load(ctx, widget).unwrap();
    native.cursor.set_char_range(Some(selection));
    native.store(ctx, widget);
    frame(app, ctx);
    editor_input(app, ctx, vec![egui::Event::Cut]);
    assert!(app.editor_state.has_pending_lyrics_edit());
    (widget, selection)
}

fn click(app: &mut MusicVideoLabApp, ctx: &egui::Context, label: &str) {
    let pos = rendered_editor_text(app, ctx, label, true);
    editor_input(app, ctx, pointer_input(pos, true));
    editor_input(app, ctx, pointer_input(pos, false));
}

#[test]
fn p4b4_lrc_ready_during_lyrics_confirmation_returns_after_intent_cancel() {
    for apply in [false, true] {
        let ctx = crate::project::test_context();
        let (mut app, id, original) = lyrics_app();
        let (tx, rx) = mpsc::channel();
        app.lrc_load = LrcLoadState::Loading {
            owner: LrcLoadOwner {
                session: app.document.id(),
                layer: id,
                target_lifetime: app.editor_state.layer_target_lifetime(id).unwrap(),
                baseline: original.clone(),
            },
            rx,
        };
        pending_lyrics(&mut app, &ctx);
        // Resolve an unselected layer, and avoid confusing its disabled inspector's
        // same-labelled cancel button with the active modal button in shape lookup.
        app.editor_state.select_layer(&app.project, 0);
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        let _ = ctx.run(input, |ctx| {
            app.process_editor_frame(ctx);
        });
        assert!(app.intent_pending());
        assert!(app.editor_state.has_pending_lyrics_edit());
        tx.send(Ok((
            "loaded.lrc".into(),
            parse_lrc(b"[00:03.00]loaded\n").unwrap(),
        )))
        .unwrap();
        frame(&mut app, &ctx);
        assert!(matches!(app.lrc_load, LrcLoadState::Ready { .. }));
        // Real pointer input must reach the lyrics decision even after a later modal
        // would otherwise claim the foreground. Do not dispatch the action directly.
        click(&mut app, &ctx, "変更を取り消す");
        assert!(
            !app.editor_state.has_pending_lyrics_edit(),
            "lyrics cancel click was blocked by the LRC modal"
        );
        assert!(confirmed(&app));
        assert!(matches!(app.lrc_load, LrcLoadState::Ready { .. }));
        assert_eq!(app.current_lyrics(id).unwrap(), original);
        click(&mut app, &ctx, "キャンセル");
        assert!(
            !app.intent_pending(),
            "intent cancel click was blocked by the LRC modal"
        );
        assert!(matches!(app.lrc_load, LrcLoadState::Ready { .. }));
        click(
            &mut app,
            &ctx,
            if apply {
                "LRC の内容へ置換（現在の歌詞変更を破棄）"
            } else {
                "現在の歌詞を保持"
            },
        );
        assert!(matches!(app.lrc_load, LrcLoadState::Idle));
        assert_eq!(
            app.current_lyrics(id).unwrap(),
            if apply {
                vec![LyricLine {
                    text: "loaded".into(),
                    time_secs: Some(3.0),
                }]
            } else {
                original
            }
        );
    }
}

fn executed(app: &MusicVideoLabApp, intent: &DocumentIntent, previous: DocumentSessionId) {
    match intent {
        DocumentIntent::Open(path) => {
            assert!(
                matches!(&app.project_load, ProjectLoadState::Loading(p) if p.queued.is_none())
            );
            assert_eq!(path, &PathBuf::from("next.json"));
            assert!(matches!(app.document_intent, DocumentIntentState::Idle));
        }
        DocumentIntent::New => {
            assert_ne!(app.document.id(), previous);
            assert!(app.document.path().is_none());
            assert_eq!(app.project, default_project(DEFAULT_DURATION_SECS));
        }
        DocumentIntent::Close => assert!(matches!(
            app.document_intent,
            DocumentIntentState::CloseApproved
        )),
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Save,
    Fail,
    SaveCancel,
    Discard,
    Cancel,
}

fn decision_case(intent: DocumentIntent, outcome: Outcome) {
    let ctx = crate::project::test_context();
    let mut app = app();
    let session = app.document.id();
    let original = app.project.clone();
    app.accept_document_intent(intent.clone());
    frame(&mut app, &ctx);
    assert!(confirmed(&app));
    match outcome {
        Outcome::Save | Outcome::Fail => {
            let tx = saving(&mut app);
            app.apply_chrome_actions(&ctx, vec![ChromeAction::SaveIntent]);
            tx.send(if matches!(outcome, Outcome::Save) {
                Ok(())
            } else {
                Err("write failed".into())
            })
            .unwrap();
            frame(&mut app, &ctx);
        }
        Outcome::SaveCancel => {
            let (tx, rx) = mpsc::channel();
            app.project_save_pending = Some(ProjectSavePending::Choosing { session, rx });
            app.apply_chrome_actions(&ctx, vec![ChromeAction::SaveIntent]);
            tx.send(None).unwrap();
            frame(&mut app, &ctx);
        }
        Outcome::Discard => {
            app.apply_chrome_actions(&ctx, vec![ChromeAction::DiscardIntent]);
        }
        Outcome::Cancel => {
            app.apply_chrome_actions(&ctx, vec![ChromeAction::CancelIntent]);
        }
    };
    if matches!(outcome, Outcome::Save | Outcome::Discard) {
        executed(&app, &intent, session);
    } else {
        assert!(matches!(app.document_intent, DocumentIntentState::Idle));
        assert!(matches!(app.project_load, ProjectLoadState::Idle));
        assert_eq!(app.document.id(), session);
        assert_eq!(app.project, original);
        assert!(app.document.is_dirty(&app.project));
    }
}

macro_rules! case {
    ($name:ident, $intent:expr, $outcome:ident) => {
        #[test]
        fn $name() {
            decision_case($intent, Outcome::$outcome);
        }
    };
}
case!(
    p4b4_open_save,
    DocumentIntent::Open("next.json".into()),
    Save
);
case!(
    p4b4_open_failure,
    DocumentIntent::Open("next.json".into()),
    Fail
);
case!(
    p4b4_open_save_cancel,
    DocumentIntent::Open("next.json".into()),
    SaveCancel
);
case!(
    p4b4_open_discard,
    DocumentIntent::Open("next.json".into()),
    Discard
);
case!(
    p4b4_open_cancel,
    DocumentIntent::Open("next.json".into()),
    Cancel
);
case!(p4b4_new_save, DocumentIntent::New, Save);
case!(p4b4_new_failure, DocumentIntent::New, Fail);
case!(p4b4_new_save_cancel, DocumentIntent::New, SaveCancel);
case!(p4b4_new_discard, DocumentIntent::New, Discard);
case!(p4b4_new_cancel, DocumentIntent::New, Cancel);
case!(p4b4_close_save, DocumentIntent::Close, Save);
case!(p4b4_close_failure, DocumentIntent::Close, Fail);
case!(p4b4_close_save_cancel, DocumentIntent::Close, SaveCancel);
case!(p4b4_close_discard, DocumentIntent::Close, Discard);
case!(p4b4_close_cancel, DocumentIntent::Close, Cancel);

#[test]
fn p4b4_clean_intents_execute_without_confirmation() {
    for intent in [
        DocumentIntent::Open("next.json".into()),
        DocumentIntent::New,
        DocumentIntent::Close,
    ] {
        let ctx = crate::project::test_context();
        let mut app = app();
        let checkpoint = app.document.begin_save("clean.json".into(), &app.project);
        app.document.complete_save(checkpoint);
        let session = app.document.id();
        app.accept_document_intent(intent.clone());
        frame(&mut app, &ctx);
        executed(&app, &intent, session);
    }
}

#[test]
fn p4b4_save_completion_then_same_frame_lrc_reconfirms_once() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let mut layer = musicvideo_core::new_lyrics_layer(&app.project);
    let id = layer.id;
    if let LayerKind::Lyrics(lyrics) = &mut layer.kind {
        lyrics.lines = vec![LyricLine {
            text: "old".into(),
            time_secs: Some(1.0),
        }];
    }
    app.project.layers.push(layer);
    app.editor_state.reset_for_project(&app.project);
    let (lrc_tx, rx) = mpsc::channel();
    app.lrc_load = LrcLoadState::Loading {
        owner: LrcLoadOwner {
            session: app.document.id(),
            layer: id,
            target_lifetime: app.editor_state.layer_target_lifetime(id).unwrap(),
            baseline: app.current_lyrics(id).unwrap(),
        },
        rx,
    };
    let save_tx = saving(&mut app);
    app.accept_document_intent(DocumentIntent::Close);
    frame(&mut app, &ctx);
    assert!(app.intent_pending());
    assert!(!confirmed(&app));
    save_tx.send(Ok(())).unwrap();
    lrc_tx
        .send(Ok((
            "loaded.lrc".into(),
            parse_lrc(b"[00:02.00]new\n").unwrap(),
        )))
        .unwrap();
    frame(&mut app, &ctx);
    assert_eq!(app.current_lyrics(id).unwrap()[0].text, "new");
    assert!(confirmed(&app));
    assert!(app.document.is_dirty(&app.project));
    // A second frame neither executes nor drops the still-confirmed intent.
    frame(&mut app, &ctx);
    assert!(confirmed(&app));
}

#[test]
fn p4b4_material_completion_during_save_reconfirms() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let tx = saving(&mut app);
    app.accept_document_intent(DocumentIntent::New);
    let image = Arc::new(ImageAsset::new(1, 1, vec![1, 2, 3, 255]).unwrap());
    let (image_tx, rx) = mpsc::channel();
    app.image_pending = Some(ImagePending {
        request: ImageLoadRequest {
            session: app.document.id(),
            target: ImageLoadTarget::AddLayer,
            target_lifetime: None,
            path: "new.png".into(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        queued: None,
    });
    image_tx
        .send(Ok((
            "new.png".into(),
            image.clone(),
            ResourceLease::new(image),
        )))
        .unwrap();
    app.poll_loads(&ctx);
    tx.send(Ok(())).unwrap();
    frame(&mut app, &ctx);
    assert!(confirmed(&app));
    assert!(app.document.is_dirty(&app.project));
}

#[test]
fn p4b4_received_queued_and_ready_project_results_are_retired_on_intent() {
    for ready in [false, true] {
        for load_first in [false, true] {
            let ctx = crate::project::test_context();
            let mut app = app();
            let session = app.document.id();
            let save_tx = saving(&mut app);
            let (load_tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            app.project_load = if ready {
                ProjectLoadState::Ready(loaded_project("intruder.json", 99.0))
            } else {
                ProjectLoadState::Loading(ProjectLoadPending {
                    started_at: app.document.edit_generation(),
                    cancel: cancel.clone(),
                    rx,
                    queued: Some("queued.json".into()),
                })
            };
            if load_first && !ready {
                load_tx
                    .send(Ok(loaded_project("intruder.json", 99.0)))
                    .unwrap();
            }
            app.accept_document_intent(DocumentIntent::Close);
            assert!(matches!(app.project_load, ProjectLoadState::Idle));
            if !ready {
                assert!(cancel.load(Ordering::Relaxed));
            }
            if !load_first && !ready {
                let _ = load_tx.send(Ok(loaded_project("intruder.json", 99.0)));
            }
            app.start_project_load("forbidden.json".into(), &ctx);
            assert!(matches!(app.project_load, ProjectLoadState::Idle));
            save_tx.send(Ok(())).unwrap();
            frame(&mut app, &ctx);
            assert_eq!(app.document.id(), session);
            assert_ne!(app.project.duration_secs, 99.0);
            assert!(matches!(
                app.document_intent,
                DocumentIntentState::CloseApproved
            ));
        }
    }
}

fn project_and_save_completion_order(load_first: bool) {
    let ctx = crate::project::test_context();
    let mut app = app();
    let session = app.document.id();
    let original = app.project.clone();
    let save_tx = saving(&mut app);
    let (load_tx, rx) = mpsc::channel();
    app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
        started_at: app.document.edit_generation(),
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        queued: None,
    });
    app.accept_document_intent(DocumentIntent::Close);
    if load_first {
        // A retired receiver may already have been dropped by cleanup. Sending an old
        // completion can then fail; neither a queued result nor that failure may be adopted.
        let _ = load_tx.send(Ok(loaded_project("intruder.json", 99.0)));
        frame(&mut app, &ctx);
        assert!(app.intent_pending());
        assert!(!confirmed(&app));
        assert_eq!(app.document.id(), session);
        assert_eq!(app.project, original);
        save_tx.send(Ok(())).unwrap();
        frame(&mut app, &ctx);
    } else {
        save_tx.send(Ok(())).unwrap();
        frame(&mut app, &ctx);
        assert!(matches!(
            app.document_intent,
            DocumentIntentState::CloseApproved
        ));
        assert_eq!(app.document.id(), session);
        // This completion is sent only AFTER the save has been consumed in a frame.
        let _ = load_tx.send(Ok(loaded_project("intruder.json", 99.0)));
        frame(&mut app, &ctx);
    }
    assert_eq!(app.document.id(), session);
    assert_eq!(app.project, original);
    assert!(matches!(app.project_load, ProjectLoadState::Idle));
    assert!(matches!(
        app.document_intent,
        DocumentIntentState::CloseApproved
    ));
}

#[test]
fn p4b4_project_completion_before_save_is_retired() {
    project_and_save_completion_order(true);
}

#[test]
fn p4b4_project_completion_after_save_is_retired() {
    project_and_save_completion_order(false);
}

#[test]
fn p4b4_close_replaces_intent_while_save_is_pending() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let tx = saving(&mut app);
    app.accept_document_intent(DocumentIntent::New);
    app.accept_document_intent(DocumentIntent::Open("ignored.json".into()));
    app.accept_document_intent(DocumentIntent::Close);
    frame(&mut app, &ctx);
    assert!(!confirmed(&app));
    tx.send(Ok(())).unwrap();
    frame(&mut app, &ctx);
    assert!(matches!(
        app.document_intent,
        DocumentIntentState::CloseApproved
    ));
}

#[test]
fn p4b4_document_switch_drops_pending_intent() {
    let ctx = crate::project::test_context();
    let mut app = app();
    app.accept_document_intent(DocumentIntent::Close);
    frame(&mut app, &ctx);
    app.document
        .replace_with_loaded("other.json".into(), &app.project);
    frame(&mut app, &ctx);
    assert!(matches!(app.document_intent, DocumentIntentState::Idle));
}

fn exporting(app: &mut MusicVideoLabApp) -> mpsc::Sender<Result<ExportCompletion, String>> {
    let (tx, rx) = mpsc::channel();
    app.export_state = ExportState::Running(ExportPending {
        kind: ExportKind::Mp4 {
            stage: Arc::new(AtomicUsize::new(0)),
            output_path: "test.mp4".into(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(AtomicUsize::new(0)),
        total_frames: 30,
        rx,
    });
    tx
}

#[test]
fn p4b4_export_close_waits_for_worker_terminal_then_dirty_decision() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let tx = exporting(&mut app);
    app.accept_document_intent(DocumentIntent::Close);
    app.apply_chrome_actions(&ctx, vec![ChromeAction::StopExportAndClose]);
    assert!(
        app.export_state
            .pending()
            .unwrap()
            .cancel
            .load(Ordering::Relaxed)
    );
    for _ in 0..2 {
        app.poll_export();
        frame(&mut app, &ctx);
        assert!(matches!(
            app.document_intent,
            DocumentIntentState::Pending {
                phase: IntentPhase::StoppingExport,
                ..
            }
        ));
    }
    tx.send(Ok(ExportCompletion::Mp4(Err(
        mimageviewer::video::mvexport::ExportError::Cancelled,
    ))))
    .unwrap();
    app.poll_export();
    frame(&mut app, &ctx);
    assert!(confirmed(&app));
}

#[test]
fn p4b4_export_close_refusal_keeps_worker_running() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let _tx = exporting(&mut app);
    app.accept_document_intent(DocumentIntent::Close);
    app.apply_chrome_actions(&ctx, vec![ChromeAction::CancelIntent]);
    assert!(matches!(app.document_intent, DocumentIntentState::Idle));
    assert!(
        !app.export_state
            .pending()
            .unwrap()
            .cancel
            .load(Ordering::Relaxed)
    );
}

#[test]
fn p4b4_close_request_cancelclose_and_approved_close_is_terminal() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let close_frame = |app: &mut MusicVideoLabApp| {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        ctx.run(input, |ctx| {
            app.process_editor_frame(ctx);
        })
    };
    app.accept_document_intent(DocumentIntent::New);
    let first = close_frame(&mut app);
    assert!(
        first.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
    assert!(matches!(
        app.document_intent,
        DocumentIntentState::Pending {
            intent: DocumentIntent::Close,
            ..
        }
    ));
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        app.apply_chrome_actions(ctx, vec![ChromeAction::DiscardIntent]);
    });
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::Close)
    );
    let approved = close_frame(&mut app);
    assert!(
        !approved.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
}

#[test]
fn p4b4_new_reuses_project_replacement_resource_boundary() {
    let ctx = crate::project::test_context();
    let mut app = super::tests::h3a_app();
    app.accept_document_intent(DocumentIntent::New);
    frame(&mut app, &ctx);
    app.apply_chrome_actions(&ctx, vec![ChromeAction::DiscardIntent]);
    assert!(app.audio.is_none());
    assert!(app.audio_player.is_none());
    assert!(app.waveform.is_none());
    assert!(app.editor_state.resource_manifest().audio().is_none());
    assert_eq!(app.editor_state.history_depths(), (0, 0));
    assert!(app.displayed_preview.is_none());
    assert!(app.document.path().is_none());
}

#[test]
fn p4b4_modal_blocks_toolbar_and_editor_input_while_saving() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let _tx = saving(&mut app);
    app.accept_document_intent(DocumentIntent::Close);
    app.apply_chrome_actions(
        &ctx,
        vec![
            ChromeAction::NewProject,
            ChromeAction::Seek(5.0),
            ChromeAction::CancelResources,
        ],
    );
    assert_eq!(app.position_secs, 0.0);
    let original = app.project.clone();
    frame(&mut app, &ctx);
    let out = frame(&mut app, &ctx);
    assert_eq!(app.project, original);
    assert!(out.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "プロジェクトの保存を待っています")));
    assert!(app.mp4_export_disabled_reason().is_some());
}

#[test]
fn p4b4_lyrics_cancel_frame_rejects_late_project_completion() {
    let ctx = crate::project::test_context();
    let (mut app, id, lines) = lyrics_app();
    let (widget, selection) = pending_lyrics(&mut app, &ctx);
    let (tx, rx) = mpsc::channel();
    app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
        started_at: app.document.edit_generation(),
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        queued: None,
    });
    app.accept_document_intent(DocumentIntent::Close);
    frame(&mut app, &ctx);
    assert!(!confirmed(&app));
    // Selected layer is irrelevant to the host's all-layer confirmation path.
    app.editor_state.select_layer(&app.project, 0);
    let pos = rendered_editor_text(&mut app, &ctx, "変更を取り消す", true);
    editor_input(&mut app, &ctx, pointer_input(pos, true));
    assert!(app.editor_state.has_pending_lyrics_edit());
    // Deliver the old worker result immediately before the pointer-release frame.
    // Lyrics cancellation is handled before poll_projects in this exact frame.
    let _ = tx.send(Ok(loaded_project("unwanted.json", 99.0)));
    editor_input(&mut app, &ctx, pointer_input(pos, false));
    assert!(!app.editor_state.has_pending_lyrics_edit());
    assert!(confirmed(&app));
    assert_eq!(app.current_lyrics(id).unwrap(), lines);
    assert_eq!(
        egui::text_edit::TextEditState::load(&ctx, widget)
            .unwrap()
            .cursor
            .char_range(),
        Some(selection)
    );
    assert!(matches!(app.project_load, ProjectLoadState::Idle));
}

#[test]
fn p4b4_save_action_and_chosen_new_path_use_the_normal_save_worker() {
    for new_document in [false, true] {
        let ctx = crate::project::test_context();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.mivmv.json");
        let mut app = app();
        if new_document {
            app.document = DocumentSession::default();
        } else {
            app.document.replace_with_loaded(path.clone(), &app.project);
            app.project.display_gain_db = 9.0;
            app.document.note_edit();
        }
        let expected = app.project.clone();
        app.accept_document_intent(DocumentIntent::Close);
        frame(&mut app, &ctx);
        if new_document {
            let (tx, rx) = mpsc::channel();
            app.project_save_pending = Some(ProjectSavePending::Choosing {
                session: app.document.id(),
                rx,
            });
            app.apply_chrome_actions(&ctx, vec![ChromeAction::SaveIntent]);
            tx.send(Some(path.clone())).unwrap();
            frame(&mut app, &ctx);
        } else {
            app.apply_chrome_actions(&ctx, vec![ChromeAction::SaveIntent]);
        }
        // Waiting is test-only. Production never waits; deliver the worker's exact result
        // through a controlled receiver to inspect the pre-completion state deterministically.
        let ProjectSavePending::Writing(pending) = app
            .project_save_pending
            .as_mut()
            .expect("normal save worker started")
        else {
            panic!("expected save worker")
        };
        let result = pending.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(result.is_ok());
        let (tx, rx) = mpsc::channel();
        pending.rx = rx;
        assert_eq!(load_project(&path).unwrap(), expected);
        assert!(app.intent_pending());
        tx.send(result).unwrap();
        frame(&mut app, &ctx);
        assert_eq!(app.document.path(), Some(path.as_path()));
        assert!(matches!(
            app.document_intent,
            DocumentIntentState::CloseApproved
        ));
    }
}

#[test]
fn p4b4_toolbar_new_enters_the_shared_confirmation() {
    let ctx = crate::project::test_context();
    let mut app = app();
    let previous = app.document.id();
    let pos = rendered_editor_text(&mut app, &ctx, "新規", true);
    editor_input(&mut app, &ctx, pointer_input(pos, true));
    editor_input(&mut app, &ctx, pointer_input(pos, false));
    assert!(confirmed(&app));
    assert!(matches!(
        app.document_intent,
        DocumentIntentState::Pending {
            intent: DocumentIntent::New,
            ..
        }
    ));
    assert_eq!(app.document.id(), previous);
}
