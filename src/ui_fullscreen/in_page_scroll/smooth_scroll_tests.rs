//! Deterministic egui viewport time, real scroll geometry, no desktop input.
use super::handler_tests::{app, full_rect, seed};
use super::*;

fn timed_frame(ctx: &egui::Context, time: f64, mut body: impl FnMut(&egui::Context)) {
    let output = ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(full_rect()),
            time: Some(time),
            focused: true,
            ..Default::default()
        },
        |ctx| body(ctx),
    );
    crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
}

fn close(actual: Vec2, expected: Vec2) {
    assert!(
        (actual - expected).abs().max_elem() < 0.001,
        "actual={actual:?} expected={expected:?}"
    );
}

#[test]
fn default_is_instant_and_smooth_writes_final_pan_before_interpolating_paint() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    timed_frame(&ctx, 1.0, |ctx| {
        assert!(!app.settings.fullscreen_smooth_page_scroll);
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert!(app.fs_page_scroll_animation.is_none());
        assert_eq!(app.fs_page_scroll_display_pan(ctx), app.fs_pan);
    });
    app.fs_pan = Vec2::ZERO;
    app.settings.fullscreen_smooth_page_scroll = true;
    let mut target = Vec2::ZERO;
    timed_frame(&ctx, 2.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        target = app.fs_pan;
        assert!(target.y < 0.0);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        assert_eq!(animation.from, Vec2::ZERO);
        assert_eq!(animation.to, target);
        assert_eq!(animation.viewport, egui::ViewportId::ROOT);
        assert_eq!(animation.start_time, 2.0);
        assert_eq!(app.fs_page_scroll_display_pan(ctx), Vec2::ZERO);
    });
    timed_frame(&ctx, 2.1, |ctx| {
        app.retire_fs_page_scroll_animation(ctx);
        assert!(app.fs_page_scroll_animation.is_some());
        assert_eq!(app.fs_pan, target);
        close(app.fs_page_scroll_display_pan(ctx), target * 0.875);
    });
    timed_frame(&ctx, 2.21, |ctx| {
        app.retire_fs_page_scroll_animation(ctx);
        assert!(app.fs_page_scroll_animation.is_none());
        assert_eq!(app.fs_page_scroll_display_pan(ctx), target);
    });
}

#[test]
fn repeated_scroll_accumulates_from_logical_target_and_restarts_from_visible_pan() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(&ctx, &mut app);
    let mut first = Vec2::ZERO;
    timed_frame(&ctx, 1.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        first = app.fs_pan;
    });
    timed_frame(&ctx, 1.1, |ctx| {
        let visible = app.fs_page_scroll_display_pan(ctx);
        close(visible, first * 0.875);
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        close(app.fs_pan, first * 2.0);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        close(animation.from, visible);
        close(animation.to, first * 2.0);
        assert_eq!(animation.start_time, 1.1);
        close(app.fs_page_scroll_display_pan(ctx), visible);
    });
}

#[test]
fn trackpad_writer_is_instant_and_retires_an_existing_animation() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(&ctx, &mut app);
    timed_frame(&ctx, 1.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert!(app.fs_page_scroll_animation.is_some());
    });
    timed_frame(&ctx, 1.05, |ctx| {
        let (zoom, pan) = (app.fs_zoom, app.fs_pan);
        let next = app.fs_page_scroll_wheel_pan(ctx, zoom, pan, egui::vec2(0.0, -1.0));
        assert_ne!(next, app.fs_pan);
        app.write_fs_page_scroll_pan_with_animation(ctx, next, false);
        assert_eq!(app.fs_pan, next);
        assert!(app.fs_page_scroll_animation.is_none());
        assert_eq!(app.fs_page_scroll_display_pan(ctx), next);
    });
}

#[test]
fn changed_pan_is_used_immediately_and_zoom_rotation_or_disabled_setting_retires() {
    for change in 0..5 {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_smooth_page_scroll = true;
        seed(&ctx, &mut app);
        timed_frame(&ctx, 1.0, |ctx| {
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
            assert!(app.fs_page_scroll_animation.is_some());
        });
        timed_frame(&ctx, 1.05, |ctx| {
            match change {
                0 => {
                    app.fs_pan = Vec2::ZERO;
                    assert_eq!(app.fs_page_scroll_display_pan(ctx), Vec2::ZERO);
                }
                1 => app.fs_zoom = 2.0,
                2 => app.fs_free_rotation = 0.1,
                3 => app.settings.fullscreen_smooth_page_scroll = false,
                _ => app.fullscreen_idx = None,
            }
            app.retire_fs_page_scroll_animation(ctx);
            assert!(app.fs_page_scroll_animation.is_none(), "change {change}");
            assert_eq!(app.fs_page_scroll_display_pan(ctx), app.fs_pan);
        });
    }
}

#[test]
fn equal_geometry_in_a_different_viewport_cannot_retain_animation() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(&ctx, &mut app);
    timed_frame(&ctx, 1.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert_eq!(
            app.fs_page_scroll_animation.as_ref().unwrap().viewport,
            egui::ViewportId::ROOT
        );
        // Keep the exact receipt and geometry. Viewport identity alone invalidates.
        app.fs_page_scroll_animation.as_mut().unwrap().viewport =
            egui::ViewportId::from_hash_of("other-same-geometry");
        assert!(app.fs_scroll_layout_matches(ctx));
        app.retire_fs_page_scroll_animation(ctx);
        assert!(app.fs_page_scroll_animation.is_none());
    });
}

#[test]
fn resized_or_changed_dpi_viewport_retires_without_requiring_a_new_paint() {
    for dpi in [1.0, 1.5] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_smooth_page_scroll = true;
        seed(&ctx, &mut app);
        timed_frame(&ctx, 1.0, |ctx| {
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        });
        let mut raw = egui::RawInput {
            screen_rect: Some(if dpi == 1.0 {
                Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(610.0, 400.0))
            } else {
                full_rect()
            }),
            time: Some(1.05),
            ..Default::default()
        };
        raw.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(dpi),
                ..Default::default()
            },
        );
        let output = ctx.run_pass(raw, |ctx| {
            app.retire_fs_page_scroll_animation(ctx);
            assert!(app.fs_page_scroll_animation.is_none(), "dpi={dpi}");
        });
        crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
    }
}

#[test]
fn interpolation_does_not_request_material_or_pdf_producers() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(&ctx, &mut app);
    let before = (
        app.fs_cache.len(),
        app.fs_pending.len(),
        app.final_ai_pending.len(),
        app.final_effect_pending.len(),
        app.edit_result_cache.len(),
        app.final_composite_cache.len(),
    );
    timed_frame(&ctx, 1.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
    });
    for time in [1.03, 1.06, 1.09, 1.12, 1.15, 1.18, 1.21] {
        timed_frame(&ctx, time, |ctx| {
            app.retire_fs_page_scroll_animation(ctx);
            let _ = app.fs_page_scroll_display_pan(ctx);
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
}
