//! Edit entry cannot overtake a resolved navigation held by flat completion.
#![cfg(test)]

use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

fn draw(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> SubmittedUnitReceipt {
    let (composition, sources) = draw_current(app, ui, ctx);
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        &sources,
    );
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("paged output")
    };
    receipt
}

#[test]
fn page_edit_admission_completing_navigation_blocks_tools_until_flat_lands() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    for anchor in [2, 0] {
        app.fullscreen_idx = Some(anchor);
        frame(&ctx, |ui, ctx| {
            draw(&mut app, ui, ctx);
        });
    }
    frame(&ctx, |_, ctx| {
        let op = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(ctx, op, OperationPhase::Press, "edit-admission");
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(2),
            None,
            0,
            Some(op),
        );
    });
    frame(&ctx, |ui, ctx| {
        assert!(draw(&mut app, ui, ctx).curl);
        assert!(!app.fullscreen_edit_mode_entry_allowed(2));
    });
    clock.set(origin + Duration::from_millis(100));
    frame(&ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        app.fs_zoom = 3.0;
        app.set_fs_pan_from_input(3.0, egui::vec2(-300.0, 0.0));
        assert!(!draw(&mut app, ui, ctx).curl);
        assert!(app.page_turn_completion_active());
        let op = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(ctx, op, OperationPhase::Press, "edit-admission-next");
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(0),
            None,
            0,
            Some(op),
        );
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Completing {
                continuation: Some(_),
                ..
            }
        ));
        app.enter_export_crop_mode(2);
        assert!(!app.export_crop_mode);
        assert_eq!(app.fullscreen_idx, Some(2));
    });
    for _ in 0..6 {
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            draw(&mut app, ui, ctx);
        });
        if app.fullscreen_idx == Some(0) && !app.page_turn_barrier_active() {
            break;
        }
    }
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(!app.page_turn_barrier_active());
    app.enter_export_crop_mode(0);
    assert!(app.export_crop_mode);
}
