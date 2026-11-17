use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{frame, ready_app};

#[test]
fn spread_step_ring_ids_roundtrip_and_assignable_only_to_images() {
    for (action, id) in [
        (RingActionId::ImageSpreadStepPrev, "image_spread_step_prev"),
        (RingActionId::ImageSpreadStepNext, "image_spread_step_next"),
    ] {
        assert_eq!(action.as_str(), id);
        assert_eq!(RingActionId::from_str(id), Some(action.clone()));
        for context in [
            RingShortcutContext::Grid,
            RingShortcutContext::ImageFullscreen,
            RingShortcutContext::VideoFullscreen,
        ] {
            let valid = context == RingShortcutContext::ImageFullscreen;
            assert_eq!(action.is_valid_for_context(context), valid);
            assert_eq!(
                RingActionId::available_for_context(context).contains(&action),
                valid
            );
            assert_eq!(
                RingActionId::available_for_mouse_button_context(context).contains(&action),
                valid
            );
        }
    }
}

#[test]
fn spread_step_ring_and_assigned_mouse_use_the_same_execution() {
    for mouse in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, SpreadMode::Ltr);
        app.settings.page_turn_animation_enabled = false;
        app.fullscreen_idx = Some(0);
        frame(&ctx, |_, ctx| {
            if mouse {
                app.settings
                    .ring_shortcuts
                    .mouse_button_profile_mut(RingShortcutContext::ImageFullscreen)
                    .back = RingActionId::ImageSpreadStepNext;
                let result = app.dispatch_mouse_button(
                    ctx,
                    MouseButtonSlot::Back,
                    crate::app::ActionSurface::Viewer,
                    "step-mouse-test",
                );
                assert_eq!(
                    result.dispatched_action,
                    Some(RingActionId::ImageSpreadStepNext)
                );
            } else {
                app.apply_ring_action(
                    ctx,
                    RingShortcutContext::ImageFullscreen,
                    RingActionId::ImageSpreadStepNext,
                    "step-ring-test",
                );
            }
        });
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.spread_step.unwrap().start_idx, 1);
        assert_eq!(
            app.spread_display_composition_for_anchor(1)
                .navigation_pages(),
            vec![1, 2]
        );
    }
}
