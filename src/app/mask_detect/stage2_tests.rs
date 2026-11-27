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
    app.launch_mask_detect_catalog_worker(move |_| {
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
        *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(Vec::new()));
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
