use super::*;
use crate::fs_exclusive_view::FsExclusiveView;

#[test]
fn depth_wait_overlay_repaint_stops_for_ready_failed_hidden_and_minimized() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    install_depth(&mut app);
    app.window_visible = true;
    let frame = |app: &App, minimized: bool, draw: bool| {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1093.0, 614.0),
            )),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .minimized = Some(minimized);
        let output = ctx.run_ui(input, |ui| {
            if draw {
                app.draw_depth_wait_indicator(
                    ui,
                    ui.max_rect(),
                    &ctx,
                    crate::app::ActionSurface::Viewer,
                );
            }
        });
        let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        output.drop_without_applying_deltas();
        delay
    };
    for _ in 0..8 {
        frame(&app, false, false);
    }
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.panel_visible = false;
    state.wait_started_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
    assert_eq!(
        frame(&app, false, true),
        std::time::Duration::from_millis(33)
    );
    app.window_visible = false;
    assert!(frame(&app, false, true) > std::time::Duration::from_secs(1));
    app.window_visible = true;
    assert!(frame(&app, true, true) > std::time::Duration::from_secs(1));
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.wait_started_at = std::time::Instant::now() + std::time::Duration::from_secs(60);
    assert_eq!(
        frame(&app, false, true),
        std::time::Duration::from_millis(150),
        "only the gate wake before visibility"
    );
    let key = crate::depth_estimation::DepthSourceKey {
        texture_id: egui::TextureId::Managed(784),
    };
    for failed in [false, true] {
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.source = if failed {
            crate::depth_parallax::DepthSourcePhase::Failed {
                key,
                reason: "fixture".into(),
            }
        } else {
            crate::depth_parallax::DepthSourcePhase::Ready { key }
        };
        assert!(
            frame(&app, false, true) > std::time::Duration::from_secs(1),
            "terminal phase cannot animate"
        );
    }
    app.set_fs_exclusive_view(FsExclusiveView::None, "depth_wait_repaint_test_off");
    assert!(frame(&app, false, true) > std::time::Duration::from_secs(1));
}

#[test]
fn depth_annotation_bake_wait_overrides_an_otherwise_complete_2d_base() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::Image("depth-annotation.png".into())];
    app.fullscreen_idx = Some(0);
    let source_id = crate::app::source_texture_test_support::seed(&mut app, &ctx, 0);
    assert!(app.ensure_final_composite_material(&ctx, 0).is_some());
    assert!(app.current_final_composite_is_complete(0));
    let key = app.page_path_key(0).unwrap();
    app.comic_docs.insert(
        key,
        vec![comic_core::AnnotationObject::new_text(
            1,
            (0.0, 0.0),
            comic_core::TextBlock::default(),
        )],
    );
    assert!(app.comic_has_objects(&ctx, 0));
    // Occupied bake slots keep the actual producer pending without starting a
    // font lookup or a worker. The resolver still selects the complete 2D base.
    app.comic_bake_inflight
        .store(2, std::sync::atomic::Ordering::Relaxed);
    let (payload, material) = app.resolve_fs_processed_payload_and_depth(&ctx, 0, false);
    app.comic_bake_inflight
        .store(0, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(payload.unwrap().source_texture_id(), source_id);
    assert!(app.current_final_composite_is_complete(0));
    assert!(matches!(
        material,
        crate::depth_parallax::DepthSourceMaterial::NotYet(
            crate::depth_parallax::DepthSourceWait::AnnotationBake
        )
    ));
}

fn install_depth(app: &mut App) {
    app.items = vec![GridItem::Image("depth-input.jpg".into())];
    app.fullscreen_idx = Some(0);
    app.set_fs_exclusive_view(
        FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(
            app.edit_request_owner_context(),
        )),
        "depth_input_test",
    );
}

#[test]
fn depth_suspends_effective_zoom_consumers_while_2d_and_panorama_keep_their_owner() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::Image("depth-zoom-owner.jpg".into())];
    app.fullscreen_idx = Some(0);
    let pixels = Arc::new(egui::ColorImage::filled([4, 2], egui::Color32::WHITE));
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_metadata: None,
            pdf_rerender_failures: Vec::new(),
            tex: ctx.load_texture("depth-zoom-owner", pixels.clone(), Default::default()),
            pixels,
            source_dims: Some([4, 2]),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    for kind in ["2d", "panorama", "depth"] {
        let target = match kind {
            "depth" => FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(
                app.edit_request_owner_context(),
            )),
            "panorama" => FsExclusiveView::Panorama(crate::panorama::PanoramaState::new(
                0.0,
                0.0,
                crate::panorama::PanoProjection::Perspective,
            )),
            _ => FsExclusiveView::None,
        };
        app.set_fs_exclusive_view(target, "effective_zoom_test");
        for (active, aiming) in [(false, false), (true, false), (false, true)] {
            app.fs_zoom_active = active;
            app.fs_zoom_aiming = aiming;
            let stored = active || aiming;
            assert_eq!(app.fs_zoom_mode_engaged(), stored);
            assert_eq!(app.fs_zoom_mode_effective(), stored && kind != "depth");
            assert_eq!(app.fs_zoom_mode_active(), active && kind != "depth");
            assert_eq!(app.fs_zoom_mode_aiming(), aiming && kind != "depth");
            assert_eq!(
                app.fs_zoom_owner(),
                if kind == "depth" {
                    FsZoomOwner::Depth
                } else if stored {
                    FsZoomOwner::ZoomMode
                } else if kind == "panorama" {
                    FsZoomOwner::Panorama
                } else {
                    FsZoomOwner::Image
                }
            );
            if kind == "depth" {
                app.update_fullscreen_cursor_visibility(&ctx, ctx.viewport_id(), true, true, false);
                assert_ne!(app.cursor_hide_reason, Some(FsCursorHide::ZoomAiming));
                assert_eq!((app.fs_zoom_active, app.fs_zoom_aiming), (active, aiming));
            }
        }
    }
}

#[test]
fn depth_excludes_an_otherwise_eligible_single_still_from_page_turn_animation() {
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::Image("depth-page-turn.jpg".into())];
    app.fullscreen_idx = Some(0);
    app.settings.page_turn_animation_enabled = true;
    assert!(
        app.page_turn_view_eligible(0),
        "fixture must be eligible in ordinary 2D"
    );
    install_depth(&mut app);
    assert!(!app.page_turn_view_eligible(0));
}

#[test]
fn depth_typed_zoom_owner_stops_all_transform_inputs_and_navigator() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    install_depth(&mut app);
    app.fs_zoom = 2.4;
    app.fs_pan = egui::vec2(31.0, -7.0);
    app.fs_free_rotation = 0.3;
    app.settings.fullscreen_navigator_visible = true;
    assert_eq!(app.fs_zoom_owner(), FsZoomOwner::Depth);
    assert!(!app.apply_fs_key_zoom_step(&ctx, true));
    assert!(!app.apply_touch_zoom_factor_about_pivot(
        2.0,
        egui::pos2(30.0, 40.0),
        egui::Pos2::ZERO
    ));
    app.apply_touch_pan_delta(egui::vec2(70.0, 50.0), egui::Rect::EVERYTHING);
    assert!(!app.apply_gamepad_fullscreen_zoom(1.0, 0.2));
    assert!(!app.apply_gamepad_fullscreen_pan(egui::vec2(20.0, 20.0)));
    assert!(!app.fs_navigator_allowed_for_display_unit(&ctx, 0, false));
    assert!(!app.page_turn_view_eligible(0));
    let fit = app.settings.fullscreen_fit_mode;
    app.cycle_fullscreen_fit_mode(&ctx, 0);
    assert_eq!(app.settings.fullscreen_fit_mode, fit);
    assert_eq!(
        (app.fs_zoom, app.fs_pan, app.fs_free_rotation),
        (2.4, egui::vec2(31.0, -7.0), 0.3)
    );
}

#[test]
fn depth_wheel_navigates_and_ctrl_wheel_never_uses_panorama_fov_or_hidden_zoom() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    install_depth(&mut app);
    // Retained Z state must not steal ordinary wheel navigation.
    app.fs_zoom_aiming = true;
    app.fs_zoom_active = true;
    for ctrl in [false, true] {
        let input = crate::wheel_input::WheelInput {
            delta: egui::vec2(0.0, -120.0),
            modifiers: egui::Modifiers {
                ctrl,
                ..Default::default()
            },
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        };
        let action = FsWheelAction {
            input,
            segment: crate::wheel_input::WheelSegment {
                input,
                first_raw_ordinal: None,
                last_raw_ordinal: None,
            },
            operation: None,
        };
        assert_eq!(app.fs_wheel_action_requests_navigation(&action), !ctrl);
        assert!(!app.handle_panorama_wheel_if_active(&ctx, -120.0, ctrl));
    }
}

#[test]
fn depth_material_waits_for_original_hold_instead_of_estimating_preview() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::Image("depth-no-source.jpg".into())];
    let (payload, material) = app.resolve_fs_processed_payload_and_depth(&ctx, 0, true);
    assert!(payload.is_none());
    assert!(matches!(
        material,
        crate::depth_parallax::DepthSourceMaterial::NotYet(
            crate::depth_parallax::DepthSourceWait::OriginalHold
        )
    ));
}

#[test]
fn depth_entry_stops_edit_tools_with_its_own_reason() {
    let mut app = crate::app::setup_app_for_test();
    install_depth(&mut app);
    assert!(!app.fullscreen_edit_mode_entry_allowed(0));
    assert!(!app.erase_mode && !app.local_adjust_mode && !app.text_mode);
}

#[test]
fn depth_none_viewer_pass_retires_owner_before_deferred_or_closed_early_return() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    install_depth(&mut app);
    app.fullscreen_idx = None;
    ctx.run_pass(Default::default(), |ctx| {
        ctx.root_ui(|ui| app.render_fullscreen_viewport(ui));
    })
    .drop_without_applying_deltas();
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    app.fullscreen_idx = Some(0);
    assert!(!app.depth_view_may_continue(0));
}

#[test]
fn depth_image_key_does_not_consume_video_or_audio_scope() {
    let _guard = crate::key_input::lock_test_input();
    for item in [
        GridItem::Video("depth-video.mp4".into()),
        GridItem::Audio("depth-audio.mp3".into()),
    ] {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![item];
        app.fullscreen_idx = Some(0);
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::D,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        app.handle_fs_key_input(&ctx, 0, false);
        assert!(app.keymap.consume_action(&ctx, KeyAction::FsDepthParallax));
        ctx.end_pass().drop_without_applying_deltas();
        assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_pending_final_edit_keeps_raw_2d_fallback_out_of_inference() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::Image("depth-pending.jpg".into())];
    app.fullscreen_idx = Some(0);
    let pixels = Arc::new(egui::ColorImage::filled([2, 3], egui::Color32::WHITE));
    let texture = ctx.load_texture("depth-pending-raw", pixels.clone(), Default::default());
    app.bind_display_texture_identity(&texture, app.item_id(0));
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_metadata: None,
            pdf_rerender_failures: vec![],
            tex: texture.clone(),
            pixels,
            source_dims: Some([2, 3]),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.local_adjust_pages.insert(0);
    app.local_adjust_page_layers.insert(
        0,
        local_adjust_core::LocalAdjustmentLayers::new(vec![
            local_adjust_core::LocalAdjustmentLayer::new(
                "depth-pending",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
            ),
        ]),
    );
    let request = crate::app::EditMaterializeRequest {
        items_generation: app.items_generation,
        input_seq: app.input_seq,
        kind: crate::app::EditMaterializeKind::Local {
            key: app.current_local_adjust_key(0),
            continuation: crate::app::LocalMaterializeContinuation::Display,
        },
    };
    let (_sender, rx) = std::sync::mpsc::channel();
    app.local_adjust_pending.insert(
        0,
        crate::app::LocalAdjustRenderPending {
            request,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rx,
            ready: None,
        },
    );
    let (payload, material) = app.resolve_fs_processed_payload_and_depth(&ctx, 0, false);
    assert_eq!(payload.unwrap().source_texture_id(), texture.id());
    assert!(matches!(
        material,
        crate::depth_parallax::DepthSourceMaterial::NotYet(
            crate::depth_parallax::DepthSourceWait::IncompleteComposite
        )
    ));
}

#[test]
fn depth_raw_development_preview_is_stable_until_replaced_by_a_new_texture() {
    let ctx = egui::Context::default();
    let mut app = crate::app::raw_page_store::tests::app_with_raw_and_jpeg();
    let pixels = Arc::new(egui::ColorImage::filled([6, 4], egui::Color32::WHITE));
    let texture = ctx.load_texture("depth-raw-preview", pixels.clone(), Default::default());
    app.bind_display_texture_identity(&texture, app.item_id(0));
    app.fs_cache.insert(
        0,
        FsCacheEntry::RawPreview {
            preview: Some(crate::fs_animation::RawPreviewTexture {
                tex: texture.clone(),
                pixels,
            }),
            developed_dims: [12000, 8000],
            load_seq: 1,
        },
    );
    let page = app.raw_pages.page_mut(0).unwrap();
    page.stage = crate::app::raw_page_store::RawInstalledStage::PreviewShown;
    page.preview = crate::app::raw_page_store::RawPreviewPhase::Done;
    app.fullscreen_idx = Some(0);
    let (payload, material) = app.resolve_fs_processed_payload_and_depth(&ctx, 0, false);
    assert_eq!(payload.unwrap().source_texture_id(), texture.id());
    assert!(
        matches!(material, crate::depth_parallax::DepthSourceMaterial::Ready { texture: selected } if selected.id() == texture.id())
    );
}
