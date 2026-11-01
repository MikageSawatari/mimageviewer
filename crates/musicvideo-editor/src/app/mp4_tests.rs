//! Headless tests use the real admission, input frame and terminal-result paths.
use super::*;
use crate::mp4::{ExportSettings, snapshot};
use mimageviewer::video::mvexport::EncoderPreference;

fn ready_app() -> MusicVideoLabApp {
    super::tests::h3a_app()
}

fn pending(app: &mut MusicVideoLabApp) -> mpsc::Sender<Result<ExportCompletion, String>> {
    let (tx, rx) = mpsc::channel();
    app.export_state = ExportState::Running(ExportPending {
        kind: ExportKind::Mp4 {
            stage: Arc::new(AtomicUsize::new(3)),
            output_path: "output.mp4".into(),
        },
        progress: Arc::new(AtomicUsize::new(30)),
        total_frames: 30,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    });
    tx
}

#[test]
fn p4b3_snapshot_shares_pcm_all_project_images_fonts_and_fixes_settings() {
    let mut app = ready_app();
    let mut originals = vec![];
    for name in ["used", "retained-unused"] {
        let id = ImageId::new(name);
        originals.push((
            id.clone(),
            app.assets.insert_image(
                id.clone(),
                ImageAsset::new(1, 1, vec![10, 20, 30, 255]).unwrap(),
            ),
        ));
        app.project.images.push(ImageRef {
            id,
            path: PathBuf::from(name),
        });
    }
    let audio = app.audio.clone().unwrap();
    let revision = app
        .editor_state
        .resource_manifest()
        .audio()
        .unwrap()
        .revision();
    let settings = ExportSettings {
        encoder: mimageviewer::video::mvexport::EncoderPreference::Qsv,
        video_bitrate_bps: 9_000_000,
        compute_frame_hash: true,
        compute_per_frame_hashes: false,
    };
    let request = snapshot(
        &app.project,
        &app.assets,
        audio.clone(),
        revision,
        "snapshot.mp4".into(),
        settings,
        true,
    );
    assert!(Arc::ptr_eq(&audio, &request.audio));
    assert!(Arc::ptr_eq(&app.assets.fonts, &request.assets.fonts));
    assert_eq!(request.pcm_id, revision);
    assert_eq!(request.source_duration_secs, 0.5);
    assert_eq!(request.encoder, settings.encoder);
    assert_eq!(request.video_bitrate_bps, settings.video_bitrate_bps);
    assert!(request.replace_existing && request.compute_frame_hash);
    app.project.duration_secs = 12.0;
    for (id, original) in originals {
        app.assets.insert_image(
            id.clone(),
            ImageAsset::new(1, 1, vec![99, 98, 97, 255]).unwrap(),
        );
        assert!(Arc::ptr_eq(&request.assets.image(&id).unwrap(), &original));
        assert_eq!(request.assets.image(&id).unwrap().rgba, [10, 20, 30, 255]);
    }
    assert_eq!(request.project.duration_secs, 0.5);
}

#[test]
fn p4b3_admission_blocks_each_loading_saving_unready_and_missing_audio_state() {
    let mut app = ready_app();
    assert!(app.mp4_export_disabled_reason().is_none());
    let ctx = crate::project::test_context();
    for case in 0..9 {
        let mut app = ready_app();
        match case {
            0 => {
                let (_tx, rx) = mpsc::channel();
                app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
                    started_at: app.document.edit_generation(),
                    cancel: Arc::new(AtomicBool::new(false)),
                    rx,
                    queued: None,
                });
            }
            1 => {
                let (_tx, rx) = mpsc::channel();
                app.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
                    checkpoint: app
                        .document
                        .begin_save(PathBuf::from("project.json"), &app.project),
                    rx,
                }));
            }
            2 => {
                let (_tx, rx) = mpsc::channel();
                app.lrc_save_pending = Some(LrcSavePending { rx });
            }
            3 => {
                let (_tx, rx) = mpsc::channel();
                app.audio_pending = Some(AudioPending {
                    request: AudioLoadRequest {
                        session: app.document.id(),
                        expected: None,
                        path: "audio.wav".into(),
                    },
                    cancel: Arc::new(AtomicBool::new(false)),
                    rx,
                    queued: None,
                });
            }
            4 => {
                let (_tx, rx) = mpsc::channel();
                app.image_pending = Some(ImagePending {
                    request: ImageLoadRequest {
                        session: app.document.id(),
                        target: ImageLoadTarget::AddLayer,
                        target_lifetime: None,
                        path: "image.png".into(),
                    },
                    cancel: Arc::new(AtomicBool::new(false)),
                    rx,
                    queued: None,
                });
            }
            5 => {
                app.runtime_resources = RuntimeResourceState::Restoring {
                    manifest: app.editor_state.resource_manifest().clone(),
                    generation: 1,
                }
            }
            6 => app.project.images.push(ImageRef {
                id: ImageId::new("missing"),
                path: "missing.png".into(),
            }),
            7 => app.audio = None,
            _ => {
                app.runtime_resources = RuntimeResourceState::Unresolved {
                    manifest: app.editor_state.resource_manifest().clone(),
                    error: "unready".into(),
                };
            }
        }
        assert!(app.mp4_export_disabled_reason().is_some(), "case {case}");
        app.begin_mp4_export("unused.mp4".into(), ExportSettings::default(), false, &ctx);
        assert!(
            app.export_state.pending().is_none(),
            "admission case {case}"
        );
    }
    // A Project result waiting for admission is also excluded.
    app.project_load = ProjectLoadState::Ready(LoadedProject {
        path: "project.json".into(),
        project: app.project.clone(),
        audio: None,
        waveform: None,
        images: vec![],
        resources: ResourceManifest::default(),
        image_errors: HashMap::new(),
        audio_error: None,
    });
    assert!(app.mp4_export_disabled_reason().is_some());
}

#[test]
fn p4b3_start_pauses_audio_cancels_preview_and_suppresses_new_workers() {
    let ctx = crate::project::test_context();
    let mut app = ready_app();
    let directory = tempfile::tempdir().unwrap();
    app.project
        .modulators
        .push(musicvideo_core::new_band_energy_modulator(&app.project));
    let audio = app.audio.clone().unwrap();
    app.install_audio(Some(audio), None);
    app.audio_player.as_ref().unwrap().wait_applied();
    app.audio_player.as_mut().unwrap().toggle();
    assert!(app.audio_player.as_ref().unwrap().wait_applied().playing);
    let (preview_tx, preview_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.preview_pending = Some(PreviewPending {
        cancel: cancel.clone(),
        rx: preview_rx,
    });
    app.begin_mp4_export(
        directory.path().join("bounded-test.mp4"),
        ExportSettings::default(),
        false,
        &ctx,
    );
    assert!(app.mp4_export_running());
    assert!(!app.audio_player.as_ref().unwrap().wait_applied().playing);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(
        app.preview_pending.is_some(),
        "UI must retain the receiver without joining"
    );
    drop(preview_tx);
    app.poll_preview(&ctx);
    assert!(app.preview_pending.is_none());
    app.start_preview_if_needed(&ctx);
    assert!(app.preview_pending.is_none());
    app.start_calibration_if_needed(&ctx);
    assert!(app.calibration_pending.is_none());
    // Cancel through production action; Drop also cancels. No UI launched.
    app.apply_chrome_actions(&ctx, vec![ChromeAction::CancelExport]);
    assert!(
        app.export_state
            .pending()
            .unwrap()
            .cancel
            .load(Ordering::Relaxed)
    );
    let terminal = app
        .export_state
        .take_worker()
        .unwrap()
        .rx
        .recv_timeout(Duration::from_secs(30));
    assert!(terminal.is_ok());
}

#[test]
fn p4b3_modal_blocks_host_actions_and_editor_undo_input() {
    let mut app = ready_app();
    let ctx = crate::project::test_context();
    let checkpoint = app.editor_state.begin_external_edit(&app.project);
    app.project.display_gain_db = 6.0;
    app.editor_state
        .finish_external_edit(&app.project, checkpoint);
    let expected = app.project.clone();
    let tx = pending(&mut app);
    app.apply_chrome_actions(
        &ctx,
        vec![
            ChromeAction::OpenProject,
            ChromeAction::OpenAudio,
            ChromeAction::SaveProject,
            ChromeAction::SaveProjectAs,
            ChromeAction::OpenBackground,
            ChromeAction::Export,
            ChromeAction::ExportMp4,
            ChromeAction::TogglePlayback,
            ChromeAction::Seek(0.3),
            ChromeAction::CancelResources,
        ],
    );
    let _ = crate::project::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        },
        |ctx| {
            app.process_editor_frame(ctx);
        },
    ));
    assert_eq!(app.project, expected);
    assert_eq!(app.position_secs, 0.0);
    assert!(!app.export_state.setup_open());
    assert!(app.project_save_pending.is_none() && !app.project_load.is_loading());
    assert!(app.audio_pending.is_none() && app.image_pending.is_none());
    assert!(app.export_state.pending().is_some());
    drop(tx);
}

#[test]
fn p4b3_choosing_is_modal_and_cancellation_returns_to_idle_without_export() {
    let mut app = ready_app();
    let ctx = crate::project::test_context();
    let (tx, rx) = mpsc::channel();
    app.export_state = ExportState::Choosing {
        settings: ExportSettings::default(),
        rx,
    };
    let checkpoint = app.editor_state.begin_external_edit(&app.project);
    app.project.display_gain_db = 6.0;
    app.editor_state
        .finish_external_edit(&app.project, checkpoint);
    let expected = app.project.clone();
    app.apply_chrome_actions(
        &ctx,
        vec![ChromeAction::TogglePlayback, ChromeAction::Seek(0.3)],
    );
    let _ = crate::project::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        },
        |ctx| {
            app.process_editor_frame(ctx);
        },
    ));
    assert_eq!(app.project, expected);
    assert_eq!(app.position_secs, 0.0);
    assert!(app.export_state.setup_open());
    tx.send(None).unwrap();
    app.poll_mp4_choice(&ctx);
    assert!(matches!(app.export_state, ExportState::Idle));
    assert!(app.export_state.pending().is_none());
}

#[test]
fn p4b3_final_frame_and_cancel_request_wait_for_worker_terminal_result() {
    let mut app = ready_app();
    let ctx = crate::project::test_context();
    let tx = pending(&mut app);
    app.poll_export();
    assert!(
        app.mp4_export_running(),
        "100% frames is not worker completion"
    );
    app.apply_chrome_actions(&ctx, vec![ChromeAction::CancelExport]);
    app.poll_export();
    assert!(app.mp4_export_running(), "cancel request is not completion");
    tx.send(Ok(ExportCompletion::Mp4(Err(
        mimageviewer::video::mvexport::ExportError::Cancelled,
    ))))
    .unwrap();
    app.poll_export();
    assert!(!app.mp4_export_running());
    assert!(app.status.contains("中止しました"));
    let tx = pending(&mut app);
    tx.send(Ok(ExportCompletion::Mp4(Err(
        mimageviewer::video::mvexport::ExportError::Io("publish failed".into()),
    ))))
    .unwrap();
    app.poll_export();
    assert!(!app.mp4_export_running());
    assert!(app.status.contains("publish failed"));
}

#[test]
fn p4b3_modal_draws_all_stages_and_exposes_only_cancel_action() {
    let mut app = ready_app();
    let ctx = crate::project::test_context();
    let tx = pending(&mut app);
    for _ in 0..2 {
        let _ =
            crate::project::headless_paint_output(ctx.run_ui(egui::RawInput::default(), |ctx| {
                app.draw_mp4_dialogs(ctx, &mut vec![])
            }));
    }
    for (index, label) in [
        "反応範囲の計算",
        "音声の符号化",
        "フレームの書き出し",
        "仕上げ",
    ]
    .into_iter()
    .enumerate()
    {
        if let ExportKind::Mp4 { stage, .. } = &app.export_state.pending().unwrap().kind {
            stage.store(index, Ordering::Relaxed);
        }
        let output =
            crate::project::headless_paint_output(ctx.run_ui(egui::RawInput::default(), |ctx| {
                let mut actions = vec![];
                app.draw_mp4_dialogs(ctx, &mut actions);
                assert!(actions.is_empty());
            }));
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(text.galley.text())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(label), "stage {index}: {text}");
        assert!(text.contains("30 / 30 フレーム") && text.contains("中止"));
        assert!(!text.contains("保存先を選ぶ"));
    }
    drop(tx);
}

#[test]
fn output_fps_30_snapshot_exports_mp4_at_selected_rate() {
    let mut app = ready_app();
    app.project.output.width = 160;
    app.project.output.height = 90;
    app.project.output.fps = 30.0;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fps30.mp4");
    let request = crate::mp4::snapshot(
        &app.project,
        &app.assets,
        app.audio.clone().unwrap(),
        app.editor_state
            .resource_manifest()
            .audio()
            .unwrap()
            .revision(),
        path.clone(),
        ExportSettings {
            encoder: EncoderPreference::OpenH264,
            ..Default::default()
        },
        false,
    );
    assert_eq!(request.project.output.fps, 30.0);
    // The worker owns a snapshot even if the live document changes later.
    app.project.output.fps = 60.0;
    let summary = crate::mp4::export_snapshot(request, &|_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(summary.frames, 15);
    let verification = mimageviewer::video::mvexport::verify_exported_mp4(&path).unwrap();
    assert_eq!(verification.video.fps, 30.0);
    assert_eq!(verification.video.frames, 15);
}
