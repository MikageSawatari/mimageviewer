use super::page_turn_submission_tests::ready_app;
use super::*;

fn make_landscape(app: &mut App, idx: usize) {
    let Some(crate::fs_animation::FsCacheEntry::Static { source_dims, .. }) =
        app.fs_cache.get_mut(&idx)
    else {
        panic!("cached static fixture")
    };
    *source_dims = Some([160, 90]);
    app.record_page_dims_for_spread(idx, (160, 90));
}

fn navigate(app: &mut App, ctx: &egui::Context, delta: i32) -> FsPageNav {
    let current = app.fullscreen_idx.unwrap();
    let nav = app.spread_page_nav(delta);
    app.handle_fs_navigation(ctx, false, false, None, None, None, nav, None, current);
    nav
}

#[test]
fn manual_nav_wrap_spreads_land_on_actual_units_with_cover_and_final_cover() {
    for mode in [
        SpreadMode::Ltr,
        SpreadMode::Rtl,
        SpreadMode::LtrCover,
        SpreadMode::RtlCover,
    ] {
        for final_cover in [false, true] {
            for forward in [false, true] {
                let ctx = egui::Context::default();
                let mut env = ready_app(&ctx, mode);
                let app = &mut *env;
                app.settings.fullscreen_manual_nav_wrap = true;
                app.settings.final_cover_spread_enabled = final_cover;
                let nav = app.get_nav_indices();
                let units = app.build_spread_display_units_for_nav(&nav);
                let first = units.first().unwrap().anchor_idx();
                let last = units.last().unwrap().anchor_idx();
                app.fullscreen_idx = Some(if forward { last } else { first });
                let target = if forward { first } else { last };
                assert_eq!(
                    navigate(app, &ctx, if forward { 1 } else { -1 }),
                    FsPageNav::Wrap(FsWrapTarget {
                        idx: target,
                        slice: None,
                        forward
                    })
                );
                assert_eq!(app.fullscreen_idx, Some(target));
                assert!(app.fs_boundary_hint.is_none());
            }
        }
    }
}

#[test]
fn manual_nav_wrap_split_ltr_rtl_land_on_reading_order_slices() {
    use crate::page_split::PageSlice;
    for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
        for forward in [false, true] {
            let ctx = egui::Context::default();
            let mut env = ready_app(&ctx, mode);
            let app = &mut *env;
            app.settings.fullscreen_manual_nav_wrap = true;
            make_landscape(app, 0);
            make_landscape(app, 3);
            let nav = app.get_nav_indices();
            let steps = app.build_presentation_steps_for_nav(&nav).unwrap();
            let current = if forward {
                *steps.last().unwrap()
            } else {
                steps[0]
            };
            let target = if forward {
                steps[0]
            } else {
                *steps.last().unwrap()
            };
            assert_ne!(target.slice, PageSlice::Full);
            app.fullscreen_idx = Some(current.source_idx);
            app.fullscreen_page_slice = current.slice;
            assert_eq!(
                navigate(app, &ctx, if forward { 1 } else { -1 }),
                FsPageNav::Wrap(FsWrapTarget {
                    idx: target.source_idx,
                    slice: Some(target.slice),
                    forward
                })
            );
            assert_eq!(app.fullscreen_idx, Some(target.source_idx));
            assert_eq!(app.fullscreen_page_slice, target.slice);
            assert!(app.fs_boundary_hint.is_none());
        }
    }
}

#[test]
fn manual_nav_wrap_single_unit_keeps_boundary_even_when_notice_is_hidden() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::SplitRtl] {
        for forward in [false, true] {
            let ctx = egui::Context::default();
            let mut env = ready_app(&ctx, mode);
            let app = &mut *env;
            app.items.truncate(1);
            app.visible_indices = vec![0];
            app.viewer_navigation_caches.invalidate();
            app.settings.fullscreen_manual_nav_wrap = true;
            app.settings.fullscreen_boundary_notice_visible = false;
            navigate(app, &ctx, if forward { 1 } else { -1 });
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(
                matches!(app.fs_boundary_hint, Some(FsBoundaryHint::Edge { at_end, .. }) if at_end == forward)
            );
        }
    }
}

#[test]
fn manual_nav_wrap_single_and_continuous_handler_follow_filtered_sorted_order() {
    for flow in [
        ReadingFlow::Paged,
        ReadingFlow::Vertical,
        ReadingFlow::Horizontal,
    ] {
        for forward in [false, true] {
            let ctx = egui::Context::default();
            let mut env = ready_app(&ctx, SpreadMode::Single);
            let app = &mut *env;
            app.settings.fullscreen_manual_nav_wrap = true;
            app.reading_flow = flow;
            // Filter out page zero, then display a non-index sort order.
            app.visible_indices = vec![2, 3, 1];
            app.viewer_navigation_caches.invalidate();
            app.fullscreen_idx = Some(if forward { 1 } else { 2 });
            navigate(app, &ctx, if forward { 1 } else { -1 });
            assert_eq!(app.fullscreen_idx, Some(if forward { 2 } else { 1 }));
            assert!(app.fs_boundary_hint.is_none());
        }
    }
}

#[test]
fn manual_nav_wrap_handler_excludes_page_turn_with_navigation_wrap_reason() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::SplitLtr] {
        let ctx = egui::Context::default();
        let mut env = ready_app(&ctx, mode);
        let app = &mut *env;
        app.settings.fullscreen_manual_nav_wrap = true;
        app.settings.page_turn_animation_enabled = true;
        let nav = app.get_nav_indices();
        let last = if mode.is_spread() {
            app.build_spread_display_units_for_nav(&nav)
                .last()
                .unwrap()
                .anchor_idx()
        } else {
            3
        };
        app.fullscreen_idx = Some(last);
        page_turn::take_test_events();
        navigate(app, &ctx, 1);
        let events = page_turn::take_test_events();
        assert!(
            events
                .iter()
                .any(|(_, kind, reason)| *kind == "skip" && *reason == "navigation_wrap"),
            "{mode:?}: {events:?}"
        );
        assert!(!events.iter().any(|(_, kind, _)| *kind == "start"));
        assert_eq!(app.fullscreen_idx, Some(0));
    }
}

#[test]
fn manual_nav_wrap_automatic_navigation_and_nudge_keep_nonwrapping_contract() {
    for wrap in [false, true] {
        let ctx = egui::Context::default();
        let mut env = ready_app(&ctx, SpreadMode::Ltr);
        let app = &mut *env;
        app.settings.fullscreen_manual_nav_wrap = wrap;
        app.fullscreen_idx = Some(2);
        assert_eq!(
            app.spread_page_nav_for_indices(&[0, 1, 2, 3], 2, 1),
            FsPageNav::Boundary { at_end: true }
        );
        assert_eq!(app.adjacent_page_turn_direction(2, 0), Some(false));
        // The final page is still the partner of anchor two: nudging to
        // source three is an existing pairing edit, independent of manual wrap.
        assert_eq!(app.compute_spread_offset_nudge(3, 1), Some((3, 1)));
        assert_eq!(
            crate::ui_helpers::fixed_jump_page_idx(&app.items, &[0, 1, 2, 3], 0, 10, false),
            None
        );
    }
}

#[test]
fn manual_nav_wrap_same_source_split_changes_only_slice() {
    for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
        let ctx = egui::Context::default();
        let mut env = ready_app(&ctx, mode);
        let app = &mut *env;
        app.items.truncate(1);
        app.visible_indices = vec![0];
        app.viewer_navigation_caches.invalidate();
        make_landscape(app, 0);
        app.settings.fullscreen_manual_nav_wrap = true;
        let steps = app.build_presentation_steps_for_nav(&[0]).unwrap();
        app.fullscreen_page_slice = steps[1].slice;
        navigate(app, &ctx, 1);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.fullscreen_page_slice, steps[0].slice);
        assert!(app.fs_holdover_tex.is_none());
        assert!(app.fs_boundary_hint.is_none());
        assert!(!app.top_level_grid_view.collection_navigation_pending());
    }
}

fn pointer_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> FsPageNav {
    app.frame_counter += 1;
    let mut nav = FsPageNav::None;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            let state = app.prepare_fullscreen_state(
                ctx,
                app.fullscreen_idx.unwrap(),
                FsPageTurnDecision::normal(),
            );
            nav = app
                .handle_fs_wheel_and_click(ui, ctx, ui.max_rect(), &state, true, 0, &mut None)
                .0;
        },
    )
    .drop_without_applying_deltas();
    nav
}

#[test]
fn manual_nav_wrap_key_wheel_mouse_and_touch_produce_the_same_wrap_target() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for source in ["key", "wheel", "mouse", "touch"] {
        let ctx = egui::Context::default();
        page_turn::install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let mut env = ready_app(&ctx, SpreadMode::Ltr);
        let app = &mut *env;
        app.fullscreen_idx = Some(2);
        app.settings.fullscreen_manual_nav_wrap = true;
        app.settings.touch_still_chrome_learned = true;
        let expected = FsPageNav::Wrap(FsWrapTarget {
            idx: 0,
            slice: None,
            forward: true,
        });
        let result = if source == "key" {
            let mut result = FsPageNav::None;
            ctx.run_pass(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::ArrowRight,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| result = app.handle_fs_key_input(ctx, 2, true).page_nav,
            )
            .drop_without_applying_deltas();
            result
        } else if source == "wheel" {
            pointer_frame(
                app,
                &ctx,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -120.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                }],
            )
        } else {
            let pos = egui::pos2(550.0, 300.0);
            let primary = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let touch = |phase| egui::Event::Touch {
                device_id: egui::TouchDeviceId(1400),
                id: egui::TouchId(1),
                phase,
                pos,
                force: None,
            };
            pointer_frame(app, &ctx, vec![]);
            let mut press = vec![egui::Event::PointerMoved(pos), primary(true)];
            if source == "touch" {
                press.insert(0, touch(egui::TouchPhase::Start));
            }
            assert!(pointer_frame(app, &ctx, press).is_none());
            let mut release = vec![primary(false)];
            if source == "touch" {
                release.insert(0, touch(egui::TouchPhase::End));
            }
            pointer_frame(app, &ctx, release)
        };
        assert_eq!(result, expected, "{source}");
        #[cfg(windows)]
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn manual_nav_wrap_single_and_split_shift_actions_resolve_source_units() {
    for mode in [
        SpreadMode::Single,
        SpreadMode::SplitLtr,
        SpreadMode::SplitRtl,
    ] {
        for dir in [-1, 1] {
            let ctx = egui::Context::default();
            let mut env = ready_app(&ctx, mode);
            let app = &mut *env;
            app.settings.fullscreen_manual_nav_wrap = true;
            make_landscape(app, 0);
            make_landscape(app, 3);
            let current = if dir > 0 { 3 } else { 0 };
            app.fullscreen_idx = Some(current);
            let mut action = FsKeyAction::default();
            app.queue_spread_shift_action(current, dir, &mut action);
            assert_eq!(action.page_nav, FsPageNav::Delta(dir));
            app.handle_fs_navigation(
                &ctx,
                false,
                false,
                None,
                None,
                None,
                action.page_nav,
                None,
                current,
            );
            assert_eq!(app.fullscreen_idx, Some(if dir > 0 { 0 } else { 3 }));
        }
    }
}

#[test]
fn manual_nav_wrap_home_end_and_fixed_jump_keep_existing_boundaries() {
    for wrap in [false, true] {
        for at_end in [false, true] {
            let ctx = egui::Context::default();
            let mut env = ready_app(&ctx, SpreadMode::Single);
            let app = &mut *env;
            let current = if at_end { 3 } else { 0 };
            app.fullscreen_idx = Some(current);
            app.settings.fullscreen_manual_nav_wrap = wrap;
            app.handle_fullscreen_boundary_jump(&ctx, current, at_end, "manual_wrap_test");
            assert_eq!(app.fullscreen_idx, Some(current));
            assert!(
                matches!(app.fs_boundary_hint, Some(FsBoundaryHint::Edge { at_end: actual, .. }) if actual == at_end)
            );
            assert_eq!(
                crate::ui_helpers::fixed_jump_page_idx(
                    &app.items,
                    &[0, 1, 2, 3],
                    current,
                    10,
                    at_end
                ),
                None
            );
        }
    }
}

#[test]
fn manual_nav_wrap_slideshow_stop_and_loop_policy_remain_independent() {
    for wrap in [false, true] {
        for mode in [SpreadMode::Single, SpreadMode::Ltr] {
            for end_action in [
                crate::settings::SlideshowEndAction::Stop,
                crate::settings::SlideshowEndAction::LoopFolder,
            ] {
                let ctx = egui::Context::default();
                let mut env = ready_app(&ctx, mode);
                let app = &mut *env;
                app.settings.fullscreen_manual_nav_wrap = wrap;
                app.settings.slideshow_end_action = end_action;
                let current = if mode.is_spread() { 2 } else { 3 };
                app.fullscreen_idx = Some(current);
                app.slideshow_anchor_idx = Some(current);
                app.slideshow_playing = true;
                app.advance_slideshow(&ctx, current);
                if end_action == crate::settings::SlideshowEndAction::Stop {
                    assert!(!app.slideshow_playing);
                    assert_eq!(app.fullscreen_idx, Some(current));
                } else {
                    assert!(app.slideshow_playing);
                    assert_eq!(app.fullscreen_idx, Some(0));
                }
            }
        }
    }
}
