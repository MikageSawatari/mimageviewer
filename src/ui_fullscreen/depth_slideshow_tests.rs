//! Depth slideshow input, timer ownership and installed destination contracts.
use super::*;
use crate::depth_estimation::DepthSourceKey;
use crate::depth_parallax::{
    DepthNextPhase, DepthNextSlot, DepthSettled, DepthSettledOutcome, DepthSourcePhase,
    DepthViewState,
};
use crate::fs_exclusive_view::FsExclusiveView;
use crate::ring_shortcut::{RingActionId, RingShortcutContext};
use std::time::{Duration, Instant};

fn install_depth(app: &mut App) {
    app.set_fs_exclusive_view(
        FsExclusiveView::Depth(DepthViewState::new(app.edit_request_owner_context())),
        "depth_slideshow_test",
    );
}

#[test]
fn depth_slideshow_progress_ring_ignores_stale_bounds_when_panel_is_not_drawn() {
    let mut app = crate::app::setup_app_for_test();
    install_depth(&mut app);
    let ctx = egui::Context::default();
    let old = egui::Rect::from_min_size(egui::pos2(688.0, 60.0), egui::vec2(300.0, 400.0));
    ctx.data_mut(|data| data.insert_temp(app.depth_panel_rect_id(), old));
    assert_eq!(app.visible_depth_panel_rect(&ctx), Some(old));
    let panel = app.draw_depth_panel(&ctx, usize::MAX);
    assert!(panel.is_none());
    assert_eq!(app.visible_depth_panel_rect(&ctx), Some(old));
    app.slideshow_playing = true;
    let content = egui::Rect::from_min_max(egui::pos2(0.0, 44.0), egui::pos2(1000.0, 700.0));
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        app.draw_slideshow_progress_indicator(
            ui,
            content,
            panel.as_ref().map(|panel| panel.rect),
            &ctx,
        );
    });
    let centers: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Circle(circle) => Some(circle.center),
            _ => None,
        })
        .collect();
    assert_eq!(centers, vec![egui::pos2(978.0, 66.0)]);
    output.drop_without_applying_deltas();
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.panel_visible = false;
    assert_eq!(app.visible_depth_panel_rect(&ctx), None);
    app.set_fs_exclusive_view(FsExclusiveView::None, "depth_progress_panel_hidden_test");
    assert_eq!(app.visible_depth_panel_rect(&ctx), None);
}

fn enter_panorama(app: &mut App) {
    app.set_fs_exclusive_view(FsExclusiveView::None, "panorama_test");
    if let Some(FsCacheEntry::Static { source_dims, .. }) = app.fs_cache.get_mut(&0) {
        *source_dims = Some([240, 120]);
    }
    app.apply_panorama_mode_toggle(0);
    assert!(app.is_panorama_mode_active(0));
}

fn settle_failed(app: &mut App) {
    let idx = app.fullscreen_idx.unwrap();
    let edit_key = app.current_edit_result_key(idx);
    let key = DepthSourceKey {
        texture_id: egui::TextureId::Managed(7781),
    };
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.source = DepthSourcePhase::Failed {
        key,
        reason: "fixture".into(),
    };
    state.current_material = Some(key);
    state.settled = Some(DepthSettled {
        fs_idx: idx,
        edit_key,
        key,
        outcome: DepthSettledOutcome::Failed,
    });
}

fn arm_expired(app: &mut App) {
    app.slideshow_playing = true;
    app.slideshow_anchor_idx = app.fullscreen_idx;
    app.slideshow_next_at = Instant::now();
}

fn timer(app: &mut App, ctx: &egui::Context, pass: SlideshowTimerPass) {
    app.handle_fs_navigation_with_render_perf(
        ctx,
        false,
        false,
        None,
        None,
        None,
        FsPageNav::None,
        None,
        app.fullscreen_idx.unwrap(),
        None,
        None,
        pass,
        &mut None,
    );
}

fn key_s(app: &mut App, ctx: &egui::Context) {
    for pressed in [true, false] {
        ctx.run_pass(
            egui::RawInput {
                focused: true,
                events: vec![egui::Event::Key {
                    key: egui::Key::S,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                app.handle_fs_key_input(ctx, app.fullscreen_idx.unwrap(), false);
            },
        )
        .drop_without_applying_deltas();
    }
}

#[test]
fn depth_slideshow_conflict_cleanup_preserves_playback_and_panorama_entry_stops_it() {
    let ctx = egui::Context::default();
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    arm_expired(&mut app);
    let deadline = app.slideshow_next_at;
    install_depth(&mut app);
    app.close_fs_exclusive_conflicts();
    assert!(app.slideshow_playing);
    assert_eq!(app.slideshow_anchor_idx, Some(0));
    assert_eq!(app.slideshow_next_at, deadline);
    enter_panorama(&mut app);
    assert!(matches!(
        app.fs_exclusive_view,
        FsExclusiveView::Panorama(_)
    ));
    assert!(!app.slideshow_playing);
    assert_eq!(app.slideshow_anchor_idx, None);
}

#[test]
fn depth_slideshow_key_and_ring_allow_start_stop_and_panorama_key_stays_suppressed() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    app.keymap = Keymap::default();
    install_depth(&mut app);
    for ring in [false, true] {
        for playing in [true, false] {
            if ring {
                app.apply_ring_action(
                    &ctx,
                    RingShortcutContext::ImageFullscreen,
                    RingActionId::ImageSlideshow,
                    "depth_slideshow_test",
                );
            } else {
                key_s(&mut app, &ctx);
            }
            assert_eq!(app.slideshow_playing, playing, "ring={ring}");
        }
    }
    enter_panorama(&mut app);
    key_s(&mut app, &ctx);
    assert!(!app.slideshow_playing);
    app.apply_ring_action(
        &ctx,
        RingShortcutContext::ImageFullscreen,
        RingActionId::ImageSlideshow,
        "panorama_slideshow_test",
    );
    assert!(
        app.slideshow_playing,
        "Panorama retains its released ring behavior"
    );
}

#[test]
fn depth_slideshow_target_matches_d3_for_single_and_spread_singleton_with_video_gap() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        let mut app = page_turn_submission_tests::ready_app(&ctx, mode);
        let mut items = app.items.clone();
        items[1] = GridItem::Video("c:/page-turn/1.mp4".into());
        app.install_new_items(items, vec![None; 4]);
        app.visible_indices = vec![0, 1, 2, 3];
        for idx in [0, 2, 3] {
            crate::app::source_texture_test_support::seed(&mut app, &ctx, idx);
        }
        if mode != SpreadMode::Single {
            if let Some(FsCacheEntry::Static { source_dims, .. }) = app.fs_cache.get_mut(&0) {
                *source_dims = Some([120, 80]);
            }
            app.record_page_dims_for_spread(0, (120, 80));
        }
        install_depth(&mut app);
        app.slideshow_playing = true;
        app.settings.page_turn_animation_enabled = false;
        assert!(matches!(
            app.resolve_visible_spread_pair(0),
            SpreadPair::Single
        ));
        let still = app.get_still_image_indices();
        assert_eq!(still.as_slice(), &[0, 2, 3]);
        assert_eq!(
            app.slideshow_next_target(0),
            SlideshowNextTarget::InItems { idx: 2 }
        );
        let step = app.spread_page_nav_for_indices(&still, 0, 1);
        assert!(
            matches!(step, FsPageNav::Target(2) | FsPageNav::Delta(1)),
            "{mode:?}: {step:?}"
        );
        // Use the ordinary paint's composition entry, including Single with
        // curl disabled. D3 demand must not need the source-prefetch policy.
        app.spread_display_composition_for_anchor(0);
        app.prepare_final_demand_topologies();
        let next = app
            .next_final_prepare_occurrences(0)
            .expect("Depth owns D3 Next with paper curl off");
        assert_eq!(next.first().map(|page| page.idx), Some(2), "{mode:?}");
        assert_eq!(app.slideshow_next_target(3), SlideshowNextTarget::Other);
        app.navigation_scope = crate::app::ViewerNavigationScope::CollectionRoot;
        assert_eq!(app.slideshow_next_target(0), SlideshowNextTarget::Other);
        app.navigation_scope = crate::app::ViewerNavigationScope::Main;
        app.spread_mode = SpreadMode::SplitLtr;
        assert_eq!(app.slideshow_next_target(0), SlideshowNextTarget::Other);
    }
}

#[test]
fn depth_slideshow_hidden_sync_disarms_and_visible_terminal_page_restarts_interval() {
    let ctx = egui::Context::default();
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    install_depth(&mut app);
    settle_failed(&mut app);
    arm_expired(&mut app);
    let state = FsFrameState::overview_shell();
    let expired = app.slideshow_next_at;
    app.sync_slideshow_anchor_for_frame(&ctx, 0, &state, SlideshowTimerPass::InputOnly);
    assert_eq!(app.slideshow_anchor_idx, Some(0));
    assert_eq!(app.slideshow_next_at, expired);
    app.sync_slideshow_anchor_for_frame(
        &ctx,
        0,
        &state,
        SlideshowTimerPass::ViewerRender {
            viewport: egui::ViewportId::ROOT,
            visible: false,
        },
    );
    assert_eq!(app.slideshow_anchor_idx, None);
    let now = Instant::now();
    app.sync_slideshow_anchor_for_frame(
        &ctx,
        0,
        &state,
        SlideshowTimerPass::ViewerRender {
            viewport: egui::ViewportId::ROOT,
            visible: true,
        },
    );
    assert_eq!(app.slideshow_anchor_idx, Some(0));
    assert!(app.slideshow_next_at >= now + app.slideshow_interval_duration());
}

#[test]
fn depth_slideshow_stop_drops_the_context_owned_next_slot() {
    let ctx = egui::Context::default();
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    install_depth(&mut app);
    arm_expired(&mut app);
    let target = app.depth_next_target(1);
    let key = DepthSourceKey {
        texture_id: egui::TextureId::Managed(7782),
    };
    let (pending, _complete) = crate::depth_estimation::DepthPending::deferred_for_test(
        app.edit_request_owner_context(),
        771,
        key,
    );
    let cancelled = pending.cancellation_token_for_test();
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.next = Some(DepthNextSlot {
        started_at: Instant::now(),
        target,
        key,
        phase: DepthNextPhase::Estimating { pending },
    });
    assert!(app.stop_slideshow_playback());
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        unreachable!()
    };
    assert!(state.next.is_none());
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(app.slideshow_anchor_idx, None);
}

#[cfg(windows)]
mod gpu_tests {
    use super::*;
    use crate::app::depth_main_window_tests::RootDepthViewer;

    #[test]
    fn depth_slideshow_progress_ring_actual_draw_avoids_only_the_visible_panel() {
        fn collect_rings(shape: &egui::Shape, centers: &mut Vec<egui::Pos2>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_rings(shape, centers);
                    }
                }
                egui::Shape::Circle(circle)
                    if circle.radius == 8.0
                        && circle.stroke
                            == egui::Stroke::new(1.2, egui::Color32::from_white_alpha(42)) =>
                {
                    centers.push(circle.center);
                }
                _ => {}
            }
        }
        let _input = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        app.settings.fullscreen_fixed_bar_gap_px = 0;
        app.settings.slideshow_interval_secs = 3600.0;
        app.slideshow_playing = true;
        app.schedule_next_slideshow_from_now();
        for (locked, visible, collapsed) in [
            (true, true, false),
            (true, true, true),
            (true, false, true),
            (false, true, false),
            (false, false, false),
        ] {
            app.settings.fullscreen_top_bar_locked = locked;
            app.settings.depth_parallax.panel_collapsed = collapsed;
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            state.panel_visible = visible;
            viewer.frame(&mut app, vec![]);
            assert!(app.slideshow_playing);
            assert!(
                !viewer.callbacks().is_empty(),
                "the real 3D callback must be submitted"
            );
            let mut centers = vec![];
            for shape in &viewer.shapes {
                collect_rings(&shape.shape, &mut centers);
            }
            assert_eq!(centers.len(), 1, "one actual progress ring: {centers:?}");
            let center = centers[0];
            assert_eq!(center.y, if locked { 66.0 } else { 22.0 });
            let panel = app.visible_depth_panel_rect(&viewer.ctx);
            assert_eq!(panel.is_some(), visible);
            if let Some(panel) = panel {
                assert!(
                    !egui::Rect::from_center_size(center, egui::Vec2::splat(16.0))
                        .intersects(panel)
                );
                if locked {
                    assert_eq!(center.x, panel.left() - 12.0);
                }
            }
            if !locked || !visible {
                assert_eq!(center.x, viewer.size.x - 22.0);
            }
        }
    }

    fn assert_panorama_hover_bar_hides_playback() {
        // Test the production HUD directly: Panorama's canvas/resources are a
        // separate owner and have no role in slideshow button admission.
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let full_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(full_rect),
                ..Default::default()
            },
            |ui| {
                App::draw_fs_hover_bar(
                    ui,
                    &ctx,
                    &Keymap::default(),
                    full_rect,
                    &[],
                    &mut false,
                    &mut FsPageNav::None,
                    crate::settings::FsSidePanelMode::Hover,
                    &mut false,
                    false,
                    false,
                    &mut None,
                    false,
                    &mut false,
                    crate::rotation_db::Rotation::None,
                    &mut None,
                    Some(crate::panorama::PanoramaTrigger::Hint),
                    true,
                    true,
                    &mut false,
                    crate::panorama::PanoProjection::Perspective,
                    &mut false,
                    &mut None,
                    false,
                    crate::spread_shift::PairingEditAvailability::Automatic,
                    None,
                    &mut SpreadMode::Single,
                    &mut ReadingFlow::Paged,
                    &mut Default::default(),
                    &mut Default::default(),
                    &mut Default::default(),
                    false,
                    &mut Default::default(),
                    &mut false,
                    false,
                    &mut false,
                    false,
                    FullscreenFitMode::Page,
                    false,
                    false,
                    &mut false,
                    &mut crate::settings::FullscreenSpecifiedScale::default(),
                    &mut crate::settings::FULLSCREEN_SPECIFIED_SCALE_MAX_DEFAULT_TENTHS,
                    false,
                    &mut false,
                    &mut None,
                    &mut None,
                    &mut None,
                    &mut false,
                    false,
                    false,
                    None,
                    false,
                    &mut false,
                    false,
                    false,
                    &mut false,
                    &mut false,
                    &mut false,
                    false,
                    false,
                    &mut false,
                    true,
                    true,
                    &mut false,
                    false,
                    None,
                    false,
                    1,
                    None,
                    None,
                    None,
                );
            },
        );
        output.drop_without_applying_deltas();
        assert!(
            ctx.read_response(egui::Id::unique("fs_panorama_projection_btn"))
                .is_some(),
            "the locked Panorama HUD must actually render"
        );
        assert!(ctx.read_response(egui::Id::unique("fs_play_btn")).is_none());
        assert!(
            ctx.read_response(egui::Id::unique("fs_overflow_btn"))
                .is_none()
        );
    }

    fn prepare_next_backing(app: &mut App, ctx: &egui::Context, idx: usize) -> DepthSourceKey {
        let material = app.ensure_final_composite_material(ctx, idx).unwrap();
        let key = DepthSourceKey {
            texture_id: material.source_texture().id(),
        };
        let render = app.wgpu_render_state.clone().unwrap();
        let mut textures = ctx.tex_manager().write().take_delta();
        for (id, deltas) in &textures.set {
            for delta in deltas {
                render
                    .renderer
                    .write()
                    .update_texture(&render.device, &render.queue, *id, delta);
            }
        }
        for id in &textures.free {
            render.renderer.write().free_texture(id);
        }
        textures.clear();
        assert!(
            render
                .renderer
                .read()
                .texture(&key.texture_id)
                .is_some_and(|texture| texture.texture.is_some())
        );
        let resident = app.resident_final_prepare_material(ctx, idx).unwrap();
        assert_eq!(resident.source_texture().id(), key.texture_id);
        key
    }

    #[test]
    fn depth_slideshow_background_exclusion_skips_prefetch_and_default_background_resumes_it() {
        let _input = crate::key_input::lock_test_input();
        for background in [1, 2] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = RootDepthViewer::new(&mut app);
            viewer.ready(&mut app);
            viewer.frame(&mut app, vec![]);
            let key = prepare_next_backing(&mut app, &viewer.ctx, 2);
            app.settings.slideshow_interval_secs = 30.0;
            app.slideshow_playing = true;
            app.schedule_next_slideshow_from_now();
            app.fs_transparent_bg_mode = background;
            viewer.frame(&mut app, vec![]);
            assert!(app.depth_current_settled());
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                unreachable!()
            };
            assert!(state.next.is_none(), "background={background}");
            assert!(!state.lru.contains(key));
            assert_eq!(
                app.depth_slideshow_gate(&viewer.ctx, 2, Instant::now()),
                crate::app::depth_view::DepthSlideshowGate::Go
            );
            app.fs_transparent_bg_mode = 0;
            viewer.frame(&mut app, vec![]);
            assert_eq!(app.fullscreen_idx, Some(0));
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                unreachable!()
            };
            let next = state
                .next
                .as_ref()
                .expect("default background resumes real readback preparation");
            assert_eq!(next.target.idx, 2);
            assert_eq!(next.key, key);
        }
    }

    #[test]
    fn depth_slideshow_actual_export_operation_suppresses_prefetch_until_cancelled() {
        let _input = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        let output = viewer.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size)),
                focused: true,
                ..Default::default()
            },
            |ui| {
                app.render_fullscreen_viewport(ui);
                app.open_depth_export(ui.ctx(), 0);
                assert!(
                    app.depth_export_dialog_open(),
                    "the real painted export entry must be admitted"
                );
            },
        );
        output.drop_without_applying_deltas();
        let key = prepare_next_backing(&mut app, &viewer.ctx, 2);
        app.settings.slideshow_interval_secs = 30.0;
        app.slideshow_playing = true;
        app.schedule_next_slideshow_from_now();
        for _ in 0..2 {
            viewer.frame(&mut app, vec![]);
            assert!(app.depth_export_dialog_open());
            assert_eq!(app.fullscreen_idx, Some(0));
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                unreachable!()
            };
            assert!(state.next.is_none());
            assert!(!state.lru.contains(key));
            assert_eq!(
                app.depth_slideshow_gate(&viewer.ctx, 2, Instant::now()),
                crate::app::depth_view::DepthSlideshowGate::Go
            );
        }
        app.cancel_depth_export_for_fullscreen_close();
        assert!(!app.depth_export_dialog_open());
        viewer.frame(&mut app, vec![]);
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        let next = state
            .next
            .as_ref()
            .expect("leaving the real settings operation restores prefetch");
        assert_eq!(next.target.idx, 2);
        assert_eq!(next.key, key);
    }

    #[test]
    fn depth_slideshow_real_manual_navigation_sequence_hands_off_the_pending_next_request() {
        let _input = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        viewer.frame(&mut app, vec![]);
        let key = prepare_next_backing(&mut app, &viewer.ctx, 2);
        let target = app.depth_next_target(2);
        let context = app.edit_request_owner_context();
        let (pending, _complete) =
            crate::depth_estimation::DepthPending::deferred_for_test(context, 771, key);
        let cancelled = pending.cancellation_token_for_test();
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.next = Some(DepthNextSlot {
            target,
            key,
            phase: DepthNextPhase::Estimating { pending },
            started_at: Instant::now(),
        });
        app.settings.slideshow_interval_secs = 30.0;
        app.slideshow_playing = true;
        app.schedule_next_slideshow_from_now();
        app.handle_fs_navigation(
            &viewer.ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(2),
            None,
            0,
        );
        assert_eq!(app.fullscreen_idx, Some(2));
        assert!(
            app.fs_navigation_sequence_blocks_new_target(),
            "the real manual handler must create a navigation sequence"
        );
        assert!(!cancelled.load(std::sync::atomic::Ordering::Acquire));
        for _ in 0..3 {
            viewer.frame(&mut app, vec![]);
            assert_eq!(app.fullscreen_idx, Some(2));
            assert!(
                !cancelled.load(std::sync::atomic::Ordering::Acquire),
                "the live request survives real PageTransition passes"
            );
        }
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        assert!(state.next.is_none());
        assert!(
            matches!(&state.source, DepthSourcePhase::Estimating { pending, .. } if pending.request_id() == 771)
        );
    }

    #[test]
    fn depth_slideshow_hud_exposes_play_but_keeps_overflow_hidden() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        let ctx = viewer.ctx.clone();
        viewer.ready(&mut app);
        app.settings.fullscreen_top_bar_locked = true;
        app.settings.slideshow_interval_secs = 30.0;
        for playing in [true, false] {
            viewer.frame(&mut app, vec![]);
            assert!(
                ctx.read_response(egui::Id::unique("fs_overflow_btn"))
                    .is_none()
            );
            let pos = ctx
                .read_response(egui::Id::unique("fs_play_btn"))
                .expect("Depth exposes slideshow playback")
                .rect
                .center();
            viewer.frame(&mut app, vec![egui::Event::PointerMoved(pos)]);
            viewer.frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert!(
                app.fs_exclusive_view.is_depth(),
                "HUD test must retain real Depth eligibility"
            );
            assert_eq!(app.slideshow_playing, playing);
        }
        enter_panorama(&mut app);
        assert!(app.is_panorama_mode_active(0));
        assert_panorama_hover_bar_hides_playback();
    }

    #[test]
    fn depth_slideshow_expired_handler_waits_without_limit_then_advances_after_cancel_or_failure() {
        use crate::app::depth_view::{DepthPageProgress, DepthPageRole, DepthSlideshowGate};
        use crate::depth_estimation::DepthPending;
        for failed in [false, true] {
            let ctx = egui::Context::default();
            let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
            install_depth(&mut app);
            settle_failed(&mut app);
            let material = app.ensure_final_composite_material(&ctx, 1).unwrap();
            let key = DepthSourceKey {
                texture_id: material.source_texture().id(),
            };
            let context = app.edit_request_owner_context();
            let (pending, _complete) = DepthPending::deferred_for_test(context, 771, key);
            let target = app.depth_next_target(1);
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            state.next = Some(DepthNextSlot {
                started_at: Instant::now(),
                target,
                key,
                phase: DepthNextPhase::Estimating { pending },
            });
            arm_expired(&mut app);
            let deadline = app.slideshow_next_at;
            for overdue in [
                Duration::from_secs(1),
                Duration::from_secs(3600),
                Duration::from_secs(86_400),
            ] {
                assert_eq!(
                    app.depth_page_progress(&ctx, 1, DepthPageRole::Next),
                    DepthPageProgress::Producing
                );
                assert_eq!(
                    app.depth_slideshow_gate(&ctx, 1, Instant::now() + overdue),
                    DepthSlideshowGate::Wait
                );
                timer(
                    &mut app,
                    &ctx,
                    SlideshowTimerPass::ViewerRender {
                        viewport: egui::ViewportId::ROOT,
                        visible: true,
                    },
                );
                assert_eq!(app.fullscreen_idx, Some(0));
                assert_eq!(app.slideshow_next_at, deadline);
            }
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            let next = state.next.as_mut().unwrap();
            if failed {
                next.phase = DepthNextPhase::Failed {
                    reason: "fixture depth failure".into(),
                };
            } else {
                let DepthNextPhase::Estimating { pending } = &next.phase else {
                    unreachable!()
                };
                pending.cancel();
            }
            assert_eq!(
                app.depth_page_progress(&ctx, 1, DepthPageRole::Next),
                DepthPageProgress::Unavailable
            );
            assert_eq!(
                app.depth_slideshow_gate(&ctx, 1, Instant::now()),
                DepthSlideshowGate::Go
            );
            timer(
                &mut app,
                &ctx,
                SlideshowTimerPass::ViewerRender {
                    viewport: egui::ViewportId::ROOT,
                    visible: true,
                },
            );
            assert_eq!(app.fullscreen_idx, Some(1), "failed={failed}");
            assert!(app.slideshow_playing);
        }
    }

    #[test]
    fn depth_slideshow_unavailable_current_and_unissued_next_do_not_stall() {
        use crate::app::depth_view::{DepthPageProgress, DepthPageRole, DepthSlideshowGate};
        let ctx = egui::Context::default();
        let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        install_depth(&mut app);
        app.fs_cache.insert(0, FsCacheEntry::Failed);
        app.fs_cache.remove(&1);
        assert_eq!(
            app.depth_page_progress(&ctx, 0, DepthPageRole::Current),
            DepthPageProgress::Unavailable
        );
        assert_eq!(
            app.depth_page_progress(&ctx, 1, DepthPageRole::Next),
            DepthPageProgress::Unavailable
        );
        app.slideshow_playing = true;
        app.slideshow_anchor_idx = None;
        app.sync_slideshow_anchor_for_frame(
            &ctx,
            0,
            &FsFrameState::overview_shell(),
            SlideshowTimerPass::ViewerRender {
                viewport: egui::ViewportId::ROOT,
                visible: true,
            },
        );
        assert_eq!(
            app.slideshow_anchor_idx,
            Some(0),
            "terminal failure starts the interval"
        );
        assert_eq!(
            app.depth_slideshow_gate(&ctx, 1, Instant::now()),
            DepthSlideshowGate::Go
        );
        arm_expired(&mut app);
        timer(
            &mut app,
            &ctx,
            SlideshowTimerPass::ViewerRender {
                viewport: egui::ViewportId::ROOT,
                visible: true,
            },
        );
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(app.slideshow_playing);
    }

    #[test]
    fn depth_slideshow_timer_ignores_forwarded_input_and_hidden_render_for_all_targets() {
        for boundary in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let viewer = RootDepthViewer::new(&mut app);
            let ctx = viewer.ctx.clone();
            if boundary {
                app.fullscreen_idx = Some(3);
            }
            install_depth(&mut app);
            settle_failed(&mut app);
            app.fs_transparent_bg_mode = 1; // The documented prefetch/gate exclusion.
            app.settings.slideshow_end_action = crate::settings::SlideshowEndAction::LoopFolder;
            arm_expired(&mut app);
            let before = app.fullscreen_idx;
            timer(&mut app, &ctx, SlideshowTimerPass::InputOnly);
            assert_eq!(app.fullscreen_idx, before);
            timer(
                &mut app,
                &ctx,
                SlideshowTimerPass::ViewerRender {
                    viewport: egui::ViewportId::ROOT,
                    visible: false,
                },
            );
            assert_eq!(app.fullscreen_idx, before);
            timer(
                &mut app,
                &ctx,
                SlideshowTimerPass::ViewerRender {
                    viewport: egui::ViewportId::ROOT,
                    visible: true,
                },
            );
            assert_eq!(app.fullscreen_idx, Some(if boundary { 0 } else { 2 }));
            assert_eq!(
                app.slideshow_anchor_idx, None,
                "landing waits for the owning Depth draw"
            );
        }
        let ctx = egui::Context::default();
        let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        arm_expired(&mut app);
        timer(&mut app, &ctx, SlideshowTimerPass::InputOnly);
        assert_eq!(
            app.fullscreen_idx,
            Some(1),
            "2D keeps its existing timer behavior"
        );
    }

    #[test]
    fn depth_slideshow_animation_lands_exits_with_one_toast_and_keeps_playing() {
        let _input = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        viewer.frame(&mut app, vec![]);
        let Some(FsCacheEntry::Static { animation, .. }) = app.fs_cache.get_mut(&2) else {
            unreachable!()
        };
        *animation = crate::fs_animation::StaticAnimationState::FirstFrameOnly(
            crate::canonical_image_loader::CanonicalAnimatedFormat::Gif,
        );
        app.feedback_toasts_for_test.clear();
        assert!(app.fs_exclusive_view.is_depth());
        assert_eq!(
            app.depth_slideshow_gate(&viewer.ctx, 2, Instant::now()),
            crate::app::depth_view::DepthSlideshowGate::Go,
        );
        arm_expired(&mut app);
        timer(
            &mut app,
            &viewer.ctx,
            SlideshowTimerPass::ViewerRender {
                viewport: egui::ViewportId::ROOT,
                visible: true,
            },
        );
        assert_eq!(app.fullscreen_idx, Some(2));
        viewer.frame(&mut app, vec![]);
        assert!(!app.fs_exclusive_view.is_depth());
        assert!(app.slideshow_playing);
        assert_eq!(app.slideshow_anchor_idx, Some(2));
        let exit_count = |app: &App| {
            app.feedback_toasts_for_test
                .iter()
                .filter(|(message, _)| message.starts_with("3D 写真を終了しました:"))
                .count()
        };
        assert_eq!(exit_count(&app), 1);
        assert!(
            app.feedback_toasts_for_test
                .iter()
                .any(|(message, _)| message.starts_with("3D 写真を終了しました:")
                    && message.contains("アニメーション"))
        );
        viewer.frame(&mut app, vec![]);
        assert_eq!(
            exit_count(&app),
            1,
            "ordinary 2D continuation must not repeat the exit notice"
        );
        assert_eq!(app.fullscreen_idx, Some(2));
    }

    #[test]
    fn depth_slideshow_real_child_and_detached_render_own_hidden_minimized_and_resume_timer() {
        // Egui's immediate renderer is thread-local. These are generated passes,
        // with the existing real Depth fixture and no native application window.
        std::thread::spawn(|| {
            let _input = crate::key_input::lock_test_input();
            for detached in [false, true] {
                for minimized in [false, true] {
                    let mut app = crate::app::setup_app_for_test();
                    let mut viewer = if detached {
                        RootDepthViewer::linked(&mut app)
                    } else {
                        RootDepthViewer::new(&mut app)
                    };
                    if !detached {
                        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
                        app.native_video_in_window_active = false;
                        app.settings.video_in_window_mode = false;
                    }
                    viewer.ready(&mut app);
                    viewer.ctx.set_embed_viewports(false);
                    let child_minimized = std::rc::Rc::new(std::cell::Cell::new(false));
                    let child_state = child_minimized.clone();
                    let child_id = std::rc::Rc::new(std::cell::Cell::new(egui::ViewportId::ROOT));
                    let observed_id = child_id.clone();
                    let passes = std::rc::Rc::new(std::cell::Cell::new(0_u64));
                    let child_passes = passes.clone();
                    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size);
                    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                        observed_id.set(viewport.ids.this);
                        let pass = child_passes.get() + 1;
                        child_passes.set(pass);
                        let output = ctx.run_ui(
                            egui::RawInput {
                                viewport_id: viewport.ids.this,
                                screen_rect: Some(rect),
                                time: Some(pass as f64 / 60.0),
                                viewports: [
                                    (
                                        egui::ViewportId::ROOT,
                                        egui::ViewportInfo {
                                            minimized: Some(true),
                                            focused: Some(false),
                                            ..Default::default()
                                        },
                                    ),
                                    (
                                        viewport.ids.this,
                                        egui::ViewportInfo {
                                            parent: Some(viewport.ids.parent),
                                            minimized: Some(child_state.get()),
                                            focused: Some(true),
                                            inner_rect: Some(rect),
                                            ..Default::default()
                                        },
                                    ),
                                ]
                                .into_iter()
                                .collect(),
                                ..Default::default()
                            },
                            |ui| (viewport.viewport_ui_cb)(ui),
                        );
                        output.drop_without_applying_deltas();
                    });
                    viewer.frame(&mut app, vec![]);
                    assert_ne!(child_id.get(), egui::ViewportId::ROOT);
                    assert!(passes.get() > 0);
                    assert!(app.fs_exclusive_view.is_depth());
                    assert!(app.depth_current_settled());
                    app.settings.slideshow_interval_secs = 30.0;
                    app.fs_transparent_bg_mode = 1; // The documented prefetch exclusion.
                    arm_expired(&mut app);
                    if minimized {
                        child_minimized.set(true);
                    } else {
                        app.window_visible = false;
                    }
                    viewer.frame_with_before(&mut app, vec![], |app, ctx| {
                        timer(app, ctx, SlideshowTimerPass::InputOnly);
                        assert_eq!(
                            app.fullscreen_idx,
                            Some(0),
                            "ROOT forwarding cannot advance Depth"
                        );
                    });
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(0),
                        "detached={detached}, minimized={minimized}"
                    );
                    assert_eq!(
                        app.slideshow_anchor_idx, None,
                        "a hidden owning pass disarms the interval"
                    );
                    child_minimized.set(false);
                    app.window_visible = true;
                    let resumed = Instant::now();
                    viewer.frame(&mut app, vec![]);
                    assert_eq!(app.fullscreen_idx, Some(0));
                    assert_eq!(app.slideshow_anchor_idx, Some(0));
                    assert!(app.slideshow_next_at >= resumed + Duration::from_secs(30));
                    arm_expired(&mut app);
                    viewer.frame_with_before(&mut app, vec![], |app, ctx| {
                        timer(app, ctx, SlideshowTimerPass::InputOnly);
                        assert_eq!(app.fullscreen_idx, Some(0));
                    });
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(2),
                        "the visible owning child advances despite minimized ROOT"
                    );
                    assert!(app.fs_exclusive_view.is_depth());
                    assert_eq!(app.slideshow_anchor_idx, None);
                }
            }
        })
        .join()
        .unwrap();
    }
}
