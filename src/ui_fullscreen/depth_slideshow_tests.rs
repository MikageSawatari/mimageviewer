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
    app.slideshow_next_at = Instant::now() - Duration::from_secs(60);
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

fn render(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    app.frame_counter += 1;
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            focused: true,
            time: Some(app.frame_counter as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| app.render_fullscreen_viewport(ui),
    ))
    .drop_without_applying_deltas();
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
        app.items[1] = GridItem::Video("c:/page-turn/1.mp4".into());
        app.items_generation += 1;
        app.viewer_navigation_caches.invalidate();
        app.spread_display_units_cache = Default::default();
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
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.next = Some(DepthNextSlot {
        target,
        key: DepthSourceKey {
            texture_id: egui::TextureId::Managed(7782),
        },
        phase: DepthNextPhase::Failed {
            reason: "fixture".into(),
        },
    });
    assert!(app.stop_slideshow_playback());
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        unreachable!()
    };
    assert!(state.next.is_none());
    assert_eq!(app.slideshow_anchor_idx, None);
}

#[cfg(windows)]
mod gpu_tests {
    use super::*;
    use crate::app::depth_main_window_tests::RootDepthViewer;

    #[test]
    fn depth_slideshow_hud_exposes_play_but_keeps_overflow_hidden() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let viewer = RootDepthViewer::new(&mut app);
        let ctx = viewer.ctx.clone();
        viewer.ready(&mut app);
        page_turn::install(&ctx);
        app.settings.fullscreen_top_bar_locked = true;
        app.settings.slideshow_interval_secs = 30.0;
        #[cfg(windows)]
        {
            app.native_video_in_window_active = true;
        }
        for playing in [true, false] {
            render(&mut app, &ctx, vec![]);
            assert!(
                ctx.read_response(egui::Id::unique("fs_overflow_btn"))
                    .is_none()
            );
            let pos = ctx
                .read_response(egui::Id::unique("fs_play_btn"))
                .expect("Depth exposes slideshow playback")
                .rect
                .center();
            render(&mut app, &ctx, vec![egui::Event::PointerMoved(pos)]);
            render(
                &mut app,
                &ctx,
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
        render(&mut app, &ctx, vec![]);
        render(&mut app, &ctx, vec![]);
        assert!(ctx.read_response(egui::Id::unique("fs_play_btn")).is_none());
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
                    app.depth_slideshow_gate(&ctx, 1, deadline + overdue),
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
}
