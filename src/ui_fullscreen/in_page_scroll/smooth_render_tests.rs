//! Paint and navigator consumers exercise the animated transform, not a second model.
use super::handler_tests::{app as scroll_app, full_rect, paint, seed, wheel_event};
use super::*;
use crate::settings::FullscreenWheelAction;

pub(super) fn timed_ui(
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    mut body: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let mut raw = egui::RawInput {
        screen_rect: Some(full_rect()),
        time: Some(time),
        focused: true,
        events,
        ..Default::default()
    };
    raw.viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .focused = Some(true);
    let output = ctx.run_pass(raw, |ctx| {
        ctx.root_ui(|root| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(root, |ui| body(ui, ctx));
        });
    });
    crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
}

fn near(actual: Vec2, expected: Vec2) {
    assert!(
        (actual - expected).abs().max_elem() < 0.01,
        "actual={actual:?} expected={expected:?}"
    );
}

#[test]
fn ordinary_single_and_spread_paint_interpolate_without_publishing_intermediate_pan() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        let mut app = scroll_app(&ctx, mode);
        app.fs_zoom = 2.0;
        app.settings.fullscreen_smooth_page_scroll = true;
        seed(&ctx, &mut app);
        let mut target = Vec2::ZERO;
        timed_ui(&ctx, 1.0, vec![], |ui, ctx| {
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
            target = app.fs_pan;
            assert!(target.y < 0.0);
            app.retire_fs_page_scroll_animation(ctx);
            paint(&mut app, ui, ctx);
            assert_eq!(app.fs_pan, target);
            for page in app.fullscreen_page_layout.displayed_pages() {
                near(normal_zoom_pan(page.transform).unwrap().1, Vec2::ZERO);
            }
        });
        for time in [1.05, 1.10, 1.15, 1.21] {
            timed_ui(&ctx, time, vec![], |ui, ctx| {
                app.retire_fs_page_scroll_animation(ctx);
                let visible = app.fs_page_scroll_display_pan(ctx);
                paint(&mut app, ui, ctx);
                assert_eq!(
                    app.fs_pan, target,
                    "paint cannot publish its intermediate transform"
                );
                for page in app.fullscreen_page_layout.displayed_pages() {
                    near(normal_zoom_pan(page.transform).unwrap().1, visible);
                }
                assert_eq!(app.fs_page_scroll_animation.is_none(), time >= 1.2);
            });
        }
    }
}

#[test]
fn asymmetric_spread_wrap_paint_keeps_final_pan_and_existing_material_producers() {
    use super::landing_tests::install_material;
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        let mut app = scroll_app(&ctx, mode);
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
        app.settings.fullscreen_smooth_page_scroll = true;
        // Keep both textures below the headless 2048-side limit while retaining
        // the tall/short spread and its overflowing painted dimensions.
        app.fs_zoom = 2.0;
        install_material(&mut app, &ctx, 0, [500, 1100]);
        install_material(&mut app, &ctx, 1, [500, 350]);
        seed(&ctx, &mut app);
        let axes = scroll_geometry(&app.fullscreen_page_layout).unwrap().axes;
        let horizontal = if mode == SpreadMode::Ltr { -1.0 } else { 1.0 };
        let start = egui::vec2(axes[0].unwrap().start(horizontal), axes[1].unwrap().min);
        let zoom = app.fs_zoom;
        app.set_fs_pan_from_input(zoom, start);
        seed(&ctx, &mut app);
        let mut target = start;
        let mut before = (0, 0, 0, 0, 0, 0);
        timed_ui(&ctx, 1.0, vec![], |ui, ctx| {
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageNScrollNext);
            target = app.fs_pan;
            assert!(
                (target.x - start.x) * horizontal > 0.0,
                "wrap advances column"
            );
            assert!(target.y > start.y, "wrap resets primary axis");
            before = (
                app.fs_cache.len(),
                app.fs_pending.len(),
                app.final_ai_pending.len(),
                app.final_effect_pending.len(),
                app.edit_result_cache.len(),
                app.final_composite_cache.len(),
            );
            app.retire_fs_page_scroll_animation(ctx);
            paint(&mut app, ui, ctx);
            assert_eq!(app.fs_pan, target);
        });
        let probe = crate::app::PdfRerenderRequestProbe::start();
        for time in [1.02, 1.04, 1.06, 1.08, 1.10, 1.12, 1.14, 1.16, 1.18, 1.21] {
            timed_ui(&ctx, time, vec![], |ui, ctx| {
                app.retire_fs_page_scroll_animation(ctx);
                let visible = app.fs_page_scroll_display_pan(ctx);
                paint(&mut app, ui, ctx);
                assert_eq!(app.fs_pan, target, "asymmetric wrap at {time}");
                for page in app.fullscreen_page_layout.displayed_pages() {
                    near(normal_zoom_pan(page.transform).unwrap().1, visible);
                }
            });
        }
        assert_eq!(
            before,
            (
                app.fs_cache.len(),
                app.fs_pending.len(),
                app.final_ai_pending.len(),
                app.final_effect_pending.len(),
                app.edit_result_cache.len(),
                app.final_composite_cache.len()
            )
        );
        assert!(probe.take_requests().is_empty());
    }
}

#[test]
fn actual_trackpad_handler_is_instant_for_both_page_scroll_settings() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for setting in [
        FullscreenWheelAction::InPageScroll,
        FullscreenWheelAction::ScrollThenPageTurn,
    ] {
        let ctx = egui::Context::default();
        let mut app = scroll_app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_smooth_page_scroll = true;
        app.settings.fullscreen_wheel_action = setting;
        seed(&ctx, &mut app);
        timed_ui(&ctx, 1.0, vec![], |_, ctx| {
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        });
        let from = app.fs_pan;
        let mut event = wheel_event(egui::vec2(0.0, -1.0), egui::Modifiers::NONE);
        if let egui::Event::MouseWheel { source, .. } = &mut event {
            *source = egui::MouseWheelSource::Trackpad;
        }
        timed_ui(
            &ctx,
            1.05,
            vec![egui::Event::PointerMoved(full_rect().center()), event],
            |ui, ctx| {
                let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                let (nav, close) = app.handle_fs_wheel_and_click(
                    ui,
                    ctx,
                    full_rect(),
                    &state,
                    false,
                    0,
                    &mut None,
                );
                assert!(nav.is_none() && !close);
                assert!(app.fs_pan.y < from.y);
                assert!(app.fs_page_scroll_animation.is_none());
                let logical = app.fs_pan;
                paint(&mut app, ui, ctx);
                near(
                    normal_zoom_pan(
                        app.fullscreen_page_layout
                            .displayed_pages()
                            .next()
                            .unwrap()
                            .transform,
                    )
                    .unwrap()
                    .1,
                    logical,
                );
            },
        );
    }
}

fn navigator_app(ctx: &egui::Context, smooth: bool, late: bool) -> crate::app::AppTestEnvForTest {
    let mut app = scroll_app(ctx, SpreadMode::Single);
    app.fs_zoom = 2.0;
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(ctx, &mut app);
    timed_ui(ctx, 1.0, vec![], |ui, ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        app.retire_fs_page_scroll_animation(ctx);
        paint(&mut app, ui, ctx);
    });
    assert!(app.fs_page_scroll_animation.is_some());
    if late {
        timed_ui(ctx, 1.19, vec![], |ui, ctx| {
            app.retire_fs_page_scroll_animation(ctx);
            paint(&mut app, ui, ctx);
        });
        assert!(app.fs_page_scroll_animation.is_some());
    }
    if !smooth {
        // The same actually painted geometry, with its own pan as the baseline.
        let painted_pan = normal_zoom_pan(
            app.fullscreen_page_layout
                .displayed_pages()
                .next()
                .unwrap()
                .transform,
        )
        .unwrap()
        .1;
        app.fs_pan = painted_pan;
        app.fs_page_scroll_animation = None;
    }
    app.settings.fullscreen_navigator_visible = true;
    app
}

fn button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn navigator_double_click_uses_the_painted_pan_during_scroll_interpolation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for late in [false, true] {
        let mut control = None;
        for smooth in [false, true] {
            let ctx = egui::Context::default();
            let mut app = navigator_app(&ctx, smooth, late);
            let base = if late { 1.21 } else { 1.01 };
            let layout = app.fs_navigator_layout(full_rect()).unwrap();
            let pos = layout.content_rect.left_top() + layout.content_rect.size() * 0.35;
            let (page, source_pos) = layout.nearest_page_at(pos).unwrap();
            let uv = page.navigator.screen_to_source_normalized(source_pos);
            for (time, pressed) in [
                (base, true),
                (base + 0.02, false),
                (base + 0.04, true),
                (base + 0.06, false),
            ] {
                timed_ui(
                    &ctx,
                    time,
                    vec![
                        egui::Event::PointerMoved(pos),
                        button(pos, egui::PointerButton::Primary, pressed),
                    ],
                    |ui, ctx| {
                        assert!(app.handle_fs_navigator_input(ui, ctx, full_rect(), 0, None));
                    },
                );
            }
            let result = (app.fs_zoom, app.fs_pan);
            if smooth {
                assert_eq!(Some(result), control);
            } else {
                control = Some(result);
            }
            timed_ui(&ctx, base + 0.07, vec![], |ui, ctx| {
                app.retire_fs_page_scroll_animation(ctx);
                paint(&mut app, ui, ctx);
                let page = app.fullscreen_page_layout.displayed_pages().next().unwrap();
                assert!(
                    page.transform
                        .source_normalized_to_screen(uv)
                        .distance(full_rect().center())
                        < 1.1
                );
            });
        }
    }
}

#[test]
fn navigator_range_zoom_uses_the_painted_pan_during_scroll_interpolation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for late in [false, true] {
        let mut control = None;
        for smooth in [false, true] {
            let ctx = egui::Context::default();
            let mut app = navigator_app(&ctx, smooth, late);
            let base = if late { 1.21 } else { 1.01 };
            let layout = app.fs_navigator_layout(full_rect()).unwrap();
            let start = layout.content_rect.left_top() + layout.content_rect.size() * 0.35;
            let end = layout.content_rect.left_top() + layout.content_rect.size() * 0.50;
            let selection = Rect::from_two_pos(start, end);
            let (page, source_pos) = layout.nearest_page_at(selection.center()).unwrap();
            let uv = page.navigator.screen_to_source_normalized(source_pos);
            timed_ui(
                &ctx,
                base,
                vec![
                    egui::Event::PointerMoved(start),
                    button(start, egui::PointerButton::Secondary, true),
                ],
                |ui, ctx| {
                    assert!(app.handle_fs_navigator_input(ui, ctx, full_rect(), 0, None));
                },
            );
            timed_ui(
                &ctx,
                base + 0.03,
                vec![
                    egui::Event::PointerMoved(end),
                    button(end, egui::PointerButton::Secondary, false),
                ],
                |ui, ctx| {
                    assert!(app.handle_fs_navigator_input(ui, ctx, full_rect(), 0, None));
                },
            );
            assert!(app.fs_zoom > 2.0);
            let result = (app.fs_zoom, app.fs_pan);
            if smooth {
                assert_eq!(Some(result), control);
            } else {
                control = Some(result);
            }
            timed_ui(&ctx, base + 0.04, vec![], |ui, ctx| {
                app.retire_fs_page_scroll_animation(ctx);
                let (_, sources) = paint(&mut app, ui, ctx);
                app.draw_fs_navigator(ui, ctx, full_rect(), 0, &sources);
                assert!(!app.fullscreen_navigator_interaction.is_active());
                let page = app.fullscreen_page_layout.displayed_pages().next().unwrap();
                assert!(
                    page.transform
                        .source_normalized_to_screen(uv)
                        .distance(full_rect().center())
                        < 1.1
                );
            });
        }
    }
}

#[test]
fn navigator_pending_center_draw_uses_the_painted_pan_during_scroll_interpolation() {
    for late in [false, true] {
        let mut control = None;
        for smooth in [false, true] {
            let ctx = egui::Context::default();
            let mut app = navigator_app(&ctx, smooth, late);
            let base = if late { 1.21 } else { 1.01 };
            app.fullscreen_navigator_interaction
                .set_flat(Some(FsNavigatorInteraction {
                    geometry: FsNavigatorGeometryIntent::Center {
                        page_idx: 0,
                        source_uv: egui::pos2(0.6, 0.4),
                    },
                    gesture: FsNavigatorPointerGesture::Idle,
                }));
            timed_ui(&ctx, base + 0.03, vec![], |ui, ctx| {
                app.draw_fs_navigator(
                    ui,
                    ctx,
                    full_rect(),
                    0,
                    &FsNavigatorTextureSources::default(),
                );
            });
            let result = (app.fs_zoom, app.fs_pan);
            if smooth {
                assert_eq!(Some(result), control);
            } else {
                control = Some(result);
            }
            assert!(!app.fullscreen_navigator_interaction.is_active());
        }
    }
}

#[test]
fn navigator_baseline_without_animation_or_with_z_keeps_the_logical_pan() {
    let ctx = egui::Context::default();
    let mut app = navigator_app(&ctx, false, false);
    let normal = app
        .fullscreen_page_layout
        .displayed_pages()
        .next()
        .unwrap()
        .transform;
    let logical = egui::vec2(31.0, 47.0);
    app.fs_pan = logical;
    assert_eq!(app.fs_navigator_transform_pan(&ctx, normal), logical);
    let input = DisplayedImageTransformInput {
        page_idx: normal.page_idx,
        viewport_rect: normal.viewport_rect,
        source_size: normal.source_size,
        texture_size: normal.texture_size,
        rotation: normal.rotation,
        free_rotation_rad: 0.0,
        content_bbox: normal.content_bbox,
        fit_mode: normal.fit_input,
        fit_scale_limits: FullscreenFitScaleLimits::default(),
        pixels_per_point: 1.0,
        placement: ResolvedDisplayPlacement::Z {
            active: true,
            factor: 3.0,
            zoom_pan: Some((2.0, Vec2::ZERO)),
            singleton_side: SingletonSpreadPlacement::Center,
            singleton_gap: 0.0,
        },
        pixel_fit: RectPixelFit::Proportional,
    };
    let z = DisplayedImageTransform::from_resolved_rect(input, normal.paint_rect).unwrap();
    assert!(normal_zoom_pan(z).is_none());
    assert_eq!(app.fs_navigator_transform_pan(&ctx, z), logical);
    let stamp = app.fullscreen_page_layout.drawn_unit.clone().unwrap();
    app.fs_page_scroll_animation = Some(FsPageScrollAnimation {
        from: Vec2::ZERO,
        to: logical,
        start_time: ctx.input(|i| i.time),
        origin: stamp,
        viewport: egui::ViewportId::ROOT,
    });
    assert_eq!(app.fs_navigator_transform_pan(&ctx, z), logical);
}

#[test]
fn actual_pdf_scroll_paint_adds_no_resolution_or_processing_request() {
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.items[0] = GridItem::PdfPage {
        pdf_path: app.tmp.path().join("smooth-page.pdf"),
        page_num: 0,
        content_type: None,
    };
    app.settings.fullscreen_smooth_page_scroll = true;
    app.fs_pdf_display_target = Some(crate::pdf_loader::PdfDisplayTarget::from_logical_size(
        600.0,
        400.0,
        1.0,
        app.pdf_display_fit_mode(),
    ));
    let probe = crate::app::PdfRerenderRequestProbe::start();
    timed_ui(&ctx, 0.0, vec![], |ui, ctx| {
        app.ensure_pdf_display_resolution(0, None);
        paint(&mut app, ui, ctx);
    });
    assert!(
        !probe.take_requests().is_empty(),
        "the ordinary display demand is really issued and intercepted"
    );
    let before = (
        app.fs_pending.len(),
        app.final_ai_pending.len(),
        app.final_effect_pending.len(),
        app.edit_result_cache.len(),
        app.final_composite_cache.len(),
    );
    timed_ui(&ctx, 1.0, vec![], |ui, ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert!(app.fs_page_scroll_animation.is_some());
        app.retire_fs_page_scroll_animation(ctx);
        app.ensure_pdf_display_resolution(0, None);
        paint(&mut app, ui, ctx);
    });
    let target = app.fs_pan;
    for time in [1.03, 1.06, 1.09, 1.12, 1.15, 1.18, 1.21] {
        timed_ui(&ctx, time, vec![], |ui, ctx| {
            app.retire_fs_page_scroll_animation(ctx);
            app.ensure_pdf_display_resolution(0, None);
            paint(&mut app, ui, ctx);
            assert_eq!(app.fs_pan, target);
        });
    }
    assert!(
        probe.take_requests().is_empty(),
        "scrolling changes pan without changing the PDF zoom demand"
    );
    assert_eq!(
        before,
        (
            app.fs_pending.len(),
            app.final_ai_pending.len(),
            app.final_effect_pending.len(),
            app.edit_result_cache.len(),
            app.final_composite_cache.len()
        )
    );
}
