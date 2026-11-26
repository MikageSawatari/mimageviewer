//! Injected detection results exercise App ownership and edit history without ONNX.
use super::*;
use crate::app::{AppTestEnvForTest, setup_app_for_test};
use local_adjust_core::{
    LocalAdjustmentLayer, LocalEffect, LocalMask, RasterMask, RasterVectorMask,
};

const MASK_DETECT_TOOLS: [MaskDetectTool; 3] = [
    MaskDetectTool::Erase,
    MaskDetectTool::Conceal,
    MaskDetectTool::LocalAdjust,
];

pub(super) fn mask_detect_output() -> MaskDetectOutput {
    mask_detect_boxes(
        vec![Detection {
            class_idx: 0,
            score: 0.9,
            rect: [2.0, 2.0, 6.0, 6.0],
        }],
        &Default::default(),
    )
}

pub(super) fn mask_detect_fixture(tool: MaskDetectTool) -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    let path = app.tmp.path().join("mask-detect.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([70, 100, 130, 255]))
        .save(&path)
        .unwrap();
    mask_detect_install_source(&mut app, path, tool, &egui::Context::default());
    app
}

pub(super) fn mask_detect_install_source(
    app: &mut App,
    path: PathBuf,
    tool: MaskDetectTool,
    ctx: &egui::Context,
) {
    app.items = vec![GridItem::Image(path)];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.page_edit_snapshot = Some(page_edit_snapshot::PageEditSnapshot {
        order: Arc::new(page_edit_snapshot::PageKeyOrder::from_items(&app.items).unwrap()),
        ..Default::default()
    });
    let pixels = Arc::new(egui::ColorImage::filled([8, 8], egui::Color32::GRAY));
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: ctx.load_texture(
                "mask-detect-source",
                pixels.as_ref().clone(),
                Default::default(),
            ),
            pixels: Arc::clone(&pixels),
            source_dims: Some([8, 8]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.erase_base_cache
        .insert(0, EraseBaseSource::raster(pixels));
    app.erase_mask_size = [8, 8];
    app.erase_mask = Some(vec![false; 64]);
    app.conceal_mask_size = [8, 8];
    app.conceal_mask = Some(vec![false; 64]);
    app.erase_mode = tool == MaskDetectTool::Erase;
    app.conceal_mode = tool == MaskDetectTool::Conceal;
    app.local_adjust_mode = tool == MaskDetectTool::LocalAdjust;
    if tool == MaskDetectTool::LocalAdjust {
        mask_detect_install_local(
            app,
            LocalMask::RasterVector(RasterVectorMask::empty(8, 8)),
            LocalAdjustMaskEditTarget::Base,
        );
    }
}

fn mask_detect_install_local(app: &mut App, mask: LocalMask, target: LocalAdjustMaskEditTarget) {
    app.local_page_edit_document = LocalPageEditDocument::Idle;
    app.adopt_materialized_local_layers(
        0,
        Arc::new(vec![LocalAdjustmentLayer::new(
            "face",
            mask,
            LocalEffect::None,
        )]),
    );
    app.local_adjust_selected_layers.insert(0, 0);
    app.local_adjust_mask_edit_target = target;
}

fn mask_detect_history(app: &App) -> (usize, usize, usize) {
    (
        app.erase_undo_stack.len(),
        app.conceal_undo_stack.len(),
        app.meta_undo.undo_len(),
    )
}

pub(super) fn mask_detect_queue(
    app: &mut App,
    request: MaskDetectRequest,
    output: MaskDetectOutput,
) -> Arc<AtomicBool> {
    let (tx, rx) = mpsc::channel();
    assert!(tx.send(Ok(output)).is_ok());
    let cancel = Arc::new(AtomicBool::new(false));
    app.mask_detect_pending = Some(Box::new(MaskDetectPending {
        request,
        cancel: Arc::clone(&cancel),
        rx,
    }));
    cancel
}

#[test]
fn mask_detect_three_tools_add_one_edit_and_undo_once() {
    for tool in MASK_DETECT_TOOLS {
        let mut app = mask_detect_fixture(tool);
        let local_before = app.local_adjust_page_layers.get(&0).cloned();
        let before = mask_detect_history(&app);
        let request = app
            .capture_mask_detect_request(tool)
            .expect("ready source and target");
        let source = Arc::clone(&request.source);
        app.apply_mask_detect_output(&request, mask_detect_output());
        assert!(Arc::ptr_eq(&source, &request.source));
        match tool {
            MaskDetectTool::Erase => {
                assert_eq!(app.erase_shapes.len(), 1);
                assert_eq!(
                    mask_detect_history(&app),
                    (before.0 + 1, before.1, before.2)
                );
                assert!(
                    app.erase_inpaint_pending.is_empty(),
                    "detection must not start MI-GAN"
                );
                assert!(app.undo_erase());
                assert!(app.erase_shapes.is_empty());
                assert_eq!(app.erase_mask.as_deref(), Some([false; 64].as_slice()));
            }
            MaskDetectTool::Conceal => {
                assert_eq!(app.conceal_shapes.len(), 1);
                assert_eq!(
                    mask_detect_history(&app),
                    (before.0, before.1 + 1, before.2)
                );
                assert!(app.undo_conceal());
                assert!(app.conceal_shapes.is_empty());
                assert_eq!(app.conceal_mask.as_deref(), Some([false; 64].as_slice()));
            }
            MaskDetectTool::LocalAdjust => {
                let LocalMask::RasterVector(mask) = &app.local_adjust_page_layers[&0][0].mask
                else {
                    panic!("vector mask expected");
                };
                assert_eq!(mask.shapes.len(), 1);
                assert_eq!(
                    mask_detect_history(&app),
                    (before.0, before.1, before.2 + 1)
                );
                app.apply_meta_undo();
                assert_eq!(app.local_adjust_page_layers.get(&0), local_before.as_ref());
            }
        }
        assert_eq!(
            mask_detect_history(&app),
            before,
            "one undo restores the whole detection"
        );
    }
}

#[test]
fn mask_detect_zero_and_cancel_do_not_change_masks_documents_or_history() {
    for tool in MASK_DETECT_TOOLS {
        for cancel_result in [false, true] {
            let mut app = mask_detect_fixture(tool);
            app.local_adjust_show_mask = false;
            let before = mask_detect_history(&app);
            let local_before = app.local_adjust_page_layers.get(&0).cloned();
            let request = app.capture_mask_detect_request(tool).unwrap();
            let output = if cancel_result {
                mask_detect_output()
            } else {
                mask_detect_boxes(Vec::new(), &Default::default())
            };
            let cancel = mask_detect_queue(&mut app, request, output);
            if cancel_result {
                cancel.store(true, Ordering::Release);
            }
            app.poll_mask_detect(&egui::Context::default());
            assert!(app.mask_detect_pending.is_none());
            assert!(app.erase_shapes.is_empty());
            assert!(app.conceal_shapes.is_empty());
            assert_eq!(app.erase_mask.as_deref(), Some([false; 64].as_slice()));
            assert_eq!(app.conceal_mask.as_deref(), Some([false; 64].as_slice()));
            assert_eq!(app.local_adjust_page_layers.get(&0), local_before.as_ref());
            assert_eq!(mask_detect_history(&app), before);
            assert!(!app.local_adjust_show_mask);
        }
    }
}

#[test]
fn mask_detect_stale_identity_size_source_and_edit_session_are_discarded() {
    for tool in MASK_DETECT_TOOLS {
        for mismatch in 0..6 {
            let mut app = mask_detect_fixture(tool);
            app.local_adjust_show_mask = false;
            let before = mask_detect_history(&app);
            let mut request = app.capture_mask_detect_request(tool).unwrap();
            match mismatch {
                0 => {
                    request.context =
                        ViewerContextId::for_test(request.context.serial().wrapping_add(1))
                }
                1 => app.items_generation = app.items_generation.wrapping_add(1),
                2 => request.page_key.push_str("-different-page"),
                3 => request.mask_size = [7, 8],
                // Equal pixels in a different Arc are a different source publication.
                4 => request.source = Arc::new(request.source.as_ref().clone()),
                5 => match tool {
                    MaskDetectTool::Erase => app.erase_session = app.erase_session.wrapping_add(1),
                    MaskDetectTool::Conceal => {
                        app.conceal_session = app.conceal_session.wrapping_add(1)
                    }
                    MaskDetectTool::LocalAdjust => {
                        let mut changed = app.local_adjust_page_layers[&0].clone();
                        Arc::make_mut(&mut changed)[0].name = "changed while detecting".into();
                        app.set_local_adjust_layers_for_idx_memory_only(0, changed);
                    }
                },
                _ => unreachable!(),
            }
            let local_before = app.local_adjust_page_layers.get(&0).cloned();
            mask_detect_queue(&mut app, request, mask_detect_output());
            app.poll_mask_detect(&egui::Context::default());
            assert!(app.mask_detect_pending.is_none());
            assert!(
                app.erase_shapes.is_empty(),
                "tool={tool:?}, mismatch={mismatch}"
            );
            assert!(
                app.conceal_shapes.is_empty(),
                "tool={tool:?}, mismatch={mismatch}"
            );
            assert_eq!(
                app.local_adjust_page_layers.get(&0),
                local_before.as_ref(),
                "mismatch={mismatch}"
            );
            assert_eq!(
                mask_detect_history(&app),
                before,
                "tool={tool:?}, mismatch={mismatch}"
            );
            assert!(
                !app.local_adjust_show_mask,
                "tool={tool:?}, mismatch={mismatch}"
            );
        }
    }
}

#[test]
fn mask_detect_local_adopted_result_enables_mask_display() {
    let mut app = mask_detect_fixture(MaskDetectTool::LocalAdjust);
    app.local_adjust_show_mask = false;
    let request = app
        .capture_mask_detect_request(MaskDetectTool::LocalAdjust)
        .unwrap();
    assert!(
        !app.local_adjust_show_mask,
        "starting detection must preserve visibility"
    );
    mask_detect_queue(&mut app, request, mask_detect_output());
    app.poll_mask_detect(&egui::Context::default());
    let LocalMask::RasterVector(mask) = &app.local_adjust_page_layers[&0][0].mask else {
        panic!("vector target");
    };
    assert_eq!(mask.shapes.len(), 1);
    assert!(app.local_adjust_show_mask);
    assert_eq!(app.meta_undo.undo_len(), 1);
}

#[test]
fn mask_detect_same_source_arc_is_adopted_through_poll() {
    for tool in MASK_DETECT_TOOLS {
        let mut app = mask_detect_fixture(tool);
        let request = app.capture_mask_detect_request(tool).unwrap();
        let source = Arc::clone(&request.source);
        let (_, current, _, _) = app.mask_detect_source(tool).unwrap();
        assert!(Arc::ptr_eq(&source, &current));
        let before = mask_detect_history(&app);
        mask_detect_queue(&mut app, request, mask_detect_output());
        app.poll_mask_detect(&egui::Context::default());
        let after = mask_detect_history(&app);
        assert_eq!(
            after.0 + after.1 + after.2,
            before.0 + before.1 + before.2 + 1
        );
    }
}

#[test]
fn mask_detect_local_raster_conversion_and_absent_overrides_are_one_undo() {
    for target in [
        LocalAdjustMaskEditTarget::Base,
        LocalAdjustMaskEditTarget::OverrideAdd,
        LocalAdjustMaskEditTarget::OverrideSubtract,
    ] {
        for zero in [false, true] {
            let mut app = mask_detect_fixture(MaskDetectTool::LocalAdjust);
            let mask = if target == LocalAdjustMaskEditTarget::Base {
                LocalMask::Raster(RasterMask {
                    width: 8,
                    height: 8,
                    alpha: Arc::new(vec![0.25; 64]),
                })
            } else {
                LocalMask::LinearGradient(Default::default())
            };
            mask_detect_install_local(&mut app, mask, target);
            let before = app.local_adjust_page_layers[&0].clone();
            let history = mask_detect_history(&app);
            let request = app
                .capture_mask_detect_request(MaskDetectTool::LocalAdjust)
                .unwrap();
            app.apply_mask_detect_output(
                &request,
                if zero {
                    mask_detect_boxes(Vec::new(), &Default::default())
                } else {
                    mask_detect_output()
                },
            );
            if zero {
                assert_eq!(app.local_adjust_page_layers[&0], before);
                assert_eq!(mask_detect_history(&app), history);
                continue;
            }
            let layer = &app.local_adjust_page_layers[&0][0];
            let mask = match target {
                LocalAdjustMaskEditTarget::Base => {
                    let LocalMask::RasterVector(mask) = &layer.mask else {
                        panic!("raster base must become raster-vector");
                    };
                    assert_eq!(mask.alpha.as_slice(), &[0.25; 64]);
                    mask
                }
                LocalAdjustMaskEditTarget::OverrideAdd => {
                    layer.manual_override.add.as_ref().unwrap()
                }
                LocalAdjustMaskEditTarget::OverrideSubtract => {
                    layer.manual_override.subtract.as_ref().unwrap()
                }
                _ => unreachable!(),
            };
            assert_eq!([mask.width, mask.height], [8, 8]);
            assert_eq!(mask.shapes.len(), 1);
            assert_eq!(app.meta_undo.undo_len(), history.2 + 1);
            app.apply_meta_undo();
            assert_eq!(app.local_adjust_page_layers[&0], before);
            assert_eq!(mask_detect_history(&app), history);
        }
    }
}

#[test]
fn mask_detect_overlapping_modes_only_change_captured_tool() {
    for tool in MASK_DETECT_TOOLS {
        let mut app = mask_detect_fixture(tool);
        app.erase_mode = true;
        app.conceal_mode = true;
        app.local_adjust_mode = true;
        let before = mask_detect_history(&app);
        let local_before = app.local_adjust_page_layers.get(&0).cloned();
        let request = app.capture_mask_detect_request(tool).unwrap();
        app.apply_mask_detect_output(&request, mask_detect_output());
        assert_eq!(
            app.erase_shapes.len(),
            usize::from(tool == MaskDetectTool::Erase)
        );
        assert_eq!(
            app.conceal_shapes.len(),
            usize::from(tool == MaskDetectTool::Conceal)
        );
        if tool != MaskDetectTool::LocalAdjust {
            assert_eq!(app.local_adjust_page_layers.get(&0), local_before.as_ref());
        }
        assert_eq!(
            mask_detect_history(&app),
            (
                before.0 + usize::from(tool == MaskDetectTool::Erase),
                before.1 + usize::from(tool == MaskDetectTool::Conceal),
                before.2 + usize::from(tool == MaskDetectTool::LocalAdjust),
            )
        );
    }
}

#[test]
fn mask_detect_cancel_drops_queued_result_and_releases_modal_input_block() {
    for tool in MASK_DETECT_TOOLS {
        let mut app = mask_detect_fixture(tool);
        app.open_mask_detect_dialog(tool);
        assert!(app.mask_detect_active());
        assert!(app.erase_input_blocked());
        assert!(app.conceal_input_blocked());
        assert!(app.local_adjust_mask_input_blocked());
        let request = app.capture_mask_detect_request(tool).unwrap();
        let cancel = mask_detect_queue(&mut app, request, mask_detect_output());
        assert!(!app.mask_detect_available(tool));
        assert!(
            app.capture_mask_detect_request(tool).is_none(),
            "a second request must not replace the first"
        );
        let before = mask_detect_history(&app);
        app.cancel_mask_detect();
        assert!(cancel.load(Ordering::Acquire));
        app.poll_mask_detect(&egui::Context::default());
        assert!(!app.mask_detect_active());
        assert!(!app.erase_input_blocked());
        assert!(!app.conceal_input_blocked());
        assert!(!app.local_adjust_mask_input_blocked());
        assert_eq!(mask_detect_history(&app), before);
        assert!(app.erase_shapes.is_empty());
        assert!(app.conceal_shapes.is_empty());
    }
}

#[test]
fn mask_detect_erase_invalidates_preview_without_starting_inpaint() {
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    let ctx = egui::Context::default();
    let pixels = Arc::clone(&app.erase_base_cache[&0].pixels);
    let display_basis = app.erase_base_cache[&0].display_basis.clone();
    app.erase_preview_cache.insert(
        0,
        ErasePreviewCacheEntry {
            display_basis,
            pixels: Arc::clone(&pixels),
            texture: ctx.load_texture(
                "mask-detect-old-preview",
                pixels.as_ref().clone(),
                Default::default(),
            ),
        },
    );
    let request = app
        .capture_mask_detect_request(MaskDetectTool::Erase)
        .unwrap();
    app.apply_mask_detect_output(&request, mask_detect_output());
    assert!(!app.erase_preview_cache.contains_key(&0));
    assert!(app.erase_inpaint_pending.is_empty());
    assert!(Arc::ptr_eq(&app.erase_base_cache[&0].pixels, &pixels));
}

#[test]
fn mask_detect_other_local_ai_mask_and_lut_requests_prevent_detection() {
    for segmentation in [false, true] {
        let mut app = mask_detect_fixture(MaskDetectTool::LocalAdjust);
        assert!(app.begin_local_page_edit(0));
        let owner = app.capture_page_edit_owner(0).unwrap();
        let document = app.local_page_edit_layers_for_target(&owner.target);
        if segmentation {
            let (_tx, rx) = mpsc::channel();
            app.local_adjust_segmentation_pending = Some(LocalAdjustSegmentationPending {
                owner,
                fs_idx: 0,
                layer_idx: 0,
                document,
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
            });
        } else {
            let (_tx, rx) = mpsc::channel();
            app.local_adjust_lut_pending = Some(LocalAdjustLutLoadPending {
                owner,
                fs_idx: 0,
                layer_idx: 0,
                document,
                path: app.tmp.path().join("mask-detect-pending.cube"),
                rx,
            });
        }
        assert!(!app.mask_detect_available(MaskDetectTool::LocalAdjust));
        assert!(
            app.capture_mask_detect_request(MaskDetectTool::LocalAdjust)
                .is_none()
        );
        app.open_mask_detect_dialog(MaskDetectTool::LocalAdjust);
        assert!(app.mask_detect_dialog.is_none());
        assert_eq!(mask_detect_history(&app), (0, 0, 0));
    }
}

#[test]
fn mask_detect_real_worker_holds_activity_while_waiting_and_cancel_releases_it() {
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    let runtime = crate::ai::runtime::AiRuntimeInitOwner::new();
    let (release, wait) = mpsc::channel::<()>();
    // Closing the sender on an assertion failure also releases this fake initializer.
    assert!(runtime.start_for_test(move || {
        let _ = wait.recv();
        Err(crate::ai::AiError::Ort(
            "mask detection test does not load ONNX".into(),
        ))
    }));
    app.ai_runtime_init = Arc::clone(&runtime);
    let activity_before = app.local_ai_activity.load(Ordering::Acquire);
    app.start_mask_detect(MaskDetectTool::Erase);
    let cancel = Arc::clone(
        &app.mask_detect_pending
            .as_ref()
            .expect("worker started")
            .cancel,
    );
    assert_eq!(
        app.local_ai_activity.load(Ordering::Acquire),
        activity_before + 1
    );
    assert!(!app.local_ai_remote_barrier_snapshot().is_quiesced());
    // Repeated acceptance must retain the first worker and its cancel token.
    app.start_mask_detect(MaskDetectTool::Erase);
    assert!(Arc::ptr_eq(
        &cancel,
        &app.mask_detect_pending.as_ref().unwrap().cancel
    ));
    assert_eq!(
        app.local_ai_activity.load(Ordering::Acquire),
        activity_before + 1
    );
    app.cancel_mask_detect();
    assert!(cancel.load(Ordering::Acquire));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.local_ai_activity.load(Ordering::Acquire) != activity_before {
        assert!(
            std::time::Instant::now() < deadline,
            "cancelled worker must release its own activity"
        );
        std::thread::yield_now();
    }
    drop(release);
    assert!(matches!(
        runtime.wait_terminal_while(|| true),
        crate::ai::runtime::AiRuntimeInitWait::Failed(_)
    ));
    assert!(app.erase_shapes.is_empty());
    assert_eq!(mask_detect_history(&app), (0, 0, 0));
}

#[test]
fn mask_detect_dialog_actions_fit_small_screen_at_100_and_200_percent() {
    let _input = crate::key_input::lock_test_input();
    for scale in [1.0_f32, 2.0] {
        for tool in MASK_DETECT_TOOLS {
            let mut app = mask_detect_fixture(tool);
            app.open_mask_detect_dialog(tool);
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            ctx.set_pixels_per_point(scale);
            // 1093x614 physical pixels, with the available points reduced at 200%.
            let screen =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0) / scale);
            for _ in 0..4 {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        app.show_mask_detect_dialog(ui.ctx());
                    },
                )
                .drop_without_applying_deltas();
            }
            let window = ctx
                .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_window")))
                .expect("dialog was drawn");
            let actions = ctx
                .data(|data| {
                    data.get_temp::<Vec<egui::Rect>>(egui::Id::unique("mask_detect_actions"))
                })
                .expect("action row was drawn");
            assert_eq!(actions.len(), 2);
            assert!(
                screen.expand(1.0 / scale).contains_rect(window),
                "tool={tool:?}, scale={scale}, window={window:?}, screen={screen:?}"
            );
            for action in actions {
                assert!(action.is_positive());
                assert!(
                    window.expand(1.0 / scale).contains_rect(action),
                    "tool={tool:?}, scale={scale}, action={action:?}, window={window:?}"
                );
                assert!(
                    screen.contains_rect(action),
                    "action must remain visible at scale={scale}"
                );
            }
        }
    }
}
