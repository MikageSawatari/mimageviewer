use super::tests::{
    mask_detect_fixture, mask_detect_install_source, mask_detect_output, mask_detect_queue,
};
use super::*;
use crate::displayed_image_transform::{
    DisplayedImageTransform, DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit,
    ResolvedDisplayPlacement,
};

fn transform() -> DisplayedImageTransform {
    DisplayedImageTransform::from_resolved_rect(
        DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0)),
            source_size: egui::vec2(8.0, 8.0),
            texture_size: egui::vec2(8.0, 8.0),
            rotation: crate::rotation_db::Rotation::None,
            free_rotation_rad: 0.0,
            content_bbox: None,
            fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                crate::settings::FullscreenFitMode::Page,
            ),
            fit_scale_limits: FullscreenFitScaleLimits::default(),
            pixels_per_point: 1.0,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            pixel_fit: RectPixelFit::Texels,
        },
        egui::Rect::from_min_size(egui::pos2(600.0, 200.0), egui::vec2(200.0, 200.0)),
    )
    .unwrap()
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

fn mask_detect_append_receipt(
    app: &crate::app::AppTestEnvForTest,
    operation_id: u64,
) -> crate::book_append_undo::BookAppendUndoReceipt {
    let root = app.tmp.path().join("append-books");
    let folder = root.join("book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder =
        crate::book_append_undo::AppendReceiptBuilder::begin(&root, &folder, "book".into())
            .unwrap();
    // An append receipt requires at least one output recorded after begin().
    let added = folder.join("0001_added.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([70, 100, 130, 255]))
        .save(&added)
        .unwrap();
    builder.record(&added).unwrap();
    let files = builder.finish().unwrap();
    files.verify_current().unwrap();
    assert_eq!(files.created.len(), 1);
    crate::book_append_undo::BookAppendUndoReceipt {
        files,
        operation_id,
        source_context: app.projected_viewer_context_id(),
        accepted_revision: app.book_append_undo_revision,
    }
}

#[test]
fn mask_detect_modal_blocks_tool_keys_raw_canvas_and_both_local_panels() {
    let _input = crate::key_input::lock_test_input();
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for pending in [false, true] {
            let mut app = mask_detect_fixture(tool);
            app.erase_tool = EraseTool::Brush;
            app.conceal_tool = crate::conceal::ConcealTool::Brush;
            app.local_adjust_mask_tool = LocalAdjustMaskTool::Brush;
            if pending {
                let request = app.capture_mask_detect_request(tool).unwrap();
                mask_detect_queue(&mut app, request, mask_detect_output());
            } else {
                app.open_mask_detect_dialog(tool);
            }
            let before = app.local_adjust_page_layers.get(&0).cloned();
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let transform = transform();
            for pressed in [true, false] {
                ctx.run_ui(
                    egui::RawInput {
                        focused: true,
                        screen_rect: Some(transform.viewport_rect),
                        events: vec![
                            key(egui::Key::K),
                            key(egui::Key::Escape),
                            egui::Event::PointerMoved(egui::pos2(680.0, 280.0)),
                            egui::Event::PointerButton {
                                pos: egui::pos2(680.0, 280.0),
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                        ..Default::default()
                    },
                    |ui| {
                        app.keyboard_owner_for_pass(&ctx);
                        match tool {
                            MaskDetectTool::Erase => {
                                app.handle_erase_keys(&ctx, 0);
                                app.handle_erase_paint(&ctx, &transform, Default::default());
                                app.draw_erase_overlay(ui, &ctx, &transform);
                            }
                            MaskDetectTool::Conceal => {
                                app.handle_conceal_keys(&ctx, 0);
                                app.handle_conceal_paint(&ctx, &transform, Default::default());
                                app.draw_conceal_panel(&ctx, transform.viewport_rect);
                            }
                            MaskDetectTool::LocalAdjust => {
                                app.handle_fs_key_input(&ctx, 0, false);
                                app.handle_local_adjust_canvas_input(
                                    &ctx,
                                    transform.viewport_rect,
                                    &transform,
                                    Default::default(),
                                );
                                app.draw_local_adjust_panel(&ctx, transform.viewport_rect);
                            }
                        }
                        assert!(
                            ctx.input(|i| i.events.iter().any(|e| matches!(
                                e,
                                egui::Event::Key {
                                    key: egui::Key::K,
                                    ..
                                }
                            ))),
                            "modal must not consume LaToolBucket or other tool keys"
                        );
                    },
                )
                .drop_without_applying_deltas();
            }
            let panel_ids: &[&str] = match tool {
                MaskDetectTool::Erase => &["mask_detect_erase_panel_enabled"],
                MaskDetectTool::Conceal => &["mask_detect_conceal_panel_enabled"],
                MaskDetectTool::LocalAdjust => &[
                    "mask_detect_local_left_panel_enabled",
                    "mask_detect_local_right_panel_enabled",
                ],
            };
            for id in panel_ids {
                assert_eq!(
                    ctx.data(|d| d.get_temp::<bool>(egui::Id::new(id))),
                    Some(false),
                    "panel {id} must be disabled"
                );
            }
            assert_eq!(app.erase_tool, EraseTool::Brush);
            assert_eq!(app.conceal_tool, crate::conceal::ConcealTool::Brush);
            assert_eq!(app.local_adjust_mask_tool, LocalAdjustMaskTool::Brush);
            assert!(app.erase_mask.as_ref().unwrap().iter().all(|v| !v));
            assert!(app.conceal_mask.as_ref().unwrap().iter().all(|v| !v));
            assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());
            assert_eq!(app.local_adjust_page_layers.get(&0), before.as_ref());
            assert!(
                app.mask_detect_active(),
                "mode-exit key must not dismiss detection"
            );
        }
    }
}

#[test]
fn mask_detect_eye_preview_rejects_start_but_keeps_existing_hold_until_release() {
    let _input = crate::key_input::lock_test_input();
    for tool in [MaskDetectTool::Erase, MaskDetectTool::Conceal] {
        for active_before in [false, true] {
            let mut app = mask_detect_fixture(tool);
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let transform = transform();
            // A ready cached erase preview avoids invoking inference in this input test.
            let pixels = Arc::clone(&app.erase_base_cache[&0].pixels);
            let basis = app.erase_base_cache[&0].display_basis.clone();
            app.erase_preview_cache.insert(
                0,
                ErasePreviewCacheEntry {
                    display_basis: basis,
                    pixels: Arc::clone(&pixels),
                    texture: ctx.load_texture(
                        "held-preview",
                        pixels.as_ref().clone(),
                        Default::default(),
                    ),
                },
            );
            let frame = |app: &mut App, events: Vec<egui::Event>| {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(transform.viewport_rect),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        app.keyboard_owner_for_pass(&ctx);
                        if tool == MaskDetectTool::Conceal {
                            app.sync_conceal_preview_before_pipeline(&ctx, egui::ViewportId::ROOT);
                            app.draw_conceal_panel(&ctx, transform.viewport_rect);
                        } else {
                            app.handle_erase_paint(&ctx, &transform, Default::default());
                            app.draw_erase_overlay(ui, &ctx, &transform);
                        }
                    },
                )
                .drop_without_applying_deltas();
            };
            for _ in 0..4 {
                frame(&mut app, Vec::new());
            }
            let eye = if tool == MaskDetectTool::Conceal {
                app.conceal_preview_button_rect.unwrap()
            } else {
                ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("mask_detect_erase_eye_rect")))
                    .unwrap()
            };
            let pointer = |pressed| {
                vec![
                    egui::Event::PointerMoved(eye.center()),
                    egui::Event::PointerButton {
                        pos: eye.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ]
            };
            if active_before {
                frame(&mut app, pointer(true));
                assert!(if tool == MaskDetectTool::Conceal {
                    app.conceal_preview_active
                } else {
                    app.erase_preview_active
                });
            }
            app.open_mask_detect_dialog(tool);
            assert!(
                app.mask_detect_active(),
                "fixture must enter the detection modal: {tool:?}"
            );
            if active_before {
                // The eye owns an accepted press through movement outside its rect.
                frame(
                    &mut app,
                    vec![egui::Event::PointerMoved(
                        eye.center() + egui::vec2(300.0, 100.0),
                    )],
                );
            } else {
                frame(&mut app, pointer(true));
            }
            assert_eq!(
                if tool == MaskDetectTool::Conceal {
                    app.conceal_preview_active
                } else {
                    app.erase_preview_active
                },
                active_before
            );
            frame(&mut app, pointer(false));
            assert!(!app.conceal_preview_active && !app.erase_preview_active);
            assert!(app.erase_inpaint_pending.is_empty());
        }
    }
}

#[test]
fn mask_detect_mode_exit_and_index_invalidation_cancel_only_owned_request() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for indices_changed in [false, true] {
            let mut app = mask_detect_fixture(tool);
            let request = app.capture_mask_detect_request(tool).unwrap();
            let cancel = mask_detect_queue(&mut app, request, mask_detect_output());
            if indices_changed {
                app.invalidate_idx_state_and_queues();
            } else {
                match tool {
                    MaskDetectTool::Erase => app.reset_erase_mode(),
                    MaskDetectTool::Conceal => app.reset_conceal_mode(),
                    MaskDetectTool::LocalAdjust => app.close_fullscreen_content(),
                }
            }
            assert!(cancel.load(Ordering::Acquire));
            assert!(app.mask_detect_pending.is_none());
            assert!(!app.mask_detect_active());
        }
    }
}

#[test]
fn mask_detect_cancel_keeps_inference_activity_until_worker_really_finishes() {
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    let request = app
        .capture_mask_detect_request(MaskDetectTool::Erase)
        .unwrap();
    let (entered, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let activity = Arc::clone(&app.local_ai_activity);
    let before = activity.load(Ordering::Acquire);
    app.launch_mask_detect_worker(request, move |_| {
        let _ = entered.send(());
        // Simulate an in-flight synchronous inference that cannot be interrupted.
        let _ = wait.recv();
        Err(MaskDetectError::Detect(DetectError::Cancelled))
    });
    ready
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    app.cancel_mask_detect();
    assert!(!app.mask_detect_active());
    assert_eq!(activity.load(Ordering::Acquire), before + 1);
    assert!(!app.local_ai_remote_barrier_snapshot().is_quiesced());
    drop(release);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while activity.load(Ordering::Acquire) != before {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(app.erase_shapes.is_empty());
}

#[test]
fn mask_detect_remote_and_append_undo_wait_reject_all_three_tools() {
    for remote in [false, true] {
        for tool in [
            MaskDetectTool::Erase,
            MaskDetectTool::Conceal,
            MaskDetectTool::LocalAdjust,
        ] {
            let mut app = mask_detect_fixture(tool);
            if remote {
                let handle = crate::remote_ipc::session::SessionHandle::new();
                app.set_remote_session_handle(handle.clone());
                let _ = handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
                    client_id: "mask-detect-test".into(),
                    peer: mimageviewer_ipc::SessionPeerInfo {
                        connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                        device_name: Some("test".into()),
                    },
                });
                assert!(app.remote_session_blocks_local_control());
            } else {
                let receipt = mask_detect_append_receipt(&app, 1);
                let (_tx, rx) = mpsc::channel();
                app.book_op_pending = Some(crate::books::BookOpPending {
                    rx,
                    shelf_scope: None,
                    intent: crate::books::BookOpIntent::UndoAppend { receipt },
                });
            }
            assert!(!app.mask_detect_available(tool));
            app.open_mask_detect_dialog(tool);
            assert!(!app.mask_detect_active());
            assert!(app.capture_mask_detect_request(tool).is_none());
            assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());
        }
    }
}

#[test]
fn mask_detect_settings_roundtrip_and_missing_key_defaults_are_shared() {
    let mut settings = crate::settings::Settings::default();
    settings.mask_detect.ellipse = false;
    settings.mask_detect.margin_percent = 27.0;
    settings.mask_detect.threshold = 0.7;
    let mut stored = serde_json::to_value(&settings).unwrap();
    let restored: crate::settings::Settings = serde_json::from_value(stored.clone()).unwrap();
    assert!(!restored.mask_detect.ellipse);
    assert_eq!(restored.mask_detect.margin_percent, 27.0);
    assert_eq!(restored.mask_detect.threshold, 0.7);
    stored.as_object_mut().unwrap().remove("mask_detect");
    let defaults: crate::settings::Settings = serde_json::from_value(stored).unwrap();
    assert!(defaults.mask_detect.ellipse);
    assert_eq!(defaults.mask_detect.margin_percent, 15.0);
    assert_eq!(defaults.mask_detect.threshold, 0.6);
}

#[test]
fn mask_detect_margin_geometry_and_paint_operation_are_independent_of_tool_erase_toggle() {
    for ellipse in [false, true] {
        let options = crate::settings::MaskDetectSettings {
            ellipse,
            ..Default::default()
        };
        let shape = mask_detect_boxes(
            vec![Detection {
                class_idx: 0,
                score: 0.9,
                rect: [2.0, 2.0, 6.0, 6.0],
            }],
            &options,
        )
        .boxes
        .pop()
        .unwrap()
        .into_shape();
        assert_eq!(shape.center(), (4.0, 4.0));
        match shape {
            crate::mask_db::Shape::Rect {
                half_w,
                half_h,
                rotation_rad,
                ..
            } => {
                assert!(!ellipse);
                assert!((half_w - 2.6).abs() < 0.0001 && (half_h - 2.6).abs() < 0.0001);
                assert_eq!(rotation_rad, 0.0);
            }
            crate::mask_db::Shape::Ellipse {
                rx,
                ry,
                rotation_rad,
                ..
            } => {
                assert!(ellipse);
                assert!((rx - 2.6).abs() < 0.0001 && (ry - 2.6).abs() < 0.0001);
                assert_eq!(rotation_rad, 0.0);
            }
            _ => panic!("box geometry"),
        }
    }
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        let mut app = mask_detect_fixture(tool);
        app.erase_paint_mode = false;
        app.conceal_paint_mode = false;
        app.local_adjust_mask_paint_add = false;
        let request = app.capture_mask_detect_request(tool).unwrap();
        app.apply_mask_detect_output(&request, mask_detect_output());
        match tool {
            MaskDetectTool::Erase => {
                assert_eq!(app.erase_shapes[0].op(), crate::mask_db::ShapeOp::Add)
            }
            MaskDetectTool::Conceal => {
                assert_eq!(app.conceal_shapes[0].op(), crate::mask_db::ShapeOp::Add)
            }
            MaskDetectTool::LocalAdjust => {
                let local_adjust_core::LocalMask::RasterVector(mask) =
                    &app.local_adjust_page_layers[&0][0].mask
                else {
                    panic!("vector target");
                };
                assert_eq!(mask.shapes[0].op(), local_adjust_core::ShapeOp::Add);
            }
        }
    }
}

#[test]
fn mask_detect_addition_invalidates_only_the_target_page_cache() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        let mut app = mask_detect_fixture(tool);
        let request = app.capture_mask_detect_request(tool).unwrap();
        let ctx = egui::Context::default();
        let pixels = Arc::clone(&request.source);
        let basis = crate::material_display_basis::MaterialDisplayBasis::from_sampler(
            [8, 8],
            crate::source_coverage::SourceCoverage::FullSource,
        );
        let local_keys = [
            app.current_local_adjust_key(0),
            app.current_local_adjust_key(1),
        ];
        for idx in [0, 1] {
            match tool {
                MaskDetectTool::Erase => {
                    app.erase_preview_cache.insert(
                        idx,
                        ErasePreviewCacheEntry {
                            display_basis: basis.clone(),
                            pixels: Arc::clone(&pixels),
                            texture: ctx.load_texture(
                                format!("mask-detect-cache-{idx}"),
                                pixels.as_ref().clone(),
                                Default::default(),
                            ),
                        },
                    );
                }
                MaskDetectTool::Conceal => {
                    let generation = app.conceal_generation;
                    app.conceal_cache.insert(
                        idx,
                        ConcealCacheEntry {
                            texture_options: Default::default(),
                            display_basis: basis.clone(),
                            pixels: Arc::clone(&pixels),
                            texture: None,
                            generation,
                        },
                    );
                }
                MaskDetectTool::LocalAdjust => {
                    app.local_adjust_cache.insert(
                        local_keys[idx],
                        LocalAdjustCacheEntry {
                            texture_options: Default::default(),
                            display_basis: basis.clone(),
                            pixels: Arc::clone(&pixels),
                            texture: None,
                        },
                    );
                }
            }
        }
        let generation = app.conceal_generation;
        app.apply_mask_detect_output(&request, mask_detect_output());
        match tool {
            MaskDetectTool::Erase => assert!(
                !app.erase_preview_cache.contains_key(&0)
                    && app.erase_preview_cache.contains_key(&1)
            ),
            MaskDetectTool::Conceal => {
                assert!(!app.conceal_cache.contains_key(&0) && app.conceal_cache.contains_key(&1))
            }
            MaskDetectTool::LocalAdjust => assert!(
                !app.local_adjust_cache.contains_key(&local_keys[0])
                    && app.local_adjust_cache.contains_key(&local_keys[1])
            ),
        }
        assert_eq!(app.conceal_generation, generation, "no global invalidation");
    }
}

#[test]
fn mask_detect_open_cancel_and_zero_keep_book_append_undo_receipt() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for zero in [false, true] {
            let mut app = mask_detect_fixture(tool);
            let revision = app.book_append_undo_revision;
            app.book_append_undo_receipt = Some(mask_detect_append_receipt(&app, 7));
            app.open_mask_detect_dialog(tool);
            assert!(app.book_append_undo_receipt.is_some());
            let request = app.capture_mask_detect_request(tool).unwrap();
            if zero {
                mask_detect_queue(
                    &mut app,
                    request,
                    mask_detect_boxes(Vec::new(), &Default::default()),
                );
                app.poll_mask_detect(&egui::Context::default());
            } else {
                mask_detect_queue(&mut app, request, mask_detect_output());
                app.cancel_mask_detect();
            }
            assert_eq!(app.book_append_undo_revision, revision);
            assert_eq!(
                app.book_append_undo_receipt.as_ref().unwrap().operation_id,
                7
            );
        }
    }
}

#[test]
fn mask_detect_expanded_dialog_keeps_actions_outside_scroll_at_small_screen() {
    use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
    let _input = crate::key_input::lock_test_input();
    for scale in [1.0_f32, 2.0] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        app.open_mask_detect_dialog(MaskDetectTool::Erase);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        ctx.set_pixels_per_point(scale);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0) / scale);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.show_mask_detect_dialog(ui.ctx());
                },
            )
            .drop_without_applying_deltas();
        };
        for _ in 0..4 {
            frame(&mut app, Vec::new());
        }
        let header = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("mask_detect_details")))
            .unwrap();
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(header.center()),
                    egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..12 {
            frame(&mut app, Vec::new());
        }
        assert_eq!(
            ctx.data(|d| d.get_temp::<bool>(egui::Id::new("mask_detect_details_open"))),
            Some(true)
        );
        let body = ctx
            .data(|d| d.get_temp::<ScrollLayout>(egui::Id::new("mask_detect_body_layout")))
            .unwrap();
        body.assert_height(HeightExpectation::Fits, screen, scale);
        let actions = ctx
            .data(|d| d.get_temp::<Vec<egui::Rect>>(egui::Id::new("mask_detect_actions")))
            .unwrap();
        let window = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("mask_detect_window")))
            .unwrap();
        for action in actions {
            assert!(
                screen.contains_rect(action),
                "scale={scale}, action={action:?}, screen={screen:?}"
            );
            assert!(
                window.contains_rect(action),
                "scale={scale}, action={action:?}, window={window:?}"
            );
            assert!(
                action.top() >= body.inner.bottom(),
                "scale={scale}, action={action:?}, allocated_body={body:?}"
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn mask_detect_parked_context_poll_and_close_leave_sibling_masks_unchanged() {
    for close in [false, true] {
        // Local documents really are bundle-owned. Erase/conceal drafts are
        // foreground scratch, and their real handoff is covered separately.
        let mut app = mask_detect_fixture(MaskDetectTool::LocalAdjust);
        let request = app
            .capture_mask_detect_request(MaskDetectTool::LocalAdjust)
            .unwrap();
        let owner_key = request.page_key.clone();
        let cancel = mask_detect_queue(&mut app, request, mask_detect_output());
        let owner = app.stash_mounted_and_start_fresh("mask_detect_test");
        let sibling = app.projected_viewer_context_id();
        assert_ne!(owner, sibling);
        let ctx = egui::Context::default();
        let (sibling_layers, sibling_cache_key, sibling_pixels) =
            mask_detect_install_sibling_local(&mut app, &ctx);
        let undo_before = app.meta_undo.undo_len();
        if close {
            app.retire_context(owner, "mask_detect_test", |_| ())
                .unwrap();
            assert!(cancel.load(Ordering::Acquire));
            assert!(app.with_owner_viewer_context(owner, |_| ()).is_none());
        } else {
            app.poll_all_page_edit_producers(&ctx);
            app.with_owner_viewer_context(owner, |owner_app| {
                assert!(owner_app.mask_detect_pending.is_none());
                let local_adjust_core::LocalMask::RasterVector(mask) =
                    &owner_app.local_adjust_page_layers[&0][0].mask
                else {
                    panic!("vector target");
                };
                assert_eq!(mask.shapes.len(), 1);
            })
            .unwrap();
        }
        app.poll_all_page_edit_producers(&ctx);
        assert_eq!(app.projected_viewer_context_id(), sibling);
        assert!(app.mask_detect_pending.is_none());
        assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());
        assert_eq!(app.local_adjust_page_layers[&0], sibling_layers);
        assert!(Arc::ptr_eq(
            &app.local_adjust_cache[&sibling_cache_key].pixels,
            &sibling_pixels
        ));
        assert_eq!(app.meta_undo.undo_len(), undo_before + usize::from(!close));
        if !close {
            mask_detect_assert_local_undo_target(&app, &owner_key);
        }
    }
}

#[cfg(windows)]
fn mask_detect_install_sibling_local(
    app: &mut crate::app::AppTestEnvForTest,
    ctx: &egui::Context,
) -> (
    local_adjust_core::LocalAdjustmentLayers,
    LocalAdjustResultKey,
    Arc<egui::ColorImage>,
) {
    let path = app.tmp.path().join("mask-detect-sibling.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([180, 90, 40, 255]))
        .save(&path)
        .unwrap();
    mask_detect_install_source(app, path, MaskDetectTool::LocalAdjust, ctx);
    let mut layers = app.local_adjust_page_layers[&0].clone();
    let layer = &mut Arc::make_mut(&mut layers)[0];
    layer.name = "unchanged sibling".into();
    let local_adjust_core::LocalMask::RasterVector(mask) = &mut layer.mask else {
        panic!("vector target");
    };
    mask.alpha_mut()[0] = 1.0;
    app.set_local_adjust_layers_for_idx_memory_only(0, layers.clone());
    let key = app.current_local_adjust_key(0);
    let pixels = app.current_local_adjust_source_pixels(0).unwrap();
    app.local_adjust_cache.insert(
        key,
        LocalAdjustCacheEntry {
            texture_options: Default::default(),
            display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                [8, 8],
                crate::source_coverage::SourceCoverage::FullSource,
            ),
            pixels: Arc::clone(&pixels),
            texture: None,
        },
    );
    (layers, key, pixels)
}

#[cfg(windows)]
fn mask_detect_assert_local_undo_target(app: &App, page_key: &str) {
    // The metadata Undo stack is shared, so assert the captured target rather
    // than incorrectly treating its length as sibling-owned state.
    let Some(crate::undo_stack::UndoEntry::LocalAdjustment { changes, .. }) =
        app.meta_undo.peek_undo()
    else {
        panic!("local detection Undo");
    };
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].target.page_key, page_key);
}

#[cfg(windows)]
#[test]
fn mask_detect_parked_empty_receiver_keeps_root_poll_awake_until_result() {
    let mut app = mask_detect_fixture(MaskDetectTool::LocalAdjust);
    let request = app
        .capture_mask_detect_request(MaskDetectTool::LocalAdjust)
        .unwrap();
    let owner_key = request.page_key.clone();
    let (tx, rx) = mpsc::channel();
    app.mask_detect_pending = Some(MaskDetectPending {
        request,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    });
    let owner = app.stash_mounted_and_start_fresh("mask_detect_wait_test");
    let sibling = app.projected_viewer_context_id();
    let ctx = egui::Context::default();
    let (sibling_layers, sibling_cache_key, sibling_pixels) =
        mask_detect_install_sibling_local(&mut app, &ctx);
    let undo_before = app.meta_undo.undo_len();
    for _ in 0..4 {
        crate::test_ui::headless_paint_output(ctx.run_pass(Default::default(), |_| {}))
            .drop_without_applying_deltas();
    }
    let output = crate::test_ui::headless_paint_output(ctx.run_pass(Default::default(), |ctx| {
        app.poll_all_page_edit_producers(ctx);
    }));
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
            <= std::time::Duration::from_millis(100)
    );
    output.drop_without_applying_deltas();
    tx.send(Ok(mask_detect_output())).ok().unwrap();
    app.poll_all_page_edit_producers(&ctx);
    app.with_owner_viewer_context(owner, |owner_app| {
        assert!(owner_app.mask_detect_pending.is_none());
        let local_adjust_core::LocalMask::RasterVector(mask) =
            &owner_app.local_adjust_page_layers[&0][0].mask
        else {
            panic!("vector target");
        };
        assert_eq!(mask.shapes.len(), 1);
    })
    .unwrap();
    assert_eq!(app.projected_viewer_context_id(), sibling);
    assert!(app.mask_detect_pending.is_none());
    assert_eq!(app.local_adjust_page_layers[&0], sibling_layers);
    assert!(Arc::ptr_eq(
        &app.local_adjust_cache[&sibling_cache_key].pixels,
        &sibling_pixels
    ));
    assert_eq!(app.meta_undo.undo_len(), undo_before + 1);
    mask_detect_assert_local_undo_target(&app, &owner_key);
}
