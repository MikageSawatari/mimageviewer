//! Deterministic egui viewport time, real scroll geometry, no desktop input.
use super::handler_tests::{app, full_rect, paint, seed};
use super::*;

fn timed_frame(ctx: &egui::Context, time: f64, mut body: impl FnMut(&egui::Context)) {
    timed_viewport_frame(ctx, egui::ViewportId::ROOT, time, |ctx| body(ctx));
}

fn timed_viewport_frame(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    time: f64,
    mut body: impl FnMut(&egui::Context),
) {
    let mut raw = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(full_rect()),
        time: Some(time),
        focused: true,
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            native_pixels_per_point: Some(1.0),
            focused: Some(true),
            ..Default::default()
        },
    );
    let output = ctx.run_pass(raw, |ctx| body(ctx));
    crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
}

fn timed_paint(ctx: &egui::Context, time: f64, app: &mut App) {
    timed_frame(ctx, time, |ctx| {
        // The real renderer retires/starts the effect before clearing the old
        // layout, then records the pan of the transform that it actually drew.
        app.retire_fs_page_scroll_animation(ctx);
        ctx.root_ui(|root| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(root, |ui| {
                    paint(app, ui, ctx);
                });
        });
    });
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
        assert_eq!(animation.start_time, None);
        assert_eq!(animation.last_painted_pan, Vec2::ZERO);
        assert_eq!(animation.last_painted_time, None);
        assert_eq!(app.fs_page_scroll_display_pan(ctx), Vec2::ZERO);
    });
    timed_paint(&ctx, 2.0, &mut app);
    assert_eq!(
        app.fs_page_scroll_animation.as_ref().unwrap().start_time,
        Some(2.0)
    );
    assert_eq!(
        app.fs_page_scroll_animation
            .as_ref()
            .unwrap()
            .last_painted_time,
        Some(2.0)
    );
    timed_paint(&ctx, 2.1, &mut app);
    timed_frame(&ctx, 2.1, |ctx| {
        assert!(app.fs_page_scroll_animation.is_some());
        assert_eq!(app.fs_pan, target);
        close(app.fs_page_scroll_display_pan(ctx), target * 0.875);
        close(
            app.fs_page_scroll_animation
                .as_ref()
                .unwrap()
                .last_painted_pan,
            target * 0.875,
        );
        assert_eq!(
            app.fs_page_scroll_animation
                .as_ref()
                .unwrap()
                .last_painted_time,
            Some(2.1)
        );
    });
    timed_paint(&ctx, 2.21, &mut app);
    timed_frame(&ctx, 2.21, |ctx| {
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
        assert_eq!(
            app.fs_page_scroll_animation.as_ref().unwrap().start_time,
            None
        );
        assert_eq!(
            app.fs_page_scroll_animation
                .as_ref()
                .unwrap()
                .last_painted_time,
            None
        );
    });
    timed_paint(&ctx, 1.0, &mut app);
    timed_paint(&ctx, 1.1, &mut app);
    let visible = app
        .fs_page_scroll_animation
        .as_ref()
        .unwrap()
        .last_painted_pan;
    close(visible, first * 0.875);
    assert_eq!(
        app.fs_page_scroll_animation
            .as_ref()
            .unwrap()
            .last_painted_time,
        Some(1.1)
    );
    timed_frame(&ctx, 1.15, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        close(app.fs_pan, first * 2.0);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        close(animation.from, visible);
        close(animation.to, first * 2.0);
        // Retargeting carries the pan/time pair from the actual 1.1 paint,
        // rather than sampling the 1.15 input pass or restarting at next paint.
        assert_eq!(animation.start_time, Some(1.1));
        close(animation.last_painted_pan, visible);
        assert_eq!(animation.last_painted_time, Some(1.1));
    });
    timed_paint(&ctx, 1.2, &mut app);
    assert_eq!(
        app.fs_page_scroll_animation.as_ref().unwrap().start_time,
        Some(1.1)
    );
    close(
        app.fs_page_scroll_animation
            .as_ref()
            .unwrap()
            .last_painted_pan,
        visible + (first * 2.0 - visible) * 0.875,
    );
    assert_eq!(
        app.fs_page_scroll_animation
            .as_ref()
            .unwrap()
            .last_painted_time,
        Some(1.2)
    );
    timed_paint(&ctx, 1.31, &mut app);
    assert!(app.fs_page_scroll_animation.is_none());
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
        assert_eq!(
            app.fs_page_scroll_animation.as_ref().unwrap().start_time,
            None
        );
        assert_eq!(
            app.fs_page_scroll_animation
                .as_ref()
                .unwrap()
                .last_painted_time,
            None
        );
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
                _ => {
                    // Closing changes the destination too. Exercise the real
                    // close owner, rather than an incomplete idx-only teardown.
                    app.close_fullscreen();
                    assert!(app.fullscreen_idx.is_none());
                }
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
    for time in [1.0, 1.03, 1.06, 1.09, 1.12, 1.15, 1.18, 1.21] {
        timed_frame(&ctx, time, |ctx| {
            app.retire_fs_page_scroll_animation(ctx);
            let _ = app.fs_page_scroll_display_pan(ctx);
            if let Some(animation) = app.fs_page_scroll_animation.as_ref() {
                assert_eq!(
                    animation.last_painted_time, None,
                    "sampling alone is not a paint receipt"
                );
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
    assert!(app.fs_page_scroll_animation.is_none());
}

#[test]
fn input_only_retargeting_neither_starts_nor_expires_the_effect() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    app.settings.continuous_reading_key_scroll_percent = 10;
    seed(&ctx, &mut app);
    for (time, action) in [
        (1.0, KeyAction::FsPageScrollDown),
        (100.0, KeyAction::FsPageScrollDown),
        (1000.0, KeyAction::FsPageScrollDown),
    ] {
        timed_frame(&ctx, time, |ctx| {
            let before = app.fs_pan;
            app.apply_fs_page_scroll_key(ctx, action);
            assert_ne!(app.fs_pan, before);
            let animation = app.fs_page_scroll_animation.as_ref().unwrap();
            assert_eq!(animation.start_time, None);
            assert_eq!(animation.from, Vec2::ZERO);
            assert_eq!(animation.last_painted_pan, Vec2::ZERO);
            assert_eq!(animation.last_painted_time, None);
            assert_eq!(animation.to, app.fs_pan);
        });
    }
    let target = app.fs_pan;
    timed_paint(&ctx, 1001.0, &mut app);
    assert_eq!(
        app.fs_page_scroll_animation.as_ref().unwrap().start_time,
        Some(1001.0)
    );
    assert_eq!(
        app.fs_page_scroll_animation
            .as_ref()
            .unwrap()
            .last_painted_time,
        Some(1001.0)
    );
    timed_paint(&ctx, 1001.1, &mut app);
    let last_painted = app
        .fs_page_scroll_animation
        .as_ref()
        .unwrap()
        .last_painted_pan;
    close(last_painted, target * 0.875);
    assert_eq!(
        app.fs_page_scroll_animation
            .as_ref()
            .unwrap()
            .last_painted_time,
        Some(1001.1)
    );
    // Even a started effect's old deadline is not consumed by input. The new
    // request starts from the last actual paint, not a never-painted sample.
    timed_frame(&ctx, 2000.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollUp);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        assert_eq!(animation.start_time, Some(1001.1));
        close(animation.from, last_painted);
        close(animation.last_painted_pan, last_painted);
        assert_eq!(animation.last_painted_time, Some(1001.1));
    });
    let final_pan = app.fs_pan;
    // The input kept the recorded clock even though its deadline had passed.
    // Only the next destination draw consumes that deadline, immediately.
    timed_paint(&ctx, 2001.0, &mut app);
    assert!(app.fs_page_scroll_animation.is_none());
    assert_eq!(app.fs_pan, final_pan);
}

#[test]
fn non_destination_pass_cannot_start_or_expire_the_effect() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(&ctx, &mut app);
    timed_frame(&ctx, 1.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
    });
    let target = app.fs_pan;
    let other = egui::ViewportId::from_hash_of("non-destination-scroll-clock");
    timed_viewport_frame(&ctx, other, 100.0, |ctx| {
        app.retire_fs_page_scroll_animation(ctx);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        assert_eq!(animation.start_time, None);
        assert_eq!(animation.last_painted_pan, Vec2::ZERO);
        assert_eq!(animation.last_painted_time, None);
        assert_eq!(app.fs_page_scroll_display_pan(ctx), target);
    });
    timed_paint(&ctx, 200.0, &mut app);
    assert_eq!(
        app.fs_page_scroll_animation.as_ref().unwrap().start_time,
        Some(200.0)
    );
    assert_eq!(
        app.fs_page_scroll_animation
            .as_ref()
            .unwrap()
            .last_painted_time,
        Some(200.0)
    );
    timed_viewport_frame(&ctx, other, 1000.0, |ctx| {
        app.retire_fs_page_scroll_animation(ctx);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        assert_eq!(animation.start_time, Some(200.0));
        assert_eq!(animation.last_painted_pan, Vec2::ZERO);
        assert_eq!(animation.last_painted_time, Some(200.0));
        assert_eq!(app.fs_page_scroll_display_pan(ctx), target);
    });
    timed_paint(&ctx, 1000.05, &mut app);
    assert!(app.fs_page_scroll_animation.is_none());
    assert_eq!(app.fs_pan, target);
}

#[test]
fn retirement_without_a_paint_receipt_does_not_invent_a_retarget_time() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(&ctx, &mut app);
    timed_frame(&ctx, 1.0, |ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
    });
    timed_frame(&ctx, 2.0, |ctx| {
        app.retire_fs_page_scroll_animation(ctx);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        assert_eq!(animation.start_time, Some(2.0));
        assert_eq!(animation.last_painted_time, None);
    });
    timed_frame(&ctx, 100.0, |ctx| {
        let previous_target = app.fs_pan;
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert_ne!(app.fs_pan, previous_target);
        let animation = app.fs_page_scroll_animation.as_ref().unwrap();
        assert_eq!(animation.start_time, None);
        assert_eq!(animation.last_painted_time, None);
        assert_eq!(animation.from, Vec2::ZERO);
        assert_eq!(animation.last_painted_pan, Vec2::ZERO);
    });
    timed_paint(&ctx, 101.0, &mut app);
    let animation = app.fs_page_scroll_animation.as_ref().unwrap();
    assert_eq!(animation.start_time, Some(101.0));
    assert_eq!(animation.last_painted_time, Some(101.0));
    assert_eq!(animation.last_painted_pan, Vec2::ZERO);
}
