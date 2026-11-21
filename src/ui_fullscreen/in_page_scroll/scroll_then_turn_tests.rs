//! Scroll-then-turn uses the same accepted geometry for prediction and input.
use super::handler_tests::{app, frame, paint, seed};
use super::*;
use crate::keymap::PageTurnPressKind::{AutoRepeat, InitialPress};

#[test]
fn press_moves_to_edge_then_turns_but_repeat_stays_on_the_page() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_n_scroll_step_percent = 100;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        let before = app.fs_pan;
        let first = app.fs_page_scroll_or_turn_key(ctx, true, InitialPress);
        assert_eq!(first, FsPageNavRequest::default());
        assert_ne!(app.fs_pan, before);
        let edge = app.fs_pan;
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, AutoRepeat),
            FsPageNavRequest::default()
        );
        assert_eq!(app.fs_pan, edge);
        let turn = app.fs_page_scroll_or_turn_key(ctx, true, InitialPress);
        assert_eq!(turn.nav, app.spread_page_nav(1));
        assert_eq!(turn.landing, Some(FsPageLanding::Start));
        assert_eq!(app.fs_pan, edge);
    });
}

#[test]
fn fitting_page_turns_on_press_and_uses_directional_landing() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.fullscreen_idx = Some(1);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        for next in [false, true] {
            assert_eq!(
                app.fs_page_scroll_or_turn_key(ctx, next, AutoRepeat),
                FsPageNavRequest::default()
            );
            let result = app.fs_page_scroll_or_turn_key(ctx, next, InitialPress);
            assert_eq!(result.nav, app.spread_page_nav(if next { 1 } else { -1 }));
            assert_eq!(
                result.landing,
                Some(if next {
                    FsPageLanding::Start
                } else {
                    FsPageLanding::End
                })
            );
        }
    });
}

#[test]
fn wheel_edge_arrival_blocks_same_frame_turn_and_small_segments_never_turn() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.continuous_reading_wheel_scroll_percent = 100;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        let delta = egui::vec2(0.0, -CONTINUOUS_READING_WHEEL_REFERENCE_DELTA * 4.0);
        let FsPageScrollOutcome::Move(edge) =
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, app.fs_pan, delta, false)
        else {
            panic!("first segment reaches the edge without turning")
        };
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, edge, delta, true),
            FsPageScrollOutcome::Blocked
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, edge, egui::vec2(0.0, -0.5), false),
            FsPageScrollOutcome::Blocked
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, edge, delta, false),
            FsPageScrollOutcome::Turn
        );
        // Applying the candidate through the real writer gives the prediction's
        // exact terminal position; the next frame starts from that same edge.
        app.write_fs_page_scroll_pan_with_animation(ctx, edge, true);
        assert_eq!(app.fs_pan, edge);
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, app.fs_pan, delta, false),
            FsPageScrollOutcome::Turn
        );
    });
}

#[test]
fn terminal_failure_replaces_only_drawn_proof_and_never_loading_or_new_generation() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        let composition = app.fs_scroll_input_composition().unwrap();
        let failed_idx = composition.pages_in_screen_order(app.spread_mode)[1].idx;
        app.fullscreen_page_layout.drawn_unit = None;
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress),
            FsPageNavRequest::default()
        );
        app.final_composite_cache.clear();
        app.fs_cache.insert(failed_idx, FsCacheEntry::Failed);
        assert_eq!(
            app.fs_page_load_state(failed_idx),
            crate::app::FsPageLoadState::LoadFailed
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, AutoRepeat),
            FsPageNavRequest::default()
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress)
                .landing,
            Some(FsPageLanding::Start)
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(
                ctx,
                app.fs_zoom,
                app.fs_pan,
                egui::vec2(0.0, -1.0),
                false
            ),
            FsPageScrollOutcome::Turn
        );
        app.items_generation += 1;
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress),
            FsPageNavRequest::default()
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(
                ctx,
                app.fs_zoom,
                app.fs_pan,
                egui::vec2(0.0, -1.0),
                false
            ),
            FsPageScrollOutcome::Blocked
        );
    });
}

#[test]
fn terminal_failure_does_not_bypass_navigation_sequence_or_surface() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        app.final_composite_cache.clear();
        app.fs_cache.insert(0, FsCacheEntry::Failed);
        app.fullscreen_page_layout.drawn_unit = None;
        app.fs_holdover_tex = Some(FsHoldover::NavigationSequence(FsNavigationSequence {
            previous: None,
            chrome: FsNavigationChromeContinuation::None,
            purpose: FsNavigationPurpose::Ordinary,
            opened_at: std::time::Instant::now(),
            target: FsNavigationSequenceTarget::FolderItems {
                accepted_generation: app.items_generation,
            },
        }));
        assert!(app.fs_navigation_sequence_blocks_new_target());
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress),
            FsPageNavRequest::default()
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(
                ctx,
                app.fs_zoom,
                app.fs_pan,
                egui::vec2(0.0, -1.0),
                false
            ),
            FsPageScrollOutcome::Blocked
        );
        app.fs_holdover_tex = None;
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress)
                .landing,
            Some(FsPageLanding::Start)
        );
        app.reading_flow = ReadingFlow::Vertical;
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress),
            FsPageNavRequest::default()
        );
    });
}

#[test]
fn terminal_failure_during_resident_thumbnail_turn_is_consumed_without_continuation() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    use crate::ui_fullscreen::page_turn_submission_tests::{
        draw_current, frame as turn_frame, install_presentation, offer_texture_for_test, ready_app,
    };
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    turn_frame(&ctx, |ui, ctx| {
        draw_current(&mut app, ui, ctx);
    });
    app.fs_cache.remove(&1);
    let texture = ctx.load_texture(
        "failed-turn-thumb",
        egui::ColorImage::filled([8, 12], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails[1] = ThumbnailState::loaded_raster(
        texture,
        crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
        false,
        12,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((80, 120)),
        None,
    );
    turn_frame(&ctx, |ui, ctx| {
        offer_texture_for_test(&mut app, ctx, 1);
        app.fullscreen_idx = Some(1);
        draw_current(&mut app, ui, ctx);
    });
    assert!(app.page_turn_barrier_active());
    app.fs_cache.insert(1, FsCacheEntry::Failed);
    turn_frame(&ctx, |_, ctx| {
        assert_eq!(
            app.fs_page_load_state(1),
            crate::app::FsPageLoadState::LoadFailed
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress),
            FsPageNavRequest::default()
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(
                ctx,
                app.fs_zoom,
                app.fs_pan,
                egui::vec2(0.0, -1.0),
                false
            ),
            FsPageScrollOutcome::Blocked
        );
        assert_eq!(app.fullscreen_idx, Some(1));
    });
    app.page_turn_presentation.cancel("failed-scroll-test");
    turn_frame(&ctx, |_, ctx| {
        assert!(app.page_turn_completion_active());
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress),
            FsPageNavRequest::default()
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(
                ctx,
                app.fs_zoom,
                app.fs_pan,
                egui::vec2(0.0, -1.0),
                false
            ),
            FsPageScrollOutcome::Blocked
        );
    });
    app.page_turn_presentation
        .deactivate("failed-scroll-test-ended");
    turn_frame(&ctx, |_, ctx| {
        assert!(!app.page_turn_barrier_active());
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress)
                .landing,
            Some(FsPageLanding::Start)
        );
    });
}

#[test]
fn fractional_wheel_advances_primary_but_cannot_wrap_to_another_column() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.fs_zoom = 2.0;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |ui, ctx| {
        let forward = egui::vec2(0.0, -0.25);
        let FsPageScrollOutcome::Move(first) =
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, app.fs_pan, forward, false)
        else {
            panic!("fractional input advances")
        };
        assert!(first.y < app.fs_pan.y);
        let geometry = scroll_geometry(&app.fullscreen_page_layout).unwrap();
        let edge = Vec2::new(app.fs_pan.x, geometry.axes[1].unwrap().min);
        let zoom = app.fs_zoom;
        app.set_fs_pan_from_input(zoom, edge);
        paint(&mut app, ui, ctx);
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, edge, forward, false),
            FsPageScrollOutcome::Blocked
        );
        assert!(matches!(
            app.fs_page_scroll_or_turn_wheel(ctx, app.fs_zoom, edge, egui::vec2(0.0, -1.0), false),
            FsPageScrollOutcome::Move(_)
        ));
    });
}

#[test]
fn all_scroll_amounts_finish_n_and_z_in_both_directions_without_repeating_wraps() {
    use crate::settings::FullscreenNScrollOrder::{HorizontalFirst, VerticalFirst};
    for fraction in [0.10, 0.50, 0.85, 1.0] {
        for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            for next in [false, true] {
                for transpose in [false, true] {
                    for trimmed in [false, true] {
                        let layout = super::tests::asymmetric_spread(trimmed, transpose);
                        let order = if transpose {
                            HorizontalFirst
                        } else {
                            VerticalFirst
                        };
                        let primary = if transpose { 0 } else { 1 };
                        let secondary = 1 - primary;
                        let geometry = scroll_geometry(&layout).unwrap();
                        let horizontal = if direction == ReadingDirection::Ltr {
                            -1.0
                        } else {
                            1.0
                        };
                        let signs = egui::vec2(horizontal, -1.0) * if next { 1.0 } else { -1.0 };
                        let mut pan = Vec2::new(
                            geometry.axes[0].unwrap().start(signs.x),
                            geometry.axes[1].unwrap().start(signs.y),
                        );
                        let mut stopped = false;
                        for _ in 0..1500 {
                            let outcome = ordered_scroll_outcome(
                                &layout, 1.0, pan, direction, order, next, fraction, None, true,
                                true,
                            );
                            let FsPageScrollOutcome::Move(candidate) = outcome else {
                                assert_eq!(outcome, FsPageScrollOutcome::Turn);
                                stopped = true;
                                break;
                            };
                            assert!(
                                (candidate[secondary] - pan[secondary]) * signs[secondary] >= 0.0
                            );
                            if (candidate[primary] - pan[primary]) * signs[primary] < 0.0 {
                                assert!(
                                    (candidate[secondary] - pan[secondary]) * signs[secondary]
                                        > 0.0,
                                    "wrap must advance to a new row/column"
                                );
                            }
                            pan = candidate;
                        }
                        assert!(
                            stopped,
                            "fraction={fraction} direction={direction:?} next={next} transpose={transpose} trim={trimmed}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn terminal_raw_failure_without_failed_fs_cache_is_still_navigable() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.items[0] = GridItem::Image(std::path::PathBuf::from("c:/page-turn/failed.cr3"));
    app.fs_cache.remove(&0);
    let generation = app.items_generation;
    app.raw_pages.set_items_generation(generation);
    let request = app
        .raw_pages
        .begin_preview(0, "failed-raw-scroll".to_owned());
    app.raw_pages
        .source_failed(0, request, crate::raw::RawError::OutOfMemory);
    assert!(!matches!(app.fs_cache.get(&0), Some(FsCacheEntry::Failed)));
    frame(&ctx, vec![], |_, ctx| {
        assert_eq!(
            app.fs_page_load_state(0),
            crate::app::FsPageLoadState::LoadFailed
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, AutoRepeat),
            FsPageNavRequest::default()
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_key(ctx, true, InitialPress)
                .landing,
            Some(FsPageLanding::Start)
        );
        assert_eq!(
            app.fs_page_scroll_or_turn_wheel(
                ctx,
                app.fs_zoom,
                app.fs_pan,
                egui::vec2(0.0, -1.0),
                false
            ),
            FsPageScrollOutcome::Turn
        );
    });
}
