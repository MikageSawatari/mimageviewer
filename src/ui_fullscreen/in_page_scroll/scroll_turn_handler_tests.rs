//! Actual key and wheel routing, with frame prediction and deferred application.
use super::handler_tests::{app as scroll_app, frame, full_rect, seed, tap, wheel_event};
use super::*;
use crate::settings::FullscreenWheelAction;

#[cfg(windows)]
struct ClearKeyboard;
#[cfg(windows)]
impl Drop for ClearKeyboard {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

pub(super) fn wheel_frame(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    merge: bool,
) -> FsWheelFrameResult {
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
    let geometry = app.still_seek_geometry_for_idx(full_rect(), idx, false);
    let (front, touch_excluded) = app.capture_fs_front_pointer_input(ctx, full_rect());
    let primary = app.advance_viewer_primary_pointer_frame(ctx, front.primary_owned);
    app.handle_fs_wheel_and_click_frame(
        ui,
        ctx,
        full_rect(),
        &state,
        geometry,
        false,
        0,
        &mut None,
        None,
        primary,
        false,
        &mut None,
        false,
        merge,
        front,
        &touch_excluded,
    )
}

fn actions(app: &App, ctx: &egui::Context) -> Vec<FsWheelAction> {
    let options = ctx.options(|o| o.input_options);
    ctx.input(|i| crate::wheel_input::remaining_wheel_segments(i, options))
        .into_iter()
        .map(|segment| FsWheelAction {
            input: segment.input,
            segment,
            operation: None,
            class: app.fs_wheel_segment_in_page_scroll(segment.input, false),
            scroll_origin: app.fullscreen_page_layout.drawn_unit.clone(),
            page_owner: (
                app.items_generation,
                app.fullscreen_idx,
                app.fullscreen_page_slice,
            ),
            seek_hover: false,
        })
        .collect()
}

fn events(delta: f32) -> Vec<egui::Event> {
    let first = wheel_event(egui::vec2(0.0, delta), egui::Modifiers::NONE);
    let mut second = first.clone();
    if let egui::Event::MouseWheel { source, .. } = &mut second {
        *source = egui::MouseWheelSource::Momentum;
    }
    vec![
        egui::Event::PointerMoved(full_rect().center()),
        first,
        second,
    ]
}

fn no_wheel(ctx: &egui::Context) -> bool {
    let options = ctx.options(|o| o.input_options);
    ctx.input(|i| crate::wheel_input::remaining_wheel_segments(i, options).is_empty())
}

#[test]
fn real_wheel_prediction_arrives_at_edge_and_next_frame_turns_with_landing() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::ScrollThenPageTurn;
    app.settings.continuous_reading_wheel_scroll_percent = 100;
    seed(&ctx, &mut app);
    let mut edge = Vec2::ZERO;
    frame(&ctx, events(-120.0), |ui, ctx| {
        let accepted = actions(&app, ctx);
        assert_eq!(
            accepted.len(),
            2,
            "source boundary creates two real segments"
        );
        let predicted = app.predict_fs_wheel_frame(ctx, full_rect(), &accepted);
        assert!(
            !predicted.2,
            "edge arrival and a second same-frame notch cannot turn"
        );
        let result = wheel_frame(&mut app, ui, ctx, true);
        assert!(result.deferred.is_none());
        assert!(result.page_nav.is_none());
        edge = app.fs_pan;
        assert_eq!((app.fs_zoom, edge), (predicted.0, predicted.1));
        assert!(edge.y < 0.0);
        assert!(no_wheel(ctx));
    });
    frame(
        &ctx,
        vec![
            egui::Event::PointerMoved(full_rect().center()),
            wheel_event(egui::vec2(0.0, -120.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let accepted = actions(&app, ctx);
            let prediction = app.predict_fs_wheel_frame(ctx, full_rect(), &accepted);
            assert!(prediction.2);
            assert_eq!(prediction.1, edge);
            let result = wheel_frame(&mut app, ui, ctx, true);
            assert_eq!(
                result.page_nav,
                FsPageNavRequest {
                    nav: FsPageNav::Delta(1),
                    landing: Some(FsPageLanding::Start)
                }
            );
            assert_eq!(app.fs_pan, edge);
            assert!(no_wheel(ctx));
        },
    );
}

#[test]
fn real_deferred_wheel_turn_consumes_later_segments_without_moving_target_page() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::ScrollThenPageTurn;
    seed(&ctx, &mut app);
    frame(&ctx, events(-120.0), |ui, ctx| {
        let result = wheel_frame(&mut app, ui, ctx, true);
        assert!(result.wheel_navigation_produced);
        let mut proposal = result
            .deferred
            .expect("navigation mixed with another segment defers");
        assert_eq!(app.fullscreen_idx, Some(0));
        app.claim_fs_wheel_frame_after_ui(&mut proposal);
        let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut None);
        assert_eq!(
            app.fullscreen_idx,
            Some(1),
            "only the first accepted segment turns"
        );
        assert!(result.page_nav.is_none());
        assert_eq!(
            app.fs_pan,
            Vec2::ZERO,
            "later segment cannot move the new page"
        );
        assert_eq!(
            app.fs_initial_placement_request.unwrap().kind,
            FsPageLanding::Start.initial_placement_kind()
        );
        assert!(no_wheel(ctx));
    });
}

#[test]
fn deferred_scroll_turn_drops_old_page_and_generation_and_respects_front_consumption() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for invalidation in 0..3 {
        let ctx = egui::Context::default();
        let mut app = scroll_app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::ScrollThenPageTurn;
        seed(&ctx, &mut app);
        frame(&ctx, events(-120.0), |ui, ctx| {
            let result = wheel_frame(&mut app, ui, ctx, true);
            let proposal = result.deferred.expect("deferred navigation");
            match invalidation {
                0 => app.fullscreen_idx = Some(1),
                1 => app.items_generation += 1,
                _ => crate::wheel_input::consume_remaining_wheel_inputs(ctx, |_| true),
            }
            let idx = app.fullscreen_idx;
            let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut None);
            assert!(result.page_nav.is_none());
            assert_eq!(app.fullscreen_idx, idx);
            assert_eq!(app.fs_pan, Vec2::ZERO);
            assert!(app.fs_initial_placement_request.is_none());
            assert!(no_wheel(ctx));
        });
    }
}

#[test]
fn real_scroll_turn_key_consumes_repeat_but_never_turns_until_a_fresh_press() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_n_scroll_step_percent = 100;
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageScrollOrTurnNext = F24\n");
    seed(&ctx, &mut app);
    frame(&ctx, tap(egui::Key::F24), |_, ctx| {
        let action = app.handle_fs_key_input(ctx, 0, false);
        assert!(action.page_nav.is_none());
        assert!(app.fs_pan.y < 0.0);
        assert!(!ctx.input(|i| i.events.iter().any(|e| matches!(
            e,
            egui::Event::Key {
                key: egui::Key::F24,
                pressed: true,
                ..
            }
        ))));
    });
    let key = |repeat| egui::Event::Key {
        key: egui::Key::F24,
        physical_key: Some(egui::Key::F24),
        pressed: true,
        repeat,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&ctx, vec![key(true)], |_, ctx| {
        assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
    });
    let mut release = key(false);
    if let egui::Event::Key { pressed, .. } = &mut release {
        *pressed = false;
    }
    frame(&ctx, vec![release], |_, ctx| {
        assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
    });
    frame(&ctx, tap(egui::Key::F24), |_, ctx| {
        assert_eq!(
            app.handle_fs_key_input(ctx, 0, false).page_nav,
            FsPageNavRequest {
                nav: FsPageNav::Delta(1),
                landing: Some(FsPageLanding::Start)
            }
        );
    });
}

#[test]
fn scroll_turn_keys_are_left_unconsumed_outside_surface_and_front_ui() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    for excluded in 0..5 {
        let ctx = egui::Context::default();
        let mut app = scroll_app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
        app.keymap =
            crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageScrollOrTurnNext = F24\n");
        seed(&ctx, &mut app);
        match excluded {
            0 => app.reading_flow = ReadingFlow::Vertical,
            1 => app.analysis_mode = true,
            2 => app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5),
            3 => app.fs_context_menu_idx = Some(0),
            _ => {}
        }
        frame(&ctx, tap(egui::Key::F24), |_, ctx| {
            if excluded == 4 {
                egui::Popup::open_id(ctx, egui::Id::unique("scroll-turn-front-popup"));
            }
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            assert_eq!(app.fs_pan, Vec2::ZERO);
            assert!(
                ctx.input(|i| i.events.iter().any(|e| matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::F24,
                        pressed: true,
                        ..
                    }
                ))),
                "excluded={excluded}"
            );
        });
    }
}

#[test]
fn fixed_arrow_overwrites_the_whole_scroll_turn_request_in_the_same_key_pass() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.fullscreen_idx = Some(1);
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageScrollOrTurnPrev = F24\n");
    seed(&ctx, &mut app);
    let mut keys = tap(egui::Key::F24);
    keys.extend(tap(egui::Key::ArrowRight));
    frame(&ctx, keys, |_, ctx| {
        let action = app.handle_fs_key_input(ctx, 1, false);
        assert_eq!(
            action.page_nav,
            FsPageNavRequest {
                nav: FsPageNav::Delta(1),
                landing: None
            }
        );
    });
}

#[test]
fn click_overwrites_the_whole_scroll_turn_wheel_request_without_landing() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::ScrollThenPageTurn;
    seed(&ctx, &mut app);
    let pos = egui::pos2(450.0, 200.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&ctx, vec![egui::Event::PointerMoved(pos)], |ui, ctx| {
        assert!(wheel_frame(&mut app, ui, ctx, false).page_nav.is_none());
    });
    frame(&ctx, vec![button(true)], |ui, ctx| {
        assert!(wheel_frame(&mut app, ui, ctx, false).page_nav.is_none());
    });
    frame(
        &ctx,
        vec![
            button(false),
            wheel_event(egui::vec2(0.0, -120.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let result = wheel_frame(&mut app, ui, ctx, false);
            assert_eq!(
                result.page_nav,
                FsPageNavRequest {
                    nav: FsPageNav::Delta(1),
                    landing: None
                }
            );
            assert!(no_wheel(ctx));
        },
    );
}

#[test]
fn ordered_step_setting_does_not_change_continuous_page_up_or_page_down() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
        for key in [egui::Key::PageUp, egui::Key::PageDown] {
            let mut control = None;
            for amount in [10, 50, 85, 100] {
                let ctx = egui::Context::default();
                let mut app = scroll_app(&ctx, SpreadMode::Single);
                app.reading_flow = flow;
                app.settings.fullscreen_n_scroll_step_percent = amount;
                app.fs_vertical_scroll = 500.0;
                frame(&ctx, tap(key), |_, ctx| {
                    assert!(app.continuous_reading_active_for_idx(0));
                    assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
                });
                let value = app.fs_vertical_scroll;
                assert_ne!(value, 500.0, "continuous input moves in the existing owner");
                if let Some(expected) = control {
                    assert_eq!(value, expected);
                } else {
                    control = Some(value);
                }
                assert!(app.fs_page_scroll_animation.is_none());
            }
        }
    }
}

#[test]
fn failed_page_deferred_turn_is_not_reinterpreted_after_a_thumbnail_receives_a_stamp() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::ScrollThenPageTurn;
    seed(&ctx, &mut app);
    let texture = ctx.load_texture(
        "failed-scroll-resident",
        egui::ColorImage::filled([80, 120], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.thumbnails[0] = ThumbnailState::loaded_raster(
        texture,
        crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
        false,
        120,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((80, 120)),
        None,
    );
    app.final_composite_cache.clear();
    app.edit_result_cache.clear();
    app.fs_cache.insert(0, FsCacheEntry::Failed);
    app.fullscreen_page_layout.drawn_unit = None;
    frame(&ctx, events(-120.0), |ui, ctx| {
        assert_eq!(
            app.fs_page_load_state(0),
            crate::app::FsPageLoadState::LoadFailed
        );
        let result = wheel_frame(&mut app, ui, ctx, true);
        assert!(result.wheel_navigation_produced);
        let proposal = result
            .deferred
            .expect("two failed-page segments defer predicted navigation");
        let FsWheelFrameInput::Unconsumed(eligible) = &proposal.wheel_input else {
            panic!("eligible input");
        };
        assert_eq!(eligible.len(), 2);
        assert!(eligible.iter().all(|action| action.scroll_origin.is_none()));
        super::handler_tests::paint(&mut app, ui, ctx);
        assert!(
            app.fullscreen_page_layout.drawn_unit.is_some(),
            "ordinary resident thumbnail publishes its unit"
        );
        let pan = app.fs_pan;
        let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut None);
        assert!(result.page_nav.is_none());
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(
            app.fs_pan, pan,
            "old Turn cannot become a Move through the newly painted stamp"
        );
        assert!(app.fs_initial_placement_request.is_none());
        assert!(no_wheel(ctx));
    });
    frame(
        &ctx,
        vec![
            egui::Event::PointerMoved(full_rect().center()),
            wheel_event(egui::vec2(0.0, -1.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let before = app.fs_pan;
            let accepted = actions(&app, ctx);
            let predicted = app.predict_fs_wheel_frame(ctx, full_rect(), &accepted);
            assert!(!predicted.2);
            assert!(
                predicted.1.y < before.y,
                "new input uses the current thumbnail geometry"
            );
            let result = wheel_frame(&mut app, ui, ctx, false);
            assert!(result.page_nav.is_none());
            assert_eq!(app.fs_pan, predicted.1);
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(no_wheel(ctx));
        },
    );
}
