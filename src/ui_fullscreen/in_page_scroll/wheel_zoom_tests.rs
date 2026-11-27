//! Setting zoom goes through the real wheel handler and ordinary page painter.
//! No renderer, PDF worker, media decoder, or desktop window is started here.
use super::handler_tests::{
    app, deferred_wheel_proposal, frame, full_rect, paint, seed, wheel_event,
};
use super::*;
use crate::settings::{FullscreenWheelAction, FullscreenWheelZoomStep};

fn section1856_zoom(wheel_y: f32, step: FullscreenWheelZoomStep) -> f32 {
    let mut zoom = 1.0;
    App::apply_wheel_zoom(
        &mut zoom,
        &mut Vec2::ZERO,
        wheel_y,
        step,
        None,
        egui::Pos2::ZERO,
    );
    zoom
}

#[test]
fn section1856_standard_matches_legacy_factor_bit_for_bit() {
    for delta in [
        -120.0, -40.0, -30.0, -20.0, -0.25, 0.0, 0.25, 20.0, 30.0, 40.0, 120.0,
    ] {
        let legacy = 1.1_f32.powf(delta / WHEEL_SENSITIVITY);
        for step in [
            FullscreenWheelZoomStep::Standard,
            FullscreenWheelZoomStep::Unknown,
        ] {
            assert_eq!(
                section1856_zoom(delta, step).to_bits(),
                legacy.to_bits(),
                "{step:?}, {delta}"
            );
        }
    }
}

#[test]
fn section1856_percent_steps_match_one_native_notch() {
    // Lock the premise shared with native wheel projection, without changing egui's input options.
    assert_eq!(
        egui::InputOptions::default().line_scroll_speed,
        WHEEL_ZOOM_POINTS_PER_NOTCH
    );
    for (step, factor) in [
        (FullscreenWheelZoomStep::Percent5, 1.05_f32),
        (FullscreenWheelZoomStep::Percent10, 1.10),
        (FullscreenWheelZoomStep::Percent20, 1.20),
        (FullscreenWheelZoomStep::Percent30, 1.30),
        (FullscreenWheelZoomStep::Percent50, 1.50),
        (FullscreenWheelZoomStep::Percent100, 2.0),
    ] {
        assert_eq!(
            section1856_zoom(WHEEL_ZOOM_POINTS_PER_NOTCH, step),
            factor,
            "{step:?}"
        );
        let out = section1856_zoom(-WHEEL_ZOOM_POINTS_PER_NOTCH, step);
        assert!((out - factor.recip()).abs() < 1e-6, "{step:?}");
    }
}

#[test]
fn section1856_zoom_is_symmetric_fractional_and_unquantized() {
    for &step in FullscreenWheelZoomStep::all() {
        let notch = section1856_zoom(WHEEL_ZOOM_POINTS_PER_NOTCH, step);
        let half = section1856_zoom(WHEEL_ZOOM_POINTS_PER_NOTCH / 2.0, step);
        assert!((half - notch.sqrt()).abs() < 1e-6, "{step:?}");
        assert!(
            section1856_zoom(0.25, step) > 1.0,
            "{step:?}: tiny input must survive"
        );
        for delta in [0.25, 20.0, 40.0, 120.0] {
            let up = section1856_zoom(delta, step);
            let down = section1856_zoom(-delta, step);
            assert!((up * down - 1.0).abs() < 1e-6, "{step:?}, {delta}");
            let mut zoom = up;
            App::apply_wheel_zoom(
                &mut zoom,
                &mut Vec2::ZERO,
                -delta,
                step,
                None,
                egui::Pos2::ZERO,
            );
            assert!((zoom - 1.0).abs() < 1e-6, "{step:?}, {delta}");
        }
        let combined = section1856_zoom(10.0 + 20.0 + 50.0, step);
        let mut separate = 1.0;
        for delta in [10.0, 20.0, 50.0] {
            App::apply_wheel_zoom(
                &mut separate,
                &mut Vec2::ZERO,
                delta,
                step,
                None,
                egui::Pos2::ZERO,
            );
        }
        assert!((combined - separate).abs() < 2e-6, "{step:?}");
    }
}

#[test]
fn section1856_zoom_clamps_and_preserves_cursor_pivot() {
    let center = egui::pos2(300.0, 200.0);
    let mouse = egui::pos2(120.0, 90.0);
    let original_pan = Vec2::new(30.0, -20.0);
    for &step in FullscreenWheelZoomStep::all() {
        for (old_zoom, delta, clamped) in [
            (1.0, WHEEL_ZOOM_POINTS_PER_NOTCH, None),
            (1.0, -WHEEL_ZOOM_POINTS_PER_NOTCH, None),
            (49.9, 120.0, Some(ZOOM_MAX)),
            (0.101, -120.0, Some(ZOOM_MIN)),
        ] {
            let mut zoom = old_zoom;
            let mut pan = original_pan;
            let image_point = (mouse - center - pan) / old_zoom;
            assert!(App::apply_wheel_zoom(
                &mut zoom,
                &mut pan,
                delta,
                step,
                Some(mouse),
                center
            ));
            if let Some(limit) = clamped {
                assert_eq!(zoom, limit, "{step:?}");
            }
            assert!(
                ((mouse - center - pan) / zoom - image_point).length() < 0.001,
                "{step:?}: pivot moved"
            );
        }
        for (limit, delta) in [(ZOOM_MAX, 120.0), (ZOOM_MIN, -120.0)] {
            let mut zoom = limit;
            let mut pan = original_pan;
            assert!(!App::apply_wheel_zoom(
                &mut zoom,
                &mut pan,
                delta,
                step,
                Some(mouse),
                center
            ));
            assert_eq!(zoom, limit);
            assert_eq!(pan, original_pan);
        }
        let mut zoom = 1.0;
        let mut pan = original_pan;
        assert!(App::apply_wheel_zoom(
            &mut zoom, &mut pan, 40.0, step, None, center
        ));
        assert_eq!(pan, original_pan);
    }
}

fn pointer_handler(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> FsPageNav {
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
    let (navigation, close) =
        app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
    assert!(!close);
    navigation
}

fn wheel_remaining(ctx: &egui::Context) -> bool {
    let options = ctx.options(|options| options.input_options);
    ctx.input(|input| !crate::wheel_input::remaining_wheel_segments(input, options).is_empty())
}

fn action(app: &App, input: crate::wheel_input::WheelInput) -> FsWheelAction {
    FsWheelAction {
        input,
        segment: crate::wheel_input::WheelSegment {
            vertical_before_axis_mapping: 0.,
            input,
            first_raw_ordinal: Some(1),
            last_raw_ordinal: Some(1),
        },
        operation: None,
        class: app.fs_wheel_segment_in_page_scroll(input, false),
        scroll_origin: app.fullscreen_page_layout.drawn_unit.clone(),
        page_owner: (
            app.items_generation,
            app.fullscreen_idx,
            app.fullscreen_page_slice,
        ),
        seek_hover: false,
    }
}

#[test]
fn section1856_selected_step_reaches_analysis_and_edit_wheel_routes() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for surface in ["analysis", "erase", "conceal"] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
        app.settings.fullscreen_wheel_zoom_step = FullscreenWheelZoomStep::Percent50;
        seed(&ctx, &mut app);
        match surface {
            "analysis" => app.analysis_mode = true,
            "erase" => app.erase_mode = true,
            "conceal" => app.conceal_mode = true,
            _ => unreachable!("surface={surface}"),
        }
        assert_eq!(
            app.is_overlay_edit_mode_active(),
            surface != "analysis",
            "surface={surface}: wheel route setup"
        );
        app.analysis_zoom = 1.0;
        app.fs_zoom = 1.0;
        for (delta, expected) in [(40.0, 1.5), (-40.0, 1.0)] {
            frame(
                &ctx,
                vec![
                    egui::Event::PointerMoved(full_rect().center()),
                    wheel_event(Vec2::new(0.0, delta), egui::Modifiers::NONE),
                ],
                |ui, ctx| {
                    let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                    let (navigation, close) = app.handle_fs_wheel_and_click(
                        ui,
                        ctx,
                        full_rect(),
                        &state,
                        false,
                        0,
                        &mut None,
                    );
                    assert!(
                        navigation.is_none(),
                        "surface={surface}, delta={delta}: navigation={navigation:?}"
                    );
                    assert!(!close, "surface={surface}, delta={delta}: viewer closed");
                    let zoom = if surface == "analysis" {
                        app.analysis_zoom
                    } else {
                        app.fs_zoom
                    };
                    assert!(
                        (zoom - expected).abs() < 1e-6,
                        "surface={surface}, delta={delta}: {zoom}"
                    );
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(0),
                        "surface={surface}, delta={delta}"
                    );
                    if surface == "analysis" {
                        assert_eq!(app.fs_zoom, 1.0, "surface={surface}, delta={delta}");
                    } else {
                        assert_eq!(app.analysis_zoom, 1.0, "surface={surface}, delta={delta}");
                    }
                },
            );
        }
    }
}

#[test]
fn section1856_fractional_same_frame_wheel_and_prediction_use_selected_step() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for (setting, modifiers) in [
            (FullscreenWheelAction::Zoom, egui::Modifiers::NONE),
            (FullscreenWheelAction::PageTurn, egui::Modifiers::CTRL),
        ] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, mode);
            app.settings.fullscreen_wheel_action = setting;
            app.settings.fullscreen_wheel_zoom_step = FullscreenWheelZoomStep::Percent50;
            app.fs_zoom = 1.0;
            app.fs_pan = Vec2::ZERO;
            seed(&ctx, &mut app);
            let pointer = egui::pos2(450.0, 200.0);
            frame(
                &ctx,
                vec![
                    egui::Event::PointerMoved(pointer),
                    wheel_event(Vec2::new(0.0, 10.0), modifiers),
                    wheel_event(Vec2::new(0.0, 10.0), modifiers),
                ],
                |ui, ctx| {
                    let options = ctx.options(|options| options.input_options);
                    let projected = ctx
                        .input(|input| crate::wheel_input::remaining_wheel_inputs(input, options));
                    assert_eq!(projected.len(), 1, "same-frame events must accumulate");
                    assert_eq!(projected[0].delta.y, 20.0);
                    let accepted = action(&app, projected[0]);
                    let viewport = app.fullscreen_media_rect(full_rect(), 0, false);
                    let predicted =
                        app.propose_fs_wheel_click_transform(ctx, viewport, &[accepted]);
                    assert!((predicted.0 - 1.5_f32.sqrt()).abs() < 1e-6);
                    assert_eq!((app.fs_zoom, app.fs_pan), (1.0, Vec2::ZERO));
                    assert!(pointer_handler(&mut app, ui, ctx).is_none());
                    assert_eq!((app.fs_zoom, app.fs_pan), predicted);
                },
            );
        }
    }
}

#[test]
fn setting_zoom_matches_ctrl_cursor_pivot_clamp_and_actual_painted_transform() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for (zoom, pan, delta, pointer) in [
        (1.0, Vec2::ZERO, 120.0, egui::pos2(120.0, 100.0)),
        (
            3.0,
            Vec2::new(200.0, -100.0),
            -120.0,
            egui::pos2(490.0, 300.0),
        ),
        (49.0, Vec2::ZERO, 120.0, egui::pos2(300.0, 200.0)),
        // An outward drag may leave only the required strip visible. A large
        // zoom-out must reuse the regular input clamp rather than keep it lost.
        (
            3.0,
            Vec2::new(950.0, 1300.0),
            -240.0,
            egui::pos2(300.0, 200.0),
        ),
    ] {
        let mut ctrl_result = None;
        for (setting, modifiers) in [
            (FullscreenWheelAction::PageTurn, egui::Modifiers::CTRL),
            (FullscreenWheelAction::Zoom, egui::Modifiers::NONE),
        ] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_wheel_action = setting;
            app.fs_zoom = zoom;
            app.fs_pan = pan;
            seed(&ctx, &mut app);
            let before = (app.fs_zoom, app.fs_pan);
            frame(
                &ctx,
                vec![
                    egui::Event::PointerMoved(pointer),
                    wheel_event(Vec2::new(0.0, delta), modifiers),
                ],
                |ui, ctx| {
                    let viewport = app.fullscreen_media_rect(full_rect(), 0, false);
                    let (mut expected_zoom, mut expected_pan) = before;
                    App::apply_wheel_zoom(
                        &mut expected_zoom,
                        &mut expected_pan,
                        delta,
                        crate::settings::FullscreenWheelZoomStep::Standard,
                        Some(pointer),
                        viewport.center(),
                    );
                    expected_pan = clamp_fullscreen_pan_from_layout(
                        &app.fullscreen_page_layout,
                        expected_zoom,
                        expected_pan,
                    );
                    assert!(pointer_handler(&mut app, ui, ctx).is_none());
                    assert_eq!((app.fs_zoom, app.fs_pan), (expected_zoom, expected_pan));
                    assert_ne!(app.fs_zoom, before.0);
                    assert!(!wheel_remaining(ctx), "the zoom owner consumes its segment");
                },
            );
            let result = (app.fs_zoom, app.fs_pan);
            if setting == FullscreenWheelAction::Zoom {
                assert_eq!(Some(result), ctrl_result);
            } else {
                ctrl_result = Some(result);
            }
            frame(&ctx, vec![], |ui, ctx| {
                paint(&mut app, ui, ctx);
                let page = app.fullscreen_page_layout.displayed_pages().next().unwrap();
                assert_eq!(normal_zoom_pan(page.transform), Some(result));
            });
        }
    }
}

#[test]
fn setting_zoom_pdf_uses_the_same_resolution_demand_as_ctrl_without_a_worker() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let mut ctrl_requests = None;
    for (setting, modifiers) in [
        (FullscreenWheelAction::PageTurn, egui::Modifiers::CTRL),
        (FullscreenWheelAction::Zoom, egui::Modifiers::NONE),
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = setting;
        app.items[0] = GridItem::PdfPage {
            pdf_path: app.tmp.path().join("setting-zoom.pdf"),
            page_num: 0,
            content_type: None,
        };
        let probe = crate::app::PdfRerenderRequestProbe::start();
        // The production viewport establishes ordinary PDF demand before paint.
        // Intercept this initial request as well as the subsequent zoom request.
        app.fs_pdf_display_target = Some(crate::pdf_loader::PdfDisplayTarget::from_logical_size(
            full_rect().width(),
            full_rect().height(),
            1.0,
            app.pdf_display_fit_mode(),
        ));
        app.ensure_pdf_display_resolution(0, None);
        seed(&ctx, &mut app);
        assert!(!probe.take_requests().is_empty());
        app.fs_pending.remove(&0);
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(egui::pos2(180.0, 140.0)),
                wheel_event(Vec2::new(0.0, 120.0), modifiers),
            ],
            |ui, ctx| {
                assert!(pointer_handler(&mut app, ui, ctx).is_none());
                assert!(app.fs_zoom > 1.0);
            },
        );
        let requests = probe.take_requests();
        assert_eq!(requests.len(), 1, "one zoom requests one PDF resolution");
        assert_eq!(requests[0].0, 0);
        assert!(
            requests[0].1 > 120,
            "the tiny cached raster needs a higher resolution"
        );
        if setting == FullscreenWheelAction::Zoom {
            assert_eq!(Some(requests), ctrl_requests);
        } else {
            ctrl_requests = Some(requests);
        }
    }
}

#[test]
fn setting_zoom_real_seek_hover_turns_and_seek_drag_preserves_its_owner() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for dragging in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::Zoom;
        seed(&ctx, &mut app);
        app.fs_seek_drag_active = dragging;
        let geometry = app.still_seek_geometry_for_idx(full_rect(), 0, false);
        let pointer = geometry.panel_rect(full_rect()).center();
        let before = (app.fs_zoom, app.fs_pan);
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(pointer),
                wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
            ],
            |ui, ctx| {
                let navigation = pointer_handler(&mut app, ui, ctx);
                assert_eq!(
                    navigation,
                    if dragging {
                        FsPageNav::None
                    } else {
                        FsPageNav::Delta(1)
                    }
                );
                assert_eq!((app.fs_zoom, app.fs_pan), before);
            },
        );
    }
}

#[test]
fn setting_zoom_real_alt_shift_and_fractional_threshold_keep_existing_meanings() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for (modifiers, delta, expected_navigation, zooms) in [
        (egui::Modifiers::ALT, -120.0, FsPageNav::Delta(1), false),
        (egui::Modifiers::SHIFT, -120.0, FsPageNav::None, false),
        (egui::Modifiers::CTRL, 120.0, FsPageNav::None, true),
        (egui::Modifiers::NONE, 0.5, FsPageNav::None, false),
        (egui::Modifiers::NONE, 0.6, FsPageNav::None, true),
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::Zoom;
        seed(&ctx, &mut app);
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(full_rect().center()),
                wheel_event(Vec2::new(0.0, delta), modifiers),
            ],
            |ui, ctx| {
                assert_eq!(pointer_handler(&mut app, ui, ctx), expected_navigation);
                assert_eq!(app.fs_zoom > 1.0, zooms);
                assert_eq!(app.fs_pan, Vec2::ZERO);
            },
        );
    }
}

#[test]
fn setting_zoom_real_fallback_menu_consumes_without_zoom_or_navigation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for setting in [
        FullscreenWheelAction::InPageScroll,
        FullscreenWheelAction::ScrollThenPageTurn,
        FullscreenWheelAction::Zoom,
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = setting;
        seed(&ctx, &mut app);
        app.fs_context_menu_idx = Some(0);
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(full_rect().center()),
                wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
            ],
            |ui, ctx| {
                assert!(pointer_handler(&mut app, ui, ctx).is_none());
                assert_eq!((app.fs_zoom, app.fs_pan), (1.0, Vec2::ZERO));
                assert_eq!(app.fullscreen_idx, Some(0));
                assert!(!wheel_remaining(ctx), "the menu must not leak wheel input");
            },
        );
    }
}

#[test]
fn setting_zoom_outside_its_surface_matches_the_legacy_real_handler() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for surface in 0..7 {
        let mut legacy = None;
        for setting in [FullscreenWheelAction::PageTurn, FullscreenWheelAction::Zoom] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            seed(&ctx, &mut app);
            app.settings.fullscreen_wheel_action = setting;
            match surface {
                0 => app.reading_flow = ReadingFlow::Vertical,
                1 => app.reading_flow = ReadingFlow::Horizontal,
                2 => app.analysis_mode = true,
                3 => app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5),
                4 => app.fs_zoom_active = true,
                5 => app.view_trim_mode = true,
                6 => app.erase_mode = true,
                _ => unreachable!(),
            }
            app.fs_vertical_scroll = 200.0;
            assert!(!app.fs_in_page_scroll_surface());
            frame(
                &ctx,
                vec![
                    egui::Event::PointerMoved(full_rect().center()),
                    wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
                ],
                |ui, ctx| {
                    let navigation = pointer_handler(&mut app, ui, ctx);
                    let state = (
                        navigation,
                        app.fs_zoom,
                        app.fs_pan,
                        app.fs_vertical_scroll,
                        app.analysis_zoom,
                        app.analysis_pan,
                    );
                    if setting == FullscreenWheelAction::Zoom {
                        assert_eq!(Some(state), legacy, "surface={surface}");
                    } else {
                        legacy = Some(state);
                    }
                },
            );
        }
    }
}

#[test]
fn setting_zoom_prediction_and_same_frame_click_use_the_applied_zoom() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::Zoom;
    seed(&ctx, &mut app);
    let pointer = egui::pos2(450.0, 200.0);
    let button = |pressed| egui::Event::PointerButton {
        pos: pointer,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    // Register the production canvas before the press reaches its hit target.
    frame(&ctx, vec![egui::Event::PointerMoved(pointer)], |ui, ctx| {
        assert!(pointer_handler(&mut app, ui, ctx).is_none());
    });
    frame(&ctx, vec![button(true)], |ui, ctx| {
        assert!(pointer_handler(&mut app, ui, ctx).is_none());
    });
    frame(
        &ctx,
        vec![
            button(false),
            wheel_event(Vec2::new(0.0, 120.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let input = crate::wheel_input::WheelInput {
                delta: Vec2::new(0.0, 120.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            };
            let accepted = action(&app, input);
            assert_eq!(accepted.class, FsWheelInPageClass::SettingZoom);
            assert!(!app.fs_wheel_action_requests_navigation(&accepted));
            let viewport = app.fullscreen_media_rect(full_rect(), 0, false);
            let expected = app.propose_fs_wheel_click_transform(ctx, viewport, &[accepted]);
            assert!(expected.0 > 1.0);
            assert_eq!((app.fs_zoom, app.fs_pan), (1.0, Vec2::ZERO));
            assert!(pointer_handler(&mut app, ui, ctx).is_none());
            assert_eq!((app.fs_zoom, app.fs_pan), expected);
            assert_eq!(app.fullscreen_idx, Some(0), "the zoomed click cannot turn");
        },
    );
}

#[cfg(windows)]
#[test]
fn accepted_setting_zoom_after_alt_media_navigation_is_consumed_without_reinterpretation() {
    let _serial = crate::key_input::lock_test_input();
    for audio in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::Zoom;
        app.settings.ring_shortcuts.video_normal_wheel_action =
            crate::ring_shortcut::VideoNormalWheelActionId::AdjustVolume;
        let path = app.tmp.path().join(if audio {
            "setting-zoom.flac"
        } else {
            "setting-zoom.mp4"
        });
        app.items[1] = if audio {
            GridItem::Audio(path.clone())
        } else {
            GridItem::Video(path.clone())
        };
        let player = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
        player.set_volume(1.0);
        app.fs_cache.insert(
            1,
            FsCacheEntry::Video {
                player: Box::new(player),
                load_seq: 1,
            },
        );
        seed(&ctx, &mut app);
        let timeline = app.music_timeline_row_secs;
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(full_rect().center()),
                wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::ALT),
                wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
            ],
            |ui, ctx| {
                let mut proposal = deferred_wheel_proposal(&mut app, ui, ctx);
                let FsWheelFrameInput::Unconsumed(eligible) = &proposal.wheel_input else {
                    panic!("mixed proposal")
                };
                assert_eq!(eligible.len(), 2);
                assert_eq!(eligible[1].class, FsWheelInPageClass::SettingZoom);
                assert!(!app.fs_wheel_action_requests_navigation(&eligible[1]));
                app.claim_fs_wheel_frame_after_ui(&mut proposal);
                let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut None);
                assert_eq!(app.fullscreen_idx, Some(1));
                assert!(result.page_nav.is_none());
                assert!(!result.close);
                assert_eq!((app.fs_zoom, app.fs_pan), (1.0, Vec2::ZERO));
                assert_eq!(app.music_timeline_row_secs, timeline);
                let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get(&1) else {
                    panic!("resident media")
                };
                assert_eq!(
                    player.volume(),
                    1.0,
                    "image zoom cannot become media volume"
                );
                assert!(!wheel_remaining(ctx));
            },
        );
    }
}

#[test]
fn default_real_wheel_still_turns_pages_without_starting_zoom_or_animation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for (delta, expected) in [(-120.0, FsPageNav::Delta(1)), (120.0, FsPageNav::Delta(-1))] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::default();
        assert_eq!(
            app.settings.fullscreen_wheel_action,
            FullscreenWheelAction::PageTurn
        );
        app.fullscreen_idx = Some(1);
        seed(&ctx, &mut app);
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(full_rect().center()),
                wheel_event(Vec2::new(0.0, delta), egui::Modifiers::NONE),
            ],
            |ui, ctx| {
                assert_eq!(pointer_handler(&mut app, ui, ctx), expected);
                assert_eq!((app.fs_zoom, app.fs_pan), (1.0, Vec2::ZERO));
                assert!(app.fs_page_scroll_animation.is_none());
            },
        );
    }
}

#[test]
fn setting_zoom_media_items_stay_outside_the_ordinary_image_classifier() {
    for audio in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::Zoom;
        let path = app.tmp.path().join(if audio {
            "classifier.flac"
        } else {
            "classifier.mp4"
        });
        app.items[0] = if audio {
            GridItem::Audio(path)
        } else {
            GridItem::Video(path)
        };
        let input = crate::wheel_input::WheelInput {
            delta: egui::vec2(0.0, 120.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        };
        assert!(!app.fs_in_page_scroll_surface());
        assert_eq!(
            app.fs_wheel_segment_in_page_scroll(input, false),
            FsWheelInPageClass::Legacy
        );
    }
}

/// §1.842: after the cursor auto-hides, its last hover position is stale for
/// passive hover UI, but a wheel turned there is real input at that position.
/// The seek bar must keep its page-turn meaning for both the v5.5.0 Zoom
/// setting and the v5.4.0 InPageScroll setting.
#[test]
fn section1842_seek_bar_wheel_after_cursor_auto_hide_still_turns_the_page() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for setting in [
        FullscreenWheelAction::Zoom,
        FullscreenWheelAction::InPageScroll,
    ] {
        for delta in [-40.0, -120.0] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_wheel_action = setting;
            seed(&ctx, &mut app);
            let geometry = app.still_seek_geometry_for_idx(full_rect(), 0, false);
            let pointer = geometry.panel_rect(full_rect()).center();
            frame(&ctx, vec![egui::Event::PointerMoved(pointer)], |ui, ctx| {
                assert_eq!(pointer_handler(&mut app, ui, ctx), FsPageNav::None);
            });
            // The pointer stays still until the idle timer hides it.
            app.cursor_hide_reason = Some(crate::ui_fullscreen::FsCursorHide::Idle);
            let before = (app.fs_zoom, app.fs_pan);
            frame(
                &ctx,
                vec![wheel_event(Vec2::new(0.0, delta), egui::Modifiers::NONE)],
                |ui, ctx| {
                    assert_eq!(
                        pointer_handler(&mut app, ui, ctx),
                        FsPageNav::Delta(1),
                        "{setting:?} delta={delta}"
                    );
                    assert_eq!(
                        (app.fs_zoom, app.fs_pan),
                        before,
                        "{setting:?} delta={delta}"
                    );
                },
            );
        }
    }
}

#[test]
fn section1842_idle_without_pointer_activity_keeps_passive_panel_hover_suppressed() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for hidden in [false, true] {
        for right_edge in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::Hover;
            seed(&ctx, &mut app);
            let pointer = egui::pos2(if right_edge { 599.0 } else { 1.0 }, 200.0);
            frame(&ctx, vec![egui::Event::PointerMoved(pointer)], |_, _| {});
            app.cursor_hide_reason = hidden.then_some(FsCursorHide::Idle);
            let before = (app.fs_zoom, app.fs_pan);
            frame(&ctx, vec![], |ui, ctx| {
                assert_eq!(ctx.input(|input| input.pointer.hover_pos()), Some(pointer));
                assert!(!ctx.input(fullscreen_cursor_active));
                assert_eq!(pointer_handler(&mut app, ui, ctx), FsPageNav::None);
                assert_eq!(
                    app.adjustment_mode.is_open(),
                    !hidden && !right_edge,
                    "hidden={hidden} right_edge={right_edge}"
                );
                assert_eq!(
                    app.fs_info_panel.hover_active,
                    !hidden && right_edge,
                    "hidden={hidden} right_edge={right_edge}"
                );
                assert_eq!(app.cursor_hide_reason, hidden.then_some(FsCursorHide::Idle));
                assert_eq!((app.fs_zoom, app.fs_pan), before);
            });
        }
    }
}
