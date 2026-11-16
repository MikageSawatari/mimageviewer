//! Native pointer conversion must end viewer holds without authorizing actions.
use super::*;
use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};

#[derive(Clone, Copy, Debug)]
enum ExpectedSecondaryAction {
    None,
    ShortTapAt(egui::Pos2),
}

fn pointer_state(ctx: &egui::Context) -> egui_winit::State {
    struct NoDisplay;
    impl winit::raw_window_handle::HasDisplayHandle for NoDisplay {
        fn display_handle(
            &self,
        ) -> Result<
            winit::raw_window_handle::DisplayHandle<'_>,
            winit::raw_window_handle::HandleError,
        > {
            Err(winit::raw_window_handle::HandleError::Unavailable)
        }
    }
    egui_winit::State::new(
        ctx.clone(),
        egui::ViewportId::ROOT,
        &NoDisplay,
        Some(1.0),
        None,
        None,
    )
}

fn native_frame(
    app: &mut App,
    ctx: &egui::Context,
    state: &mut egui_winit::State,
    events: impl IntoIterator<Item = WindowEvent>,
    expected: ExpectedSecondaryAction,
) {
    for event in events {
        let _ = state
            .on_pointer_event(&event, 1.0)
            .expect("native pointer event");
    }
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
    let mut raw = state.egui_input_mut().take();
    raw.screen_rect = Some(rect);
    ctx.run_pass(raw, |ctx| {
        let (front, _) = app.capture_fs_front_pointer_input(ctx, rect);
        let action = app.recognize_viewer_right_drag_frame(ctx, rect, false, false, 0, front);
        match (expected, &action) {
            (ExpectedSecondaryAction::None, FsSecondaryAction::None) => {}
            (
                ExpectedSecondaryAction::ShortTapAt(expected),
                FsSecondaryAction::ShortTap { pos, .. },
            ) => {
                assert_eq!(*pos, expected);
            }
            (
                ExpectedSecondaryAction::ShortTapAt(expected),
                FsSecondaryAction::Recognized {
                    pos, recognition, ..
                },
            ) => {
                assert_eq!(*pos, expected);
                assert_eq!(
                    recognition.outcome,
                    crate::ring_shortcut::MouseFlickOutcome::ShortTap
                );
                assert!(recognition.command.is_none());
            }
            _ => panic!("viewer action must match {expected:?}"),
        }
        assert_eq!(
            matches!(
                app.apply_fs_secondary_input(ctx, action),
                FsSecondaryInputOutcome::CloseRequested
            ),
            matches!(expected, ExpectedSecondaryAction::ShortTapAt(_)),
        );
    })
    .drop_without_applying_deltas();
}

fn moved(x: f64) -> WindowEvent {
    WindowEvent::CursorMoved {
        device_id: DeviceId::dummy(),
        position: winit::dpi::PhysicalPosition::new(x, 200.0),
    }
}

fn right(pressed: bool) -> WindowEvent {
    WindowEvent::MouseInput {
        device_id: DeviceId::dummy(),
        state: if pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        },
        button: MouseButton::Right,
    }
}

#[test]
fn native_unknown_secondary_release_cancels_all_viewer_right_drag_modes() {
    use crate::ring_shortcut::{RightDragContext, RightDragMode, ViewerShortRightClickAction};
    use ExpectedSecondaryAction::{None, ShortTapAt};
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in [
        RightDragMode::Disabled,
        RightDragMode::RingShortcut,
        RightDragMode::MouseGesture,
    ] {
        let ctx = egui::Context::default();
        let mut state = pointer_state(&ctx);
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![GridItem::Image(std::path::PathBuf::from("C:/release.png"))];
        app.fullscreen_idx = Some(0);
        app.settings
            .ring_shortcuts
            .set_right_drag_mode(RightDragContext::ImageFullscreen, mode.clone());
        app.settings
            .ring_shortcuts
            .set_viewer_short_right_click_action(
                RightDragContext::ImageFullscreen,
                ViewerShortRightClickAction::CloseFullscreen,
            );
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [moved(599.0), right(true)],
            None,
        );
        // Only two pixels of motion: this is still a short-tap candidate, but
        // the release after CursorLeft carries no coordinate proof.
        native_frame(&mut app, &ctx, &mut state, [moved(601.0)], None);
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [WindowEvent::CursorLeft {
                device_id: DeviceId::dummy(),
            }],
            None,
        );
        native_frame(&mut app, &ctx, &mut state, [right(false)], None);
        assert_eq!(
            app.fs_secondary_press,
            FullscreenSecondaryPress::Idle,
            "{mode:?}"
        );
        assert!(
            app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none(),
            "{mode:?}: release must retire the hold"
        );
        assert!(app.fs_context_menu_idx.is_none());
        native_frame(&mut app, &ctx, &mut state, [], None);
        // A fresh positioned click retains the configured close action.
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [moved(500.0), right(true)],
            None,
        );
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [right(false), moved(200.0)],
            ShortTapAt(egui::pos2(500.0, 200.0)),
        );
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [moved(599.0), right(true)],
            None,
        );
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [
                moved(601.0),
                WindowEvent::CursorLeft {
                    device_id: DeviceId::dummy(),
                },
            ],
            None,
        );
        // Cancel the old stream and admit a genuinely new press in order,
        // even when native delivery batches both into one egui frame.
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [right(false), moved(500.0), right(true)],
            None,
        );
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [right(false)],
            ShortTapAt(egui::pos2(500.0, 200.0)),
        );
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [moved(500.0), right(true)],
            None,
        );
        // Hold A has a qualified release before B starts and ends without a
        // coordinate. B's cancellation cannot erase A's completed short tap.
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [
                right(false),
                right(true),
                moved(601.0),
                WindowEvent::CursorLeft {
                    device_id: DeviceId::dummy(),
                },
                right(false),
            ],
            ShortTapAt(egui::pos2(500.0, 200.0)),
        );
        assert_eq!(
            app.fs_secondary_press,
            FullscreenSecondaryPress::Idle,
            "{mode:?}"
        );
        assert!(
            app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none(),
            "{mode:?}: B terminates without replacing A's action"
        );
        native_frame(&mut app, &ctx, &mut state, [], None);
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [moved(500.0), right(true)],
            None,
        );
        // The final level/motion sample can cancel B's passive press too;
        // it still cannot discard A's already completed operation.
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [right(false), right(true), moved(200.0)],
            ShortTapAt(egui::pos2(500.0, 200.0)),
        );
        native_frame(
            &mut app,
            &ctx,
            &mut state,
            [
                WindowEvent::CursorLeft {
                    device_id: DeviceId::dummy(),
                },
                right(false),
            ],
            None,
        );
        assert_eq!(
            app.fs_secondary_press,
            FullscreenSecondaryPress::Idle,
            "{mode:?}"
        );
        assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
    }
}
