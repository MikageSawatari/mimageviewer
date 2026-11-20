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

#[cfg(windows)]
#[test]
fn ordinary_single_and_spread_paint_interpolate_without_publishing_intermediate_pan() {
    let _serial = crate::key_input::lock_test_input();
    struct ClearKeyboard;
    impl Drop for ClearKeyboard {
        fn drop(&mut self) {
            crate::key_input::clear_test_frame();
        }
    }
    let _clear = ClearKeyboard;

    // Match the production viewport fixture's root dispatch. Do not perform
    // page-local input, animation retirement, or paint in the test's order:
    // the full renderer owns all three and must preserve its previous stamp
    // until it validates the animation after input.
    fn render_frame(
        app: &mut App,
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
    ) -> Vec<DisplayedImageTransform> {
        app.frame_counter += 1;
        crate::key_input::set_test_frame_for_viewport(egui::ViewportId::ROOT, Vec::new());
        let output = ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(full_rect()),
                focused: true,
                time: Some(time),
                events,
                viewports: [(
                    egui::ViewportId::ROOT,
                    egui::ViewportInfo {
                        native_pixels_per_point: Some(1.0),
                        focused: Some(true),
                        minimized: Some(false),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| app.render_fullscreen_viewport_with_root_input(ui, None));
            },
        );
        // This only discards recorded egui paint/texture output; it constructs
        // no renderer, RenderState, device, session, or native application window.
        crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
        let pages: Vec<_> = app
            .fullscreen_page_layout
            .displayed_pages()
            .map(|page| page.transform)
            .collect();
        assert!(
            !pages.is_empty(),
            "the actual viewport paints resident pages"
        );
        assert!(
            app.fullscreen_page_layout.drawn_unit.is_some(),
            "the actual renderer republishes the current unit stamp"
        );
        assert!(
            app.fs_scroll_layout_matches(ctx),
            "the repainted unit remains admissible for the next input"
        );
        pages
    }

    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        for smooth in [false, true] {
            let ctx = egui::Context::default();
            let mut app = scroll_app(&ctx, mode);
            app.fs_zoom = 2.0;
            app.settings.fullscreen_smooth_page_scroll = smooth;
            app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
            app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageScrollDown = F24\n");
            let initial = render_frame(&mut app, &ctx, 0.0, vec![]);
            let expected_pages = if mode.is_spread() { 2 } else { 1 };
            assert_eq!(initial.len(), expected_pages);
            let from = app.fs_pan;
            for page in initial {
                near(normal_zoom_pan(page).unwrap().1, from);
            }

            // One real key action reaches the production keymap and page-local
            // writer in the same pass as the production retirement and paint.
            let input_pages = render_frame(
                &mut app,
                &ctx,
                1.0,
                super::handler_tests::tap(egui::Key::F24),
            );
            let to = app.fs_pan;
            assert!(to.y < from.y, "mode={mode:?} smooth={smooth}");
            let motion = to - from;
            assert_eq!(input_pages.len(), expected_pages);
            for page in input_pages {
                near(
                    normal_zoom_pan(page).unwrap().1,
                    if smooth { from } else { to },
                );
            }
            if smooth {
                let animation = app
                    .fs_page_scroll_animation
                    .as_ref()
                    .expect("the actual viewport keeps the newly accepted animation");
                assert_eq!((animation.from, animation.to), (from, to));
                assert_eq!(animation.start_time, Some(1.0));
            } else {
                assert!(app.fs_page_scroll_animation.is_none());
            }

            let mut previous_distance = motion.length();
            for time in [1.05, 1.10, 1.15, 1.21] {
                let pages = render_frame(&mut app, &ctx, time, vec![]);
                assert_eq!(pages.len(), expected_pages);
                assert_eq!(
                    app.fs_pan, to,
                    "the full viewport cannot publish an intermediate pan: {mode:?}, smooth={smooth}, time={time}"
                );
                let mut frame_distance: Option<f32> = None;
                for page in pages {
                    let painted = normal_zoom_pan(page).unwrap().1;
                    if smooth && time < 1.2 {
                        let progress = (painted - from).dot(motion) / motion.length_sq();
                        assert!(
                            progress > 0.0 && progress < 1.0,
                            "a subsequent real frame paints strictly between from/to: {mode:?}, time={time}, pan={painted:?}"
                        );
                        near(painted, from + motion * progress);
                        let distance = (to - painted).length();
                        assert!(
                            distance < previous_distance,
                            "successive real frames approach the logical target"
                        );
                        // All pages share this frame's sample. Update the
                        // previous-frame baseline only after inspecting the unit.
                        if let Some(expected) = frame_distance {
                            assert!((distance - expected).abs() < 0.01);
                        }
                        frame_distance = Some(distance);
                    } else {
                        near(painted, to);
                    }
                }
                if let Some(distance) = frame_distance {
                    previous_distance = distance;
                }
                assert_eq!(app.fs_page_scroll_animation.is_some(), smooth && time < 1.2);
            }
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
        start_time: Some(ctx.input(|i| i.time)),
        last_painted_pan: Vec2::ZERO,
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

#[cfg(windows)]
struct ForwardedScrollClockViewer {
    ctx: egui::Context,
    app: crate::app::AppTestEnvForTest,
    destination: egui::ViewportId,
    child_time: std::rc::Rc<std::cell::Cell<f64>>,
    child_events: std::rc::Rc<std::cell::RefCell<Vec<egui::Event>>>,
}

#[cfg(windows)]
impl ForwardedScrollClockViewer {
    fn new(mode: SpreadMode) -> Self {
        let ctx = egui::Context::default();
        let mut app = scroll_app(&ctx, mode);
        app.native_video_in_window_active = false;
        app.settings.video_in_window_mode = false;
        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
        // The headless integration already owns this viewport. Avoid the
        // production first-show path; no native window is created or activated.
        app.fs_viewport_shown = true;
        app.fs_viewport_presentation = Some(crate::app::ViewerPresentation::Fullscreen);
        app.fs_zoom = 2.0;
        app.settings.fullscreen_smooth_page_scroll = true;
        app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
        app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageScrollDown = F24\n");
        let destination = app.fullscreen_viewport_id();
        assert_ne!(destination, egui::ViewportId::ROOT);
        let child_time = std::rc::Rc::new(std::cell::Cell::new(10.0));
        let current_child_time = child_time.clone();
        let child_events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let queued_child_events = child_events.clone();
        ctx.set_embed_viewports(false);
        // Same nested immediate-pass fixture as wheel_frame_tests. This callback
        // runs egui only; it never constructs a GPU renderer, device or HWND.
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            assert_eq!(viewport.ids.this, destination);
            crate::key_input::set_test_frame_for_viewport(destination, Vec::new());
            let output = ctx.run_pass(
                egui::RawInput {
                    viewport_id: destination,
                    screen_rect: Some(full_rect()),
                    focused: true,
                    time: Some(current_child_time.get()),
                    events: std::mem::take(&mut *queued_child_events.borrow_mut()),
                    viewports: [(
                        destination,
                        egui::ViewportInfo {
                            parent: Some(viewport.ids.parent),
                            native_pixels_per_point: Some(1.0),
                            focused: Some(true),
                            minimized: Some(false),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                },
                |ctx| ctx.root_ui(|ui| (viewport.viewport_ui_cb)(ui)),
            );
            crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
        });
        Self {
            ctx,
            app,
            destination,
            child_time,
            child_events,
        }
    }

    fn root_raw(&self, time: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(full_rect()),
            focused: true,
            time: Some(time),
            events,
            viewports: [(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(1.0),
                    focused: Some(true),
                    minimized: Some(false),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }
    }

    fn paint_destination(&mut self, time: f64) -> Vec<DisplayedImageTransform> {
        self.paint_destination_from_root(time, time, vec![])
    }

    fn paint_destination_from_root(
        &mut self,
        root_time: f64,
        child_time: f64,
        events: Vec<egui::Event>,
    ) -> Vec<DisplayedImageTransform> {
        self.child_time.set(child_time);
        *self.child_events.borrow_mut() = events;
        self.app.frame_counter += 1;
        crate::key_input::set_test_frame_for_viewport(egui::ViewportId::ROOT, Vec::new());
        let raw = self.root_raw(root_time, vec![]);
        let output = self.ctx.run_pass(raw, |ctx| {
            ctx.root_ui(|ui| self.app.render_fullscreen_viewport(ui));
        });
        crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
        assert_eq!(
            self.ctx.input_for(self.destination, |input| input.time),
            child_time
        );
        assert_eq!(self.ctx.viewport_id(), egui::ViewportId::ROOT);
        assert_eq!(self.ctx.input(|input| input.time), root_time);
        assert!(self.app.fullscreen_page_layout.drawn_unit.is_some());
        assert!(self.app.fs_scroll_layout_matches(&self.ctx));
        let pages: Vec<_> = self
            .app
            .fullscreen_page_layout
            .displayed_pages()
            .map(|page| page.transform)
            .collect();
        assert_eq!(
            pages.len(),
            if self.app.spread_mode.is_spread() {
                2
            } else {
                1
            }
        );
        pages
    }

    fn press_on_root(&mut self, time: f64, stale_destination_time: f64) {
        self.app.frame_counter += 1;
        crate::key_input::set_test_frame_for_viewport(egui::ViewportId::ROOT, Vec::new());
        let raw = self.root_raw(time, super::handler_tests::tap(egui::Key::F24));
        let output = self.ctx.run_pass(raw, |ctx| {
            // Keep the last child input/geometry resident without drawing it.
            ctx.show_viewport_deferred(self.destination, Default::default(), |_, _| {});
            assert_eq!(ctx.viewport_id(), egui::ViewportId::ROOT);
            assert_eq!(ctx.input(|input| input.time), time);
            assert_eq!(
                ctx.input_for(self.destination, |input| input.time),
                stale_destination_time
            );
            assert!(time > stale_destination_time);
            assert!(self.app.fs_scroll_layout_matches(ctx));
            assert!(self.app.handle_fullscreen_root_key_input(ctx));
            assert!(!ctx.input(|input| input.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::F24,
                    pressed: true,
                    ..
                }
            ))));
            assert_eq!(
                ctx.input_for(self.destination, |input| input.time),
                stale_destination_time,
                "root forwarding does not update the destination's cached input"
            );
        });
        crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
    }
}

#[cfg(windows)]
#[test]
fn forwarded_root_scroll_waits_for_first_destination_paint_before_starting_animation() {
    // Immediate viewport callbacks are thread-local. Isolate this RawInput-only
    // integration from every other test's callback without an actual window.
    std::thread::spawn(|| {
        let _serial = crate::key_input::lock_test_input();
        struct ClearKeyboard;
        impl Drop for ClearKeyboard {
            fn drop(&mut self) {
                crate::key_input::clear_test_frame();
            }
        }
        let _clear = ClearKeyboard;
        crate::key_input::arm_test_synthetic_input(1823801, egui::ViewportId::ROOT);
        for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
            let mut viewer = ForwardedScrollClockViewer::new(mode);
            viewer.paint_destination(10.0);
            let from = viewer.app.fs_pan;
            viewer.press_on_root(11.0, 10.0);
            let to = viewer.app.fs_pan;
            assert!(to.y < from.y);
            let animation = viewer.app.fs_page_scroll_animation.as_ref().unwrap();
            assert_eq!(animation.viewport, viewer.destination);
            assert_eq!(
                animation.start_time, None,
                "input does not start the paint clock"
            );
            assert_eq!((animation.from, animation.to), (from, to));
            near(animation.last_painted_pan, from);
            for page in viewer.paint_destination(11.05) {
                near(normal_zoom_pan(page).unwrap().1, from);
            }
            let animation = viewer.app.fs_page_scroll_animation.as_ref().unwrap();
            assert_eq!(animation.start_time, Some(11.05));
            near(animation.last_painted_pan, from);
            for (time, eased) in [(11.10, 0.578125), (11.15, 0.875)] {
                let expected = from + (to - from) * eased;
                for page in viewer.paint_destination(time) {
                    let pan = normal_zoom_pan(page).unwrap().1;
                    let progress = (pan - from).dot(to - from) / (to - from).length_sq();
                    assert!(
                        progress > 0.0 && progress < 1.0,
                        "destination paints an intermediate pan: {mode:?}, pan={pan:?}"
                    );
                    near(pan, expected);
                }
                let animation = viewer.app.fs_page_scroll_animation.as_ref().unwrap();
                assert_eq!(animation.start_time, Some(11.05));
                near(animation.last_painted_pan, expected);
                assert_eq!(viewer.app.fs_pan, to);
            }
            for page in viewer.paint_destination(11.26) {
                near(normal_zoom_pan(page).unwrap().1, to);
            }
            assert_eq!(viewer.app.fs_pan, to);
            assert!(viewer.app.fs_page_scroll_animation.is_none());
        }
    })
    .join()
    .expect("headless forwarded-scroll paint-start test completes");
}

#[cfg(windows)]
#[test]
fn forwarded_root_scroll_retargets_from_the_last_actual_destination_paint() {
    std::thread::spawn(|| {
        let _serial = crate::key_input::lock_test_input();
        struct ClearKeyboard;
        impl Drop for ClearKeyboard {
            fn drop(&mut self) {
                crate::key_input::clear_test_frame();
            }
        }
        let _clear = ClearKeyboard;
        crate::key_input::arm_test_synthetic_input(1823802, egui::ViewportId::ROOT);
        for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
            let mut viewer = ForwardedScrollClockViewer::new(mode);
            viewer.paint_destination(10.0);
            let from = viewer.app.fs_pan;
            viewer.press_on_root(11.0, 10.0);
            let first_to = viewer.app.fs_pan;
            viewer.paint_destination(11.05);
            let pages = viewer.paint_destination(11.10);
            let last_painted = normal_zoom_pan(pages[0]).unwrap().1;
            near(last_painted, from + (first_to - from) * 0.578125);
            // Even a ROOT time past the old duration cannot finish or sample
            // this animation. Its last actual destination paint is the origin.
            viewer.press_on_root(12.0, 11.10);
            let second_to = viewer.app.fs_pan;
            assert!(second_to.y < first_to.y);
            let animation = viewer.app.fs_page_scroll_animation.as_ref().unwrap();
            assert_eq!(animation.start_time, None);
            assert_eq!(animation.viewport, viewer.destination);
            assert_eq!(animation.to, second_to);
            near(animation.from, last_painted);
            near(animation.last_painted_pan, last_painted);
            assert!(
                (animation.from - first_to).length() > 1.0,
                "ROOT's elapsed time cannot replace the last painted origin with the old target"
            );
            for page in viewer.paint_destination(12.05) {
                near(normal_zoom_pan(page).unwrap().1, last_painted);
            }
            assert_eq!(
                viewer
                    .app
                    .fs_page_scroll_animation
                    .as_ref()
                    .unwrap()
                    .start_time,
                Some(12.05)
            );
            for page in viewer.paint_destination(12.10) {
                let pan = normal_zoom_pan(page).unwrap().1;
                let progress = (pan - last_painted).dot(second_to - last_painted)
                    / (second_to - last_painted).length_sq();
                assert!(progress > 0.0 && progress < 1.0);
                near(pan, last_painted + (second_to - last_painted) * 0.578125);
            }
            assert_eq!(viewer.app.fs_pan, second_to);
            for page in viewer.paint_destination(12.26) {
                near(normal_zoom_pan(page).unwrap().1, second_to);
            }
            assert!(viewer.app.fs_page_scroll_animation.is_none());
            assert_eq!(viewer.app.fs_pan, second_to);
        }
    })
    .join()
    .expect("headless forwarded-scroll last-paint test completes");
}

#[cfg(windows)]
#[test]
fn deferred_child_wheel_uses_last_paint_after_returning_to_an_older_root_clock() {
    std::thread::spawn(|| {
        let _serial = crate::key_input::lock_test_input();
        struct ClearKeyboard;
        impl Drop for ClearKeyboard {
            fn drop(&mut self) {
                crate::key_input::clear_test_frame();
            }
        }
        let _clear = ClearKeyboard;
        crate::key_input::arm_test_synthetic_input(1823803, egui::ViewportId::ROOT);
        let mut viewer = ForwardedScrollClockViewer::new(SpreadMode::Single);
        viewer.app.fullscreen_idx = Some(viewer.app.items.len() - 1);
        viewer.app.settings.fullscreen_manual_nav_wrap = false;
        viewer.app.settings.fullscreen_wheel_action = FullscreenWheelAction::InPageScroll;
        viewer.paint_destination(10.0);
        let from = viewer.app.fs_pan;
        viewer.press_on_root(11.0, 10.0);
        let first_to = viewer.app.fs_pan;
        viewer.paint_destination(11.05);
        let events = vec![
            egui::Event::PointerMoved(full_rect().center()),
            wheel_event(egui::vec2(0.0, -1.0), egui::Modifiers::NONE),
            wheel_event(egui::vec2(0.0, -120.0), egui::Modifiers::ALT),
        ];
        // Local scroll plus navigation is deferred until the real renderer
        // returns to ROOT. Alt at the last page hits the ordinary boundary and
        // keeps this same unit/animation owner alive after the tail dispatch.
        let pages = viewer.paint_destination_from_root(11.06, 11.15, events);
        assert_eq!(viewer.app.fullscreen_idx, Some(viewer.app.items.len() - 1));
        let drawn = normal_zoom_pan(pages[0]).unwrap().1;
        let expected_drawn = from + (first_to - from) * 0.875;
        near(drawn, expected_drawn);
        let second_to = viewer.app.fs_pan;
        assert!(
            second_to.y < first_to.y,
            "deferred local wheel updates the logical destination"
        );
        let animation = viewer.app.fs_page_scroll_animation.as_ref().unwrap();
        assert_eq!(
            animation.start_time, None,
            "new tail-applied input has not passed through a destination paint yet"
        );
        near(animation.from, drawn);
        near(animation.last_painted_pan, drawn);
        assert_eq!(animation.to, second_to);
        // Sampling the old owner using ROOT 11.06 would rewind to t=0.01.
        let rewound = from + (first_to - from) * (1.0 - 0.95_f32.powi(3));
        assert!((animation.from - rewound).length() > 1.0);
        assert_eq!(viewer.ctx.input(|input| input.time), 11.06);
        assert_eq!(
            viewer.ctx.input_for(viewer.destination, |input| input.time),
            11.15
        );
        for page in viewer.paint_destination(11.20) {
            near(normal_zoom_pan(page).unwrap().1, drawn);
        }
        assert_eq!(
            viewer
                .app
                .fs_page_scroll_animation
                .as_ref()
                .unwrap()
                .start_time,
            Some(11.20)
        );
        for page in viewer.paint_destination(11.25) {
            near(
                normal_zoom_pan(page).unwrap().1,
                drawn + (second_to - drawn) * 0.578125,
            );
        }
        for page in viewer.paint_destination(11.41) {
            near(normal_zoom_pan(page).unwrap().1, second_to);
        }
        assert!(viewer.app.fs_page_scroll_animation.is_none());
        assert_eq!(viewer.app.fs_pan, second_to);
    })
    .join()
    .expect("headless deferred-wheel last-paint test completes");
}
