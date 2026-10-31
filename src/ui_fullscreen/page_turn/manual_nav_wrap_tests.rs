use super::*;
use crate::ui_fullscreen::{
    FsKeyAction, FsPageNav, FsWrapTarget,
    page_turn_submission_tests::{ready_app, seed_leaf},
};

#[test]
fn manual_nav_wrap_barrier_preserves_resolved_target_slice_and_shift_fallback() {
    for completing in [false, true] {
        for source in ["delta", "wrap", "wrap_slice", "shift"] {
            let ctx = egui::Context::default();
            let mut env = ready_app(&ctx, SpreadMode::Single);
            let app = &mut *env;
            // seed_leaf turns from zero to one. In this reading order one is
            // the final page, so the next manual source-unit move wraps to two.
            app.visible_indices = vec![2, 3, 0, 1];
            app.viewer_navigation_caches.invalidate();
            app.settings.fullscreen_manual_nav_wrap = true;
            seed_leaf(&ctx, app);
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Turning { .. }
            ));
            if completing {
                app.page_turn_presentation.cancel("manual_wrap_test_seed");
                assert!(app.page_turn_completion_active());
            }
            let operation = ViewerOperationId::Pad {
                batch: next_operation_batch(),
                ordinal: 0,
            };
            let slice = (source == "wrap_slice").then_some(crate::page_split::PageSlice::Left);
            if source == "shift" {
                let mut action = FsKeyAction::default();
                app.queue_spread_shift_action_for_frame(&ctx, 1, 1, &mut action, Some(operation));
                assert!(action.page_nav.is_none());
            } else {
                let nav = if source == "delta" {
                    FsPageNav::Delta(1)
                } else {
                    FsPageNav::Wrap(FsWrapTarget {
                        idx: 2,
                        slice,
                        forward: true,
                    })
                };
                app.handle_fs_navigation_with_operation(
                    &ctx,
                    false,
                    false,
                    None,
                    None,
                    None,
                    nav,
                    None,
                    1,
                    Some(operation),
                );
            }
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "the barrier still owns the old flat"
            );
            let PresentationState::Completing {
                continuation: Some(command),
                completion: FlatCompletion::NeedFlat,
                ..
            } = &app.page_turn_presentation.state
            else {
                panic!("{source}: a resolved continuation must wait for canonical flat")
            };
            assert_eq!(command.from_idx, 1);
            assert_eq!(command.target_idx, 2, "{source}");
            assert_eq!(
                command.contract,
                crate::fs_page_load_scheduler::FsPageLoadContract::Sequential
            );
            assert!(
                matches!(command.intent, ResolvedNavigationIntent::Page { slice: actual } if actual == slice)
            );
            assert!(app.fs_boundary_hint.is_none());
        }
    }
}
