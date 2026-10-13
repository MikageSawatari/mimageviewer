//! Timer ownership through the production slideshow navigation and page submission.
#![cfg(test)]
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, ready_app};

fn frame(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    events: Vec<egui::Event>,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let mut raw = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        events,
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            parent: (viewport != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
            focused: Some(true),
            ..Default::default()
        },
    );
    let _ = ctx.run(raw, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| run(ui, ctx));
    });
}

fn draw_and_sync(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> SubmittedUnitReceipt {
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
    let (composition, sources) = draw_current(app, ui, ctx);
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        &sources,
    );
    app.sync_slideshow_anchor_for_frame(ctx, idx, &state);
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("paged receipt")
    };
    receipt
}

#[test]
fn slideshow_regression_expired_outgoing_deadline_turns_and_preserves_full_display_interval() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        for manual in [false, true] {
            take_test_events();
            let ctx = egui::Context::default();
            install(&ctx);
            install_fs_navigator_input_tracking(&ctx);
            let mut app = ready_app(&ctx, mode);
            app.settings.page_turn_animation_enabled = true;
            app.settings.page_turn_animation_duration_ms = 600;
            app.settings.slideshow_interval_secs = 3.0;
            app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
            let viewport = app.page_turn_destination().viewport;
            frame(&ctx, viewport, vec![], |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                draw_and_sync(&mut app, ui, ctx);
            });
            app.slideshow_playing = true;
            app.slideshow_anchor_idx = Some(0);
            app.slideshow_next_at = Instant::now();
            let expired = app.slideshow_next_at;
            let events = if manual {
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -120.0),
                    modifiers: egui::Modifiers::NONE,
                }]
            } else {
                vec![]
            };
            frame(&ctx, viewport, events, |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                if manual {
                    let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                    let mut operation = None;
                    let rect = ui.max_rect();
                    let (nav, close) = app.handle_fs_wheel_and_click_with_geometry_and_operation(
                        ui,
                        ctx,
                        rect,
                        &state,
                        StillSeekGeometry::bar_only(),
                        mode.is_spread(),
                        0,
                        &mut None,
                        None,
                        &mut operation,
                    );
                    assert!(matches!(operation, Some(ViewerOperationId::Pointer { .. })));
                    app.handle_fs_navigation_with_operation(
                        ctx, close, false, None, None, None, nav, None, 0, operation,
                    );
                } else {
                    app.advance_slideshow(ctx, 0);
                }
                assert!(draw_and_sync(&mut app, ui, ctx).curl);
                assert_eq!(app.slideshow_anchor_idx, None);
                assert_eq!(
                    app.slideshow_next_at, expired,
                    "outgoing deadline cannot reject or time the new page"
                );
            });
            let target = app.fullscreen_idx.unwrap();
            assert_eq!(target, if mode.is_spread() { 2 } else { 1 });
            let (operation, tracker) = match &app.page_turn_presentation.state {
                PresentationState::Turning { offer, .. } => {
                    (offer.fact.operation_id, offer.submission.clone())
                }
                _ => panic!("real navigation starts a turn"),
            };
            assert_eq!(tracker.status(), SubmissionStatus::Started);
            frame(&ctx, viewport, vec![], |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                assert!(draw_and_sync(&mut app, ui, ctx).curl);
                assert_eq!(app.slideshow_anchor_idx, None);
            });
            if let PresentationState::Turning { offer, .. } = &mut app.page_turn_presentation.state
            {
                offer.accepted_at -= offer.duration + Duration::from_millis(1);
            } else {
                panic!("turn persists")
            }
            frame(&ctx, viewport, vec![], |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                assert!(!draw_and_sync(&mut app, ui, ctx).curl);
                assert_eq!(
                    app.slideshow_anchor_idx, None,
                    "wait for flat adoption, not merely texture readiness"
                );
            });
            assert_eq!(tracker.status(), SubmissionStatus::Finished);
            let before = Instant::now();
            frame(&ctx, viewport, vec![], |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                assert!(!draw_and_sync(&mut app, ui, ctx).curl);
                assert_eq!(app.slideshow_anchor_idx, Some(target));
            });
            let after = Instant::now();
            assert!(app.slideshow_next_at >= before + Duration::from_secs(3));
            assert!(app.slideshow_next_at <= after + Duration::from_secs(3));
            let events = take_test_events();
            assert!(
                events
                    .iter()
                    .any(|(fact, kind, _)| fact.operation_id == operation && *kind == "finish")
            );
            assert!(
                !events
                    .iter()
                    .any(|(_, kind, _)| matches!(*kind, "skip" | "cancel")),
                "{events:?}"
            );
        }
    }
}

#[test]
fn slideshow_regression_short_interval_and_disabled_animation_keep_flat_cadence() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for enabled in [false, true] {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, SpreadMode::Single);
        app.settings.page_turn_animation_enabled = enabled;
        app.settings.page_turn_animation_duration_ms = 600;
        app.settings.slideshow_interval_secs = 0.5;
        let viewport = app.page_turn_destination().viewport;
        frame(&ctx, viewport, vec![], |ui, ctx| {
            draw_and_sync(&mut app, ui, ctx);
        });
        take_test_events();
        app.slideshow_playing = true;
        app.slideshow_anchor_idx = Some(0);
        app.slideshow_next_at = Instant::now();
        let before = Instant::now();
        frame(&ctx, viewport, vec![], |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            app.advance_slideshow(ctx, 0);
            assert!(!draw_and_sync(&mut app, ui, ctx).curl);
            assert_eq!(app.slideshow_anchor_idx, Some(1));
        });
        let after = Instant::now();
        assert!(app.slideshow_next_at >= before + Duration::from_millis(500));
        assert!(app.slideshow_next_at <= after + Duration::from_millis(500));
        let events = take_test_events();
        assert_eq!(
            events
                .iter()
                .filter(|(_, kind, reason)| *kind == "skip"
                    && *reason == "slideshow_interval_shorter_than_duration")
                .count(),
            usize::from(enabled)
        );
    }
}

#[test]
fn slideshow_regression_continuous_wait_remains_its_existing_interval() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.reading_flow = ReadingFlow::Vertical;
    app.settings.slideshow_interval_secs = 3.0;
    app.settings.slideshow_continuous_wait_secs = 1.5;
    let before = Instant::now();
    app.schedule_next_slideshow_from_now();
    let after = Instant::now();
    assert_eq!(app.slideshow_anchor_idx, app.fullscreen_idx);
    assert!(app.slideshow_next_at >= before + Duration::from_millis(1500));
    assert!(app.slideshow_next_at <= after + Duration::from_millis(1500));
}
