use super::tests::{
    mask_detect_fixture, mask_detect_history, mask_detect_install_local, mask_detect_output,
    mask_detect_queue,
};
use super::*;
use crate::ai::detector_catalog::DetectorReadError;
use crate::ai::runtime::ExternalModelKey;

fn entry(name: &str) -> DetectorEntry {
    DetectorEntry {
        name: name.into(),
        result: Ok(DetectorInfo {
            key: ExternalModelKey {
                path: PathBuf::from(name),
                size: 10,
                modified: std::time::UNIX_EPOCH,
            },
            name: name.into(),
            format: DetectorFormat::UltralyticsSegment,
            names: vec!["first".into(), "second".into()],
            thresholds: vec![0.15, 0.3],
            input_size: 640,
            shape: None,
            license: Some("model license".into()),
        }),
    }
}

fn patch() -> MaskPatch {
    MaskPatch {
        x: 1,
        y: 0,
        w: 2,
        h: 2,
        bits: vec![true, false, false, true],
    }
}

fn assert_mask_detect_feedback_only_on_viewer(app: &mut App, kind: crate::feedback::FeedbackKind) {
    assert_eq!(app.fs_feedback_toast_surface, Some(ActionSurface::Viewer));
    assert_eq!(app.feedback_toasts_for_test.last().unwrap().1, kind);
    let message = app.fs_feedback_toast.as_ref().unwrap().0.clone();
    fn contains_message(shape: &egui::Shape, message: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text() == message,
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| contains_message(s, message)),
            _ => false,
        }
    }
    for surface in [ActionSurface::MainWindow, ActionSurface::Viewer] {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        for _ in 0..2 {
            // Verify routing independently of font initialization and wall time.
            app.fs_feedback_toast.as_mut().unwrap().1 = std::time::Instant::now();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    app.draw_feedback_toast(ui, rect, &ctx, surface);
                },
            );
            let painted = output
                .shapes
                .iter()
                .any(|s| contains_message(&s.shape, &message));
            assert_eq!(
                painted,
                surface == ActionSurface::Viewer,
                "{surface:?}: {message}"
            );
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn mask_detect_results_empty_and_stale_feedback_only_paint_on_originating_viewer_surface() {
    let _input = crate::key_input::lock_test_input();
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::DetachedWindow,
    ] {
        for tool in [
            MaskDetectTool::Erase,
            MaskDetectTool::Conceal,
            MaskDetectTool::LocalAdjust,
        ] {
            for outcome in 0..3 {
                let mut app = mask_detect_fixture(tool);
                app.viewer_presentation = presentation;
                let request = app.capture_mask_detect_request(tool).unwrap();
                let output = if outcome == 1 {
                    MaskDetectOutput::default()
                } else {
                    mask_detect_output()
                };
                if outcome == 2 {
                    app.items_generation = app.items_generation.wrapping_add(1);
                }
                mask_detect_queue(&mut app, request, output);
                app.poll_mask_detect(&egui::Context::default());
                let message = &app.fs_feedback_toast.as_ref().unwrap().0;
                match outcome {
                    0 => assert!(message.starts_with("1 か所を追加しました")),
                    1 => assert_eq!(message, "見つかりませんでした"),
                    _ => assert_eq!(message, "画像が変わったため中止しました"),
                }
                assert_mask_detect_feedback_only_on_viewer(
                    &mut app,
                    crate::feedback::FeedbackKind::Info,
                );
            }
        }
    }
}

#[test]
fn mask_detect_errors_and_folder_warning_only_paint_on_viewer_surface() {
    let _input = crate::key_input::lock_test_input();
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::DetachedWindow,
    ] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        app.viewer_presentation = presentation;
        for error in [
            MaskDetectError::MissingModel,
            MaskDetectError::Runtime("test runtime".into()),
            MaskDetectError::Detect(DetectError::ModelMissing(PathBuf::from("missing.onnx"))),
            MaskDetectError::Detect(DetectError::LoadFailed(crate::ai::AiError::Ort(
                "test load".into(),
            ))),
            MaskDetectError::Detect(DetectError::InferenceFailed(crate::ai::AiError::Ort(
                "test inference".into(),
            ))),
            MaskDetectError::Detect(DetectError::OutputShape("test shape".into())),
            MaskDetectError::Detect(DetectError::InvalidInput("test image".into())),
            MaskDetectError::Detect(DetectError::DetectorChanged),
        ] {
            app.feedback_toasts_for_test.clear();
            let request = app
                .capture_mask_detect_request(MaskDetectTool::Erase)
                .unwrap();
            let (tx, rx) = mpsc::channel();
            assert!(tx.send(Err(error)).is_ok());
            app.mask_detect_pending = Some(Box::new(MaskDetectPending {
                request,
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
            }));
            app.poll_mask_detect(&egui::Context::default());
            assert_mask_detect_feedback_only_on_viewer(
                &mut app,
                crate::feedback::FeedbackKind::Error,
            );
        }
        let request = app
            .capture_mask_detect_request(MaskDetectTool::Erase)
            .unwrap();
        let (tx, rx) = mpsc::channel();
        drop(tx);
        app.mask_detect_pending = Some(Box::new(MaskDetectPending {
            request,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        }));
        app.poll_mask_detect(&egui::Context::default());
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "検出処理が停止しました"
        );
        assert_mask_detect_feedback_only_on_viewer(&mut app, crate::feedback::FeedbackKind::Error);
        app.erase_mask_size = [7, 8];
        assert!(
            app.capture_mask_detect_request(MaskDetectTool::Erase)
                .is_none()
        );
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "画像とマスクの大きさが一致しません"
        );
        assert_mask_detect_feedback_only_on_viewer(&mut app, crate::feedback::FeedbackKind::Error);
        // Thread-spawn failures share this publisher; do not exhaust threads or
        // launch Explorer just to induce the rare failure in a regression test.
        app.show_mask_detect_error("検出器フォルダを開けません: test spawn".into());
        assert_mask_detect_feedback_only_on_viewer(&mut app, crate::feedback::FeedbackKind::Error);
        app.external_open_failure_tx
            .send(crate::external_links::ExternalOpenFailure::DetectorFolder)
            .unwrap();
        app.poll_external_open_failures();
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "検出器フォルダを開けませんでした"
        );
        assert_mask_detect_feedback_only_on_viewer(
            &mut app,
            crate::feedback::FeedbackKind::Warning,
        );
    }
}

#[test]
fn mask_detect_selected_byo_blocks_start_until_catalog_ready_and_is_captured() {
    let _input = crate::key_input::lock_test_input();
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    app.settings.mask_detect.detectors.insert(
        "saved".into(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            ..Default::default()
        },
    );
    *app.mask_detect_catalog = DetectorCatalogState::Idle;
    assert!(!app.mask_detect_selection_valid());
    assert!(
        app.capture_mask_detect_request(MaskDetectTool::Erase)
            .is_none()
    );
    let (tx, rx) = mpsc::channel();
    *app.mask_detect_catalog = DetectorCatalogState::Loading {
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    };
    app.mask_detect_dialog = Some(MaskDetectTool::Erase);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let frame = |app: &mut App| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1093.0, 614.0),
                )),
                ..Default::default()
            },
            |ui| app.show_mask_detect_dialog(ui.ctx()),
        )
        .drop_without_applying_deltas();
    };
    for _ in 0..3 {
        frame(&mut app);
    }
    assert_eq!(
        ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("mask_detect_start_enabled"))),
        Some(false)
    );
    assert!(!app.mask_detect_selection_valid());
    assert!(
        app.capture_mask_detect_request(MaskDetectTool::Erase)
            .is_none()
    );
    tx.send(Ok(vec![entry("saved")])).unwrap();
    frame(&mut app);
    assert_eq!(
        ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("mask_detect_start_enabled"))),
        Some(true)
    );
    let request = app
        .capture_mask_detect_request(MaskDetectTool::Erase)
        .unwrap();
    assert!(request.options.face_selected);
    assert_eq!(request.detectors.len(), 1);
    assert_eq!(request.detectors[0].info.name, "saved");
    assert_eq!(
        request.detectors[0].params.selected_classes,
        vec![true, true]
    );
}

#[test]
fn mask_detect_ready_unavailable_byo_requires_face_or_another_executable_detector() {
    for face in [false, true] {
        for state in 0..4 {
            let mut app = mask_detect_fixture(MaskDetectTool::Erase);
            app.settings.mask_detect.face_selected = face;
            app.settings.mask_detect.detectors.insert(
                "saved".into(),
                crate::settings::ExternalDetectorSettings {
                    selected: true,
                    classes: (state == 2).then(Vec::new),
                    ..Default::default()
                },
            );
            *app.mask_detect_catalog = DetectorCatalogState::Ready(match state {
                0 => Ok(Vec::new()), // temporarily absent
                1 => Ok(vec![DetectorEntry {
                    name: "saved".into(),
                    result: Err(DetectorReadError::OutputShape),
                }]),
                2 => Ok(vec![entry("saved")]), // no selected classes
                _ => Err("catalog read failed".into()),
            });
            assert_eq!(
                app.mask_detect_selection_valid(),
                face,
                "face={face}, state={state}"
            );
            let request = app.capture_mask_detect_request(MaskDetectTool::Erase);
            assert_eq!(request.is_some(), face);
            if let Some(request) = request {
                assert!(request.options.face_selected);
                assert!(request.detectors.is_empty());
            }
            assert!(app.settings.mask_detect.detectors["saved"].selected);
        }
    }
}

#[test]
fn mask_detect_empty_contour_worker_result_keeps_masks_history_targets_and_visibility() {
    for (tool, target) in [
        (MaskDetectTool::Erase, LocalAdjustMaskEditTarget::Base),
        (MaskDetectTool::Conceal, LocalAdjustMaskEditTarget::Base),
        (MaskDetectTool::LocalAdjust, LocalAdjustMaskEditTarget::Base),
        (
            MaskDetectTool::LocalAdjust,
            LocalAdjustMaskEditTarget::OverrideAdd,
        ),
        (
            MaskDetectTool::LocalAdjust,
            LocalAdjustMaskEditTarget::OverrideSubtract,
        ),
    ] {
        let mut app = mask_detect_fixture(tool);
        if tool == MaskDetectTool::LocalAdjust {
            let mask = if target == LocalAdjustMaskEditTarget::Base {
                local_adjust_core::LocalMask::Raster(local_adjust_core::RasterMask {
                    width: 8,
                    height: 8,
                    alpha: Arc::new(vec![0.25; 64]),
                })
            } else {
                local_adjust_core::LocalMask::LinearGradient(Default::default())
            };
            mask_detect_install_local(&mut app, mask, target);
        }
        app.local_adjust_show_mask = false;
        app.settings.mask_detect.face_selected = false;
        app.settings.mask_detect.detectors.insert(
            "empty".into(),
            crate::settings::ExternalDetectorSettings {
                selected: true,
                ..Default::default()
            },
        );
        *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(vec![entry("empty")]));
        let before = app.local_adjust_page_layers.get(&0).cloned();
        let history = mask_detect_history(&app);
        let request = app.capture_mask_detect_request(tool).unwrap();
        let mut output = MaskDetectOutput::default();
        // Detection removes the all-false contour and its candidate together.
        // Inject that worker contract through the production merge and poll.
        output.append_ultralytics_result(
            &request.detectors[0],
            UltralyticsOutput {
                detections: Vec::new(),
                patches: Vec::new(),
            },
            request.options.margin_percent,
        );
        assert!(output.counts.is_empty());
        assert!(output.boxes.is_empty() && output.patches.is_empty());
        mask_detect_queue(&mut app, request, output);
        app.poll_mask_detect(&egui::Context::default());
        assert_eq!(app.local_adjust_page_layers.get(&0), before.as_ref());
        assert_eq!(mask_detect_history(&app), history);
        assert!(!app.local_adjust_show_mask);
        assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());
        assert_eq!(app.erase_mask.as_deref(), Some([false; 64].as_slice()));
        assert_eq!(app.conceal_mask.as_deref(), Some([false; 64].as_slice()));
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "見つかりませんでした"
        );
    }
}

#[test]
fn mask_detect_contour_counts_follow_adopted_worker_detections() {
    let catalog = DetectorCatalogState::Ready(Ok(vec![entry("mixed")]));
    let mut options = crate::settings::MaskDetectSettings::default();
    options.detectors.insert(
        "mixed".into(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            ..Default::default()
        },
    );
    let chosen = chosen_detectors(&catalog, &options).remove(0);
    let mut output = mask_detect_output();
    output.append_ultralytics_result(
        &chosen,
        UltralyticsOutput {
            detections: vec![Detection {
                class_idx: 1,
                score: 0.8,
                rect: [1.0, 0.0, 3.0, 2.0],
            }],
            patches: vec![patch()],
        },
        options.margin_percent,
    );
    output.append_ultralytics_result(
        &chosen,
        UltralyticsOutput {
            detections: Vec::new(),
            patches: Vec::new(),
        },
        options.margin_percent,
    );
    assert_eq!(output.boxes.len(), 1); // existing face result is kept
    assert_eq!(output.patches.len(), 1);
    assert_eq!(
        output.counts,
        vec![("顔".into(), 1), ("mixed / second".into(), 1)]
    );
}

#[test]
fn mask_detect_mixed_patch_and_box_patch_only_and_box_only_undo_as_one_edit() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for mode in 0..3 {
            let mut app = mask_detect_fixture(tool);
            let before = app.local_adjust_page_layers.get(&0).cloned();
            let mut output = mask_detect_output();
            if mode != 2 {
                output.patches.push(patch());
            }
            if mode == 1 {
                output.boxes.clear();
            }
            let request = app.capture_mask_detect_request(tool).unwrap();
            app.apply_mask_detect_output(&request, output);
            let expected_shapes = usize::from(mode != 1);
            match tool {
                MaskDetectTool::Erase => {
                    assert_eq!(app.erase_shapes.len(), expected_shapes);
                    assert_eq!(app.erase_mask.as_ref().unwrap()[1], mode != 2);
                    assert_eq!(app.erase_mask.as_ref().unwrap()[10], mode != 2);
                    assert_eq!(app.erase_undo_stack.len(), 1);
                    assert!(app.undo_erase());
                    assert!(app.erase_shapes.is_empty());
                    assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![false; 64]);
                }
                MaskDetectTool::Conceal => {
                    assert_eq!(app.conceal_shapes.len(), expected_shapes);
                    assert_eq!(app.conceal_mask.as_ref().unwrap()[1], mode != 2);
                    assert_eq!(app.conceal_mask.as_ref().unwrap()[10], mode != 2);
                    assert_eq!(app.conceal_undo_stack.len(), 1);
                    assert!(app.undo_conceal());
                    assert!(app.conceal_shapes.is_empty());
                    assert_eq!(app.conceal_mask.as_ref().unwrap(), &vec![false; 64]);
                }
                MaskDetectTool::LocalAdjust => {
                    let local_adjust_core::LocalMask::RasterVector(mask) =
                        &app.local_adjust_page_layers[&0][0].mask
                    else {
                        panic!("vector target")
                    };
                    assert_eq!(mask.shapes.len(), expected_shapes);
                    assert_eq!(mask.alpha[1], if mode != 2 { 1.0 } else { 0.0 });
                    assert_eq!(mask.alpha[10], if mode != 2 { 1.0 } else { 0.0 });
                    assert_eq!(app.meta_undo.undo_len(), 1);
                    app.apply_meta_undo();
                    assert_eq!(app.local_adjust_page_layers.get(&0), before.as_ref());
                }
            }
        }
    }
}

#[test]
fn mask_detect_settings_preserve_missing_detectors_and_all_class_semantics() {
    let mut options: crate::settings::MaskDetectSettings = serde_json::from_str("{}").unwrap();
    assert!(options.face_selected);
    assert_eq!(options.contour_margin_percent, 0.6);
    assert!(options.detectors.is_empty());
    options.detectors.insert(
        "missing.onnx".into(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            classes: Some(vec!["first".into()]),
            shape: Some(DetectorShape::Ellipse),
            threshold: Some(0.4),
        },
    );
    options
        .detectors
        .insert("all.onnx".into(), Default::default());
    let json = serde_json::to_value(&options).unwrap();
    let restored: crate::settings::MaskDetectSettings =
        serde_json::from_value(json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), json);
    assert_eq!(
        restored.detectors["missing.onnx"].classes.as_deref(),
        Some(["first".to_string()].as_slice())
    );
    assert!(restored.detectors["all.onnx"].classes.is_none());
    assert!(!restored.detectors["all.onnx"].selected);
    assert_eq!(
        chosen_detectors(&DetectorCatalogState::Ready(Ok(Vec::new())), &restored).len(),
        0
    );
    assert_eq!(restored.detectors.len(), 2);
}

#[test]
fn mask_detect_selection_requires_face_or_readable_selected_nonempty_classes_and_captures_options()
{
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    app.settings.mask_detect.face_selected = false;
    assert!(!app.mask_detect_selection_valid());
    *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(vec![
        entry("test.onnx"),
        DetectorEntry {
            name: "bad.onnx".into(),
            result: Err(DetectorReadError::OutputShape),
        },
    ]));
    app.settings.mask_detect.detectors.insert(
        "bad.onnx".into(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            ..Default::default()
        },
    );
    assert!(!app.mask_detect_selection_valid());
    app.settings.mask_detect.detectors.insert(
        "test.onnx".into(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            classes: Some(Vec::new()),
            ..Default::default()
        },
    );
    assert!(!app.mask_detect_selection_valid());
    app.settings
        .mask_detect
        .detectors
        .get_mut("test.onnx")
        .unwrap()
        .classes = Some(vec!["removed-class".into()]);
    assert!(!app.mask_detect_selection_valid());
    app.settings
        .mask_detect
        .detectors
        .get_mut("test.onnx")
        .unwrap()
        .classes = None;
    assert!(app.mask_detect_selection_valid());
    let request = app
        .capture_mask_detect_request(MaskDetectTool::Erase)
        .unwrap();
    assert_eq!(request.detectors.len(), 1);
    assert_eq!(
        request.detectors[0].params.selected_classes,
        vec![true, true]
    );
    assert_eq!(request.detectors[0].params.thresholds, vec![0.15, 0.3]);
    assert_eq!(request.detectors[0].shape, DetectorShape::Contour);
    let old_key = request.detectors[0].info.key.clone();
    let old_names = request.detectors[0].info.names.clone();
    let mut replacement = entry("test.onnx");
    let info = replacement.result.as_mut().unwrap();
    info.key.size = 999;
    info.names = vec!["new-class".into()];
    info.thresholds = vec![0.8];
    *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(vec![replacement]));
    assert_eq!(request.detectors[0].info.key, old_key);
    assert_eq!(request.detectors[0].info.names, old_names);
    assert_eq!(request.detectors[0].info.thresholds, vec![0.15, 0.3]);
    assert_eq!(
        request.detectors[0].params.selected_classes,
        vec![true, true]
    );
    assert!(app.mask_detect_request_is_current(&request));
    *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(Vec::new()));
    assert_eq!(request.detectors[0].info.key, old_key);
    assert_eq!(request.detectors[0].info.names, old_names);
    assert_eq!(request.detectors[0].params.thresholds, vec![0.15, 0.3]);
    app.settings.mask_detect.face_selected = true;
    app.settings.mask_detect.detectors.clear();
    assert!(!request.options.face_selected);
    assert_eq!(request.detectors.len(), 1);
    assert!(app.mask_detect_request_is_current(&request));
}

#[test]
fn mask_detect_catalog_loading_worker_runs_off_ui_and_retains_activity_until_terminal() {
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    *app.mask_detect_catalog = DetectorCatalogState::Idle;
    let ui_thread = std::thread::current().id();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let activity = app.local_ai_activity.load(Ordering::Acquire);
    let folder = app.tmp.path().join("detectors");
    app.launch_mask_detect_catalog_worker(folder, move |_| {
        started_tx.send(std::thread::current().id()).unwrap();
        release_rx.recv().unwrap();
        Ok(vec![entry("worker.onnx")])
    });
    assert_ne!(
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        ui_thread
    );
    assert!(matches!(
        &*app.mask_detect_catalog,
        DetectorCatalogState::Loading { .. }
    ));
    assert_eq!(app.local_ai_activity.load(Ordering::Acquire), activity + 1);
    app.poll_mask_detect_catalog(&egui::Context::default());
    assert!(matches!(
        &*app.mask_detect_catalog,
        DetectorCatalogState::Loading { .. }
    ));
    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while matches!(
        &*app.mask_detect_catalog,
        DetectorCatalogState::Loading { .. }
    ) {
        assert!(std::time::Instant::now() < deadline);
        app.poll_mask_detect_catalog(&egui::Context::default());
        std::thread::yield_now();
    }
    assert!(
        matches!(&*app.mask_detect_catalog, DetectorCatalogState::Ready(Ok(entries)) if entries.len() == 1)
    );
    while app.local_ai_activity.load(Ordering::Acquire) != activity {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn mask_detect_summary_omits_zero_counts_and_limits_detail() {
    let summary = detection_summary(
        10,
        &[
            ("zero".into(), 0),
            ("one".into(), 1),
            ("two".into(), 2),
            ("three".into(), 3),
            ("four".into(), 4),
        ],
    );
    assert!(summary.starts_with("10 か所を追加しました"));
    assert!(!summary.contains("zero"));
    assert!(!summary.contains("four"));
    assert!(summary.contains("ほか"));
}

#[test]
fn mask_detect_replacing_segment_with_detect_keeps_saved_shape_but_uses_rect() {
    let mut entry = entry("replacement.onnx");
    let info = entry.result.as_mut().unwrap();
    let setting = crate::settings::ExternalDetectorSettings {
        selected: true,
        shape: Some(DetectorShape::Contour),
        ..Default::default()
    };
    assert_eq!(detector_shape(info, &setting), DetectorShape::Contour);
    info.format = DetectorFormat::UltralyticsDetect;
    assert_eq!(detector_shape(info, &setting), DetectorShape::Rect);
    assert_eq!(setting.shape, Some(DetectorShape::Contour));
}

#[test]
fn mask_detect_many_readable_and_unreadable_rows_scroll_with_visible_actions() {
    let _input = crate::key_input::lock_test_input();
    for scale in [1.0_f32, 2.0] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        let mut entries = Vec::new();
        for index in 0..10 {
            let name = format!("readable-detector-{index}.onnx");
            entries.push(entry(&name));
            app.settings.mask_detect.detectors.insert(
                name,
                crate::settings::ExternalDetectorSettings {
                    selected: true,
                    ..Default::default()
                },
            );
            entries.push(DetectorEntry {
                name: format!("unreadable-detector-{index}.onnx"),
                result: Err(DetectorReadError::OutputShape),
            });
        }
        *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(entries));
        app.mask_detect_dialog = Some(MaskDetectTool::Erase);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        ctx.set_pixels_per_point(scale);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0) / scale);
        for _ in 0..4 {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.show_mask_detect_dialog(ui.ctx()),
            )
            .drop_without_applying_deltas();
        }
        let layout = ctx
            .data(|data| {
                data.get_temp::<crate::test_ui::scroll_layout::ScrollLayout>(egui::Id::unique(
                    "mask_detect_body_layout",
                ))
            })
            .unwrap();
        layout.assert_height(
            crate::test_ui::scroll_layout::HeightExpectation::Scrolls,
            screen,
            scale,
        );
        let window = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_window")))
            .unwrap();
        assert!(
            screen.expand(1.0 / scale).contains_rect(window),
            "scale={scale}, window={window:?}"
        );
        let actions = ctx
            .data(|data| data.get_temp::<Vec<egui::Rect>>(egui::Id::unique("mask_detect_actions")))
            .unwrap();
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|action| screen.contains_rect(*action)));
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("mask_detect_reload_enabled"))),
            Some(true)
        );
    }
}

#[test]
fn mask_detect_unselected_face_collapses_shape_and_details_but_keeps_shared_margin() {
    let _input = crate::key_input::lock_test_input();
    fn has_text(shape: &egui::Shape, label: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(label),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, label)),
            _ => false,
        }
    }
    for selected in [false, true] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        let mut boxed = entry("boxed");
        boxed.result.as_mut().unwrap().format = DetectorFormat::UltralyticsDetect;
        *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(vec![boxed]));
        app.settings.mask_detect.detectors.insert(
            "boxed".into(),
            crate::settings::ExternalDetectorSettings {
                selected: true,
                ..Default::default()
            },
        );
        app.mask_detect_dialog = Some(MaskDetectTool::Erase);
        app.settings.mask_detect.face_selected = selected;
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let mut margin_painted = false;
        for _ in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1093.0, 614.0),
                    )),
                    ..Default::default()
                },
                |ui| app.show_mask_detect_dialog(ui.ctx()),
            );
            margin_painted |= output
                .shapes
                .iter()
                .any(|shape| has_text(&shape.shape, "箱の余白"));
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_details")))
                .is_some(),
            selected
        );
        assert!(margin_painted);
    }
}

#[test]
fn mask_detect_margin_sliders_follow_selected_readable_detector_kinds_and_keep_values() {
    let _input = crate::key_input::lock_test_input();
    for (label, face, format, selected, readable, shape, expected) in [
        ("empty", false, None, false, true, None, (false, false)),
        ("face only", true, None, false, true, None, (true, false)),
        (
            "unselected box",
            false,
            Some(DetectorFormat::UltralyticsDetect),
            false,
            true,
            None,
            (false, false),
        ),
        (
            "selected box",
            false,
            Some(DetectorFormat::UltralyticsDetect),
            true,
            true,
            None,
            (true, false),
        ),
        (
            "unselected segment",
            false,
            Some(DetectorFormat::UltralyticsSegment),
            false,
            true,
            None,
            (false, false),
        ),
        (
            "selected segment",
            false,
            Some(DetectorFormat::UltralyticsSegment),
            true,
            true,
            None,
            (false, true),
        ),
        (
            "face and segment",
            true,
            Some(DetectorFormat::UltralyticsSegment),
            true,
            true,
            None,
            (true, true),
        ),
        (
            "unreadable segment",
            false,
            Some(DetectorFormat::UltralyticsSegment),
            true,
            false,
            None,
            (false, false),
        ),
        (
            "unreadable box",
            false,
            Some(DetectorFormat::UltralyticsDetect),
            true,
            false,
            None,
            (false, false),
        ),
        (
            "segment as ellipse",
            false,
            Some(DetectorFormat::UltralyticsSegment),
            true,
            true,
            Some(DetectorShape::Ellipse),
            (true, true),
        ),
        (
            "segment as rect",
            false,
            Some(DetectorFormat::UltralyticsSegment),
            true,
            true,
            Some(DetectorShape::Rect),
            (true, true),
        ),
    ] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        app.settings.mask_detect.face_selected = face;
        app.settings.mask_detect.margin_percent = 27.0;
        app.settings.mask_detect.contour_margin_percent = 1.2;
        // An absent saved selection must neither enable a slider nor be erased.
        app.settings.mask_detect.detectors.insert(
            "absent".into(),
            crate::settings::ExternalDetectorSettings {
                selected: true,
                ..Default::default()
            },
        );
        let mut entries = Vec::new();
        if let Some(format) = format {
            let mut detector = entry("model");
            detector.result.as_mut().unwrap().format = format;
            if !readable {
                detector.result = Err(DetectorReadError::OutputShape);
            }
            entries.push(detector);
            app.settings.mask_detect.detectors.insert(
                "model".into(),
                crate::settings::ExternalDetectorSettings {
                    selected,
                    shape,
                    ..Default::default()
                },
            );
        }
        *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(entries));
        assert_eq!(
            mask_detect_margin_visibility(&app.mask_detect_catalog, &app.settings.mask_detect),
            expected,
            "{label}"
        );
        app.mask_detect_dialog = Some(MaskDetectTool::Erase);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        // A selected model's controls can push the contour margin below the
        // ScrollArea viewport. egui omits clipped labels from output.shapes, so
        // the shared visibility predicate, rather than painted text, determines
        // whether each slider belongs to the body. Render as well to check that
        // the widgets preserve the stored values.
        for _ in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1093.0, 614.0),
                    )),
                    ..Default::default()
                },
                |ui| app.show_mask_detect_dialog(ui.ctx()),
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            mask_detect_margin_visibility(&app.mask_detect_catalog, &app.settings.mask_detect),
            expected,
            "{label}: after rendering"
        );
        assert_eq!(app.settings.mask_detect.margin_percent, 27.0);
        assert_eq!(app.settings.mask_detect.contour_margin_percent, 1.2);
        assert!(app.settings.mask_detect.detectors["absent"].selected);
    }
}

#[test]
fn mask_detect_margin_sliders_do_not_infer_detector_kinds_before_catalog_ready() {
    let mut options = crate::settings::MaskDetectSettings::default();
    options.detectors.insert(
        "saved".into(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            ..Default::default()
        },
    );
    for face in [false, true] {
        options.face_selected = face;
        let (_tx, rx) = mpsc::channel();
        for catalog in [
            DetectorCatalogState::Idle,
            DetectorCatalogState::Loading {
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
            },
            DetectorCatalogState::Ready(Err("read failed".into())),
        ] {
            assert_eq!(
                mask_detect_margin_visibility(&catalog, &options),
                (face, false)
            );
        }
    }
}

#[test]
fn mask_detect_catalog_report_includes_counts_typed_reasons_and_error_details_on_single_lines() {
    let folder = PathBuf::from("test-data").join("detectors");
    let report = detector_catalog_report(
        &folder,
        &Ok(vec![
            entry("good"),
            DetectorEntry {
                name: "broken\nname".into(),
                result: Err(DetectorReadError::Json(
                    "threshold.json: invalid value\nline 2".into(),
                )),
            },
            DetectorEntry {
                name: "shape".into(),
                result: Err(DetectorReadError::OutputShape),
            },
        ]),
    );
    assert_eq!((report.readable, report.unreadable), (1, 2));
    assert_eq!(report.lines.len(), 3);
    assert!(report.lines[0].contains(&format!("folder={folder:?}")));
    assert!(report.lines[0].contains("readable=1 unreadable=2"));
    assert!(report.lines[1].contains("broken\\nname"));
    assert!(report.lines[1].contains("reason=Json("));
    assert!(report.lines[1].contains("JSON を読めません"));
    assert!(report.lines[1].contains("threshold.json: invalid value\\nline 2"));
    assert!(report.lines[2].contains("name=\"shape\" reason=OutputShape"));
    assert!(report.lines[2].contains("出力の形式を読めません"));
    assert!(
        report
            .lines
            .iter()
            .all(|line| !line.contains('\r') && !line.contains('\n'))
    );
}

#[test]
fn mask_detect_catalog_report_empty_missing_and_failed_reads_each_have_one_summary() {
    for folder in ["absent/detectors", "empty/detectors"] {
        let report = detector_catalog_report(std::path::Path::new(folder), &Ok(Vec::new()));
        assert_eq!((report.readable, report.unreadable), (0, 0));
        assert_eq!(report.lines.len(), 1);
        assert!(report.lines[0].contains("readable=0 unreadable=0"));
        assert!(report.lines[0].contains(folder));
    }
    let report = detector_catalog_report(
        std::path::Path::new("detectors"),
        &Err("read_dir failed\naccess denied".into()),
    );
    assert_eq!(report.lines.len(), 1);
    assert!(report.lines[0].contains("read_dir failed\\naccess denied"));
    assert!(!report.lines[0].contains('\n'));
}

#[test]
fn mask_detect_loading_row_disables_reload_without_changing_selection() {
    let _input = crate::key_input::lock_test_input();
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    let (_tx, rx) = mpsc::channel();
    *app.mask_detect_catalog = DetectorCatalogState::Loading {
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    };
    app.mask_detect_dialog = Some(MaskDetectTool::Erase);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    fn has_loading_text(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text() == "検出器を読み込み中…",
            egui::Shape::Vec(shapes) => shapes.iter().any(has_loading_text),
            _ => false,
        }
    }
    let mut painted_loading = false;
    for _ in 0..3 {
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1093.0, 614.0),
                )),
                ..Default::default()
            },
            |ui| app.show_mask_detect_dialog(ui.ctx()),
        );
        painted_loading |= output
            .shapes
            .iter()
            .any(|shape| has_loading_text(&shape.shape));
        output.drop_without_applying_deltas();
    }
    assert!(painted_loading);
    assert_eq!(
        ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("mask_detect_reload_enabled"))),
        Some(false)
    );
    assert!(app.settings.mask_detect.face_selected);
    assert!(app.mask_detect_selection_valid());
}

#[test]
fn mask_detect_patch_writes_only_true_bits_with_clipped_bounds() {
    let mut mask = vec![false; 9];
    apply_mask_patches(
        &[MaskPatch {
            x: 2,
            y: 2,
            w: 2,
            h: 2,
            bits: vec![true; 4],
        }],
        [3, 3],
        |index| mask[index] = true,
    );
    assert_eq!(
        mask,
        vec![false, false, false, false, false, false, false, false, true]
    );
}

#[test]
fn mask_detect_cancelled_or_stale_mixed_results_preserve_masks_document_undo_and_visibility() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for cancel in [false, true] {
            let mut app = mask_detect_fixture(tool);
            app.local_adjust_show_mask = false;
            let before = app.local_adjust_page_layers.get(&0).cloned();
            let mut output = mask_detect_output();
            output.patches.push(patch());
            let request = app.capture_mask_detect_request(tool).unwrap();
            let token = mask_detect_queue(&mut app, request, output);
            if cancel {
                token.store(true, Ordering::Release);
            } else {
                app.items_generation = app.items_generation.wrapping_add(1);
            }
            app.poll_mask_detect(&egui::Context::default());
            assert!(app.mask_detect_pending.is_none());
            assert!(app.erase_shapes.is_empty());
            assert!(app.conceal_shapes.is_empty());
            assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![false; 64]);
            assert_eq!(app.conceal_mask.as_ref().unwrap(), &vec![false; 64]);
            assert_eq!(app.local_adjust_page_layers.get(&0), before.as_ref());
            assert_eq!(app.erase_undo_stack.len(), 0);
            assert_eq!(app.conceal_undo_stack.len(), 0);
            assert_eq!(app.meta_undo.undo_len(), 0);
            assert!(!app.local_adjust_show_mask);
        }
    }
}

#[test]
fn mask_detect_chosen_detector_uses_json_defaults_and_explicit_class_threshold_shape() {
    let mut e = entry("defaults.onnx");
    e.result.as_mut().unwrap().shape = Some(DetectorShape::Ellipse);
    let mut options = crate::settings::MaskDetectSettings::default();
    options.detectors.insert(
        e.name.clone(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            ..Default::default()
        },
    );
    let catalog = DetectorCatalogState::Ready(Ok(vec![e]));
    assert_eq!(
        chosen_detectors(&catalog, &options)[0].shape,
        DetectorShape::Ellipse
    );
    let setting = options.detectors.get_mut("defaults.onnx").unwrap();
    setting.classes = Some(vec!["second".into()]);
    setting.threshold = Some(0.4);
    setting.shape = Some(DetectorShape::Rect);
    let chosen = chosen_detectors(&catalog, &options);
    assert_eq!(chosen[0].shape, DetectorShape::Rect);
    assert_eq!(chosen[0].params.selected_classes, vec![false, true]);
    assert_eq!(chosen[0].params.thresholds, vec![0.4, 0.4]);
}
