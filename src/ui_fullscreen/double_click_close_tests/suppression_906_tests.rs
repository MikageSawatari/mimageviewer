//! §1.906: lifetime at the production suppression boundary (no native windows).
use super::*;
use FullscreenPrimarySuppression::{Idle, PointerStream, ReleasedThisPass, TouchStream};
use FullscreenPrimarySuppressionDecision::{Allow, Preserve, Suppress};
use FullscreenPrimarySuppressionReason::{FocusRestore, Other};

#[cfg(windows)]
fn page_canvas() -> Canvas {
    let mut canvas = Canvas::for_focus_restore();
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
    canvas
}

fn f2(canvas: &mut Canvas, enabled: bool) {
    canvas.app.settings.vst3_enabled = true;
    canvas.app.settings.vst3_gui_visible = enabled;
    // F1 uses wall-clock time, independently of Canvas.time.
    canvas.app.fs_focus_regained_at = None;
}

#[cfg(windows)]
#[test]
fn suppression_906_f2_same_and_split_frame_next_click_turns_page() {
    let _input = crate::key_input::lock_test_input();
    for split in [false, true] {
        let mut canvas = page_canvas();
        f2(&mut canvas, true);
        let first = if split {
            canvas.split_click(egui::Modifiers::NONE)
        } else {
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE))
        };
        assert_eq!(first, (false, false));
        assert_eq!(
            canvas.app.fs_primary_suppression,
            if split {
                Idle
            } else {
                ReleasedThisPass(FocusRestore)
            }
        );
        f2(&mut canvas, false);
        canvas.redraw_canvas_after_release();
        // Old code fails for split=false: it suppresses this new click too.
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            (false, true)
        );
        assert_eq!(canvas.app.fs_primary_suppression, Idle);
    }
}

#[cfg(windows)]
#[test]
fn suppression_906_drag_after_same_frame_restore_moves_zoomed_image() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = page_canvas();
    canvas.app.fs_zoom = 2.0;
    f2(&mut canvas, true);
    assert_eq!(
        canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
        (false, false)
    );
    f2(&mut canvas, false);
    canvas.redraw_canvas_after_release();
    let before = canvas.app.fs_pan;
    canvas.merge(vec![button(canvas_pos(), true, egui::Modifiers::NONE)]);
    canvas.merge(vec![egui::Event::PointerMoved(
        canvas_pos() + egui::vec2(50.0, 30.0),
    )]);
    // Old code retains PointerStream and never admits this drag.
    assert_ne!(canvas.app.fs_pan, before);
    assert_eq!(canvas.app.fs_primary_suppression, Idle);
    canvas.merge(vec![button(
        canvas_pos() + egui::vec2(50.0, 30.0),
        false,
        egui::Modifiers::NONE,
    )]);
    assert!(canvas.app.fs_pan_drag_start.is_none());
}

fn brush_transform() -> DisplayedImageTransform {
    use crate::displayed_image_transform::{
        DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit,
        ResolvedDisplayPlacement,
    };
    DisplayedImageTransform::from_resolved_rect(
        DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: viewport(),
            source_size: egui::vec2(200.0, 200.0),
            texture_size: egui::vec2(200.0, 200.0),
            rotation: crate::rotation_db::Rotation::None,
            free_rotation_rad: 0.0,
            content_bbox: None,
            fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                crate::settings::FullscreenFitMode::Page,
            ),
            fit_scale_limits: FullscreenFitScaleLimits::default(),
            pixels_per_point: 1.0,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            pixel_fit: RectPixelFit::Texels,
        },
        egui::Rect::from_min_size(egui::pos2(380.0, 100.0), egui::vec2(200.0, 200.0)),
    )
    .unwrap()
}

#[cfg(windows)]
#[test]
fn suppression_906_restore_does_not_paint_or_push_undo_but_next_click_does() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = page_canvas();
    canvas.app.erase_mode = true;
    canvas.app.erase_tool = crate::app::EraseTool::Brush;
    canvas.app.erase_paint_mode = true;
    canvas.app.erase_brush_radius = 1.0;
    canvas.app.erase_mask_size = [200, 200];
    canvas.app.erase_mask = Some(vec![false; 200 * 200]);
    canvas.app.erase_base_cache.insert(
        0,
        crate::app::EraseBaseSource::raster(Arc::new(egui::ColorImage::filled(
            [200, 200],
            egui::Color32::WHITE,
        ))),
    );
    let pos = egui::pos2(480.0, 200.0);
    let run = |app: &mut App, ui: &mut egui::Ui| {
        let (result, _) = super::super::wheel_frame_tests::actual_merge(app, ui, false);
        // Crucially, the actual downstream editor runs in this SAME pass.
        app.keyboard_owner_for_pass(ui.ctx());
        app.handle_erase_paint(ui.ctx(), &brush_transform(), Default::default());
        (result.close, !result.page_nav.is_none())
    };
    canvas.merge_using(vec![egui::Event::PointerMoved(pos)], true, None, run);
    assert!(!canvas.app.erase_panel_rect(viewport()).contains(pos));
    let undo_before = canvas.app.erase_undo_stack.len();
    f2(&mut canvas, true);
    assert_eq!(
        canvas.merge_using(click(pos, egui::Modifiers::NONE), true, None, run),
        (false, false)
    );
    assert_eq!(
        canvas.app.fs_primary_suppression,
        ReleasedThisPass(FocusRestore)
    );
    assert!(
        canvas
            .app
            .erase_mask
            .as_ref()
            .unwrap()
            .iter()
            .all(|pixel| !pixel)
    );
    assert_eq!(canvas.app.erase_undo_stack.len(), undo_before);
    f2(&mut canvas, false);
    canvas.merge_using(click(pos, egui::Modifiers::NONE), true, None, run);
    // Old code fails here: the second stroke is suppressed until its release.
    assert!(
        canvas
            .app
            .erase_mask
            .as_ref()
            .unwrap()
            .iter()
            .any(|pixel| *pixel)
    );
    assert_eq!(canvas.app.erase_undo_stack.len(), undo_before + 1);
}

fn secondary(pressed: bool) -> egui::Event {
    let mut event = button(canvas_pos(), pressed, egui::Modifiers::NONE);
    if let egui::Event::PointerButton { button, .. } = &mut event {
        *button = egui::PointerButton::Secondary;
    }
    event
}

fn enable_right_drag(canvas: &mut Canvas) {
    canvas.app.settings.ring_shortcuts.set_right_drag_mode(
        crate::ring_shortcut::RightDragContext::ImageFullscreen,
        crate::ring_shortcut::RightDragMode::RingShortcut,
    );
}

#[cfg(windows)]
#[test]
fn suppression_906_release_then_press_rearms_f1_and_right_drag_until_release() {
    let _input = crate::key_input::lock_test_input();
    for right in [false, true] {
        let mut canvas = page_canvas();
        canvas.merge(vec![button(canvas_pos(), true, egui::Modifiers::NONE)]);
        // Model an existing suppressed flow, ending before the new press.
        canvas.app.fs_primary_suppression =
            FullscreenPrimarySuppression::pointer_stream_for_test(FocusRestore);
        if right {
            enable_right_drag(&mut canvas);
            canvas.merge(vec![secondary(true)]);
            assert!(canvas.app.mouse_ring_flick.is_some());
        } else {
            canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        }
        assert_eq!(
            canvas.merge(vec![
                button(canvas_pos(), false, egui::Modifiers::NONE),
                button(canvas_pos(), true, egui::Modifiers::NONE),
            ]),
            (false, false)
        );
        let reason = if right { Other } else { FocusRestore };
        // Old advance arms before processing the old release (or misses F1).
        assert_eq!(canvas.app.fs_primary_suppression, PointerStream(reason));
        canvas.merge(vec![egui::Event::PointerMoved(
            canvas_pos() + egui::vec2(40.0, 20.0),
        )]);
        assert_eq!(canvas.app.fs_primary_suppression, PointerStream(reason));
        assert_eq!(canvas.app.fs_pan, egui::Vec2::ZERO);
        assert_eq!(
            canvas.merge(vec![button(canvas_pos(), false, egui::Modifiers::NONE)]),
            (false, false)
        );
        assert_eq!(canvas.app.fs_primary_suppression, Idle);
    }
}

#[cfg(windows)]
#[test]
fn suppression_906_right_drag_same_and_split_click_stops_only_that_flow() {
    let _input = crate::key_input::lock_test_input();
    for split in [false, true] {
        let mut canvas = page_canvas();
        enable_right_drag(&mut canvas);
        canvas.merge(vec![secondary(true)]);
        assert!(canvas.app.mouse_ring_flick.is_some());
        let result = if split {
            canvas.split_click(egui::Modifiers::NONE)
        } else {
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE))
        };
        assert_eq!(result, (false, false));
        assert!(canvas.app.mouse_ring_flick.is_none());
        assert_eq!(
            canvas.app.fs_primary_suppression,
            if split { Idle } else { ReleasedThisPass(Other) }
        );
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            (false, true)
        );
    }
}

#[cfg(windows)]
fn scrub_key(down: bool) {
    use crate::key_input::{SyntheticKeyCommand, SyntheticNavigationKey};
    let now = std::time::Instant::now();
    let command = if down {
        SyntheticKeyCommand::down(
            now,
            SyntheticNavigationKey::Tab,
            crate::key_input::SyntheticModifiers {
                ctrl: true,
                ..Default::default()
            },
        )
    } else {
        SyntheticKeyCommand::up(now, SyntheticNavigationKey::Tab)
    };
    crate::key_input::enqueue_test_synthetic_command(command);
    crate::key_input::advance_test_synthetic_input(now);
}

#[cfg(windows)]
#[test]
fn suppression_906_viewer_scrub_same_frame_and_held_until_release() {
    let _input = crate::key_input::lock_test_input();
    for split in [false, true] {
        let mut canvas = page_canvas();
        // Use the same explicit Ctrl+Tab binding and physical-key timeline as
        // scrub_zoom_primary_handler_owns_click_and_drag_but_excludes_media.
        // Arm after fixture initialization so this test owns the input setup.
        canvas.app.keymap =
            crate::keymap::Keymap::from_ini_str("[FsImage]\nFsScrubZoomHold = Ctrl+Tab\n");
        crate::key_input::arm_test_synthetic_input(0xE906, egui::ViewportId::ROOT);
        scrub_key(true);
        let expected_held = std::cell::Cell::new(true);
        let run = |app: &mut App, ui: &mut egui::Ui| {
            let owner = app.keyboard_owner_for_pass(ui.ctx());
            for virtual_key in [0x09, 0x11] {
                assert_eq!(
                    crate::key_input::physical_key_down(crate::key_input::PhysicalKeySlot::new(
                        virtual_key,
                        false
                    ),),
                    expected_held.get(),
                    "the synthetic Tab/Ctrl physical levels must match this phase"
                );
            }
            let permit = crate::keyboard_input::focused_key_state_permit(ui.ctx())
                .expect("scrub fixture viewport must be focused");
            assert!(
                !crate::keyboard_input::keymap_owner_blocks_shortcuts(ui.ctx()),
                "scrub fixture keyboard owner blocks the chord: {owner:?}"
            );
            assert_eq!(
                app.keymap
                    .key_held_action(ui.ctx(), permit, KeyAction::FsScrubZoomHold),
                expected_held.get(),
                "the production keymap must observe the synthetic chord before the handler"
            );
            let (result, _) = super::super::wheel_frame_tests::actual_merge(app, ui, false);
            (result.close, !result.page_nav.is_none())
        };
        let events = if split {
            vec![button(canvas_pos(), true, egui::Modifiers::CTRL)]
        } else {
            click(canvas_pos(), egui::Modifiers::CTRL)
        };
        let first = canvas.merge_using(events, true, None, run);
        if split {
            assert_eq!(first, (false, false));
            assert_eq!(canvas.app.fs_primary_suppression, PointerStream(Other));
            assert!(canvas.app.fs_middle_zoom_drag.is_some());
        } else {
            // A complete click clears egui's press_origin in this pass, so
            // scrub never starts and no suppression is armed. The existing
            // page-turn result of that fast chord-click is a separate issue;
            // deliberately do not assert `first` for this control case.
            assert_eq!(canvas.app.fs_primary_suppression, Idle);
            assert!(canvas.app.fs_middle_zoom_drag.is_none());
        }
        scrub_key(false);
        expected_held.set(false);
        if split {
            canvas.merge_using(
                vec![egui::Event::PointerMoved(canvas_pos())],
                true,
                None,
                run,
            );
            assert_eq!(canvas.app.fs_primary_suppression, PointerStream(Other));
            assert_eq!(
                canvas.merge_using(
                    vec![button(canvas_pos(), false, egui::Modifiers::NONE)],
                    true,
                    None,
                    run
                ),
                (false, false)
            );
        }
        assert!(canvas.app.fs_middle_zoom_drag.is_none());
        // A held scrub returns before fs_click, so redraw its hit target before
        // the next click. Both delivery forms must leave suppression idle.
        assert_eq!(canvas.merge_using(vec![], true, None, run), (false, false));
        assert_eq!(canvas.app.fs_primary_suppression, Idle);
        // The next ordinary click must work in both control cases.
        assert_eq!(
            canvas.merge_using(click(canvas_pos(), egui::Modifiers::NONE), true, None, run),
            (false, true)
        );
        assert_eq!(canvas.app.fs_primary_suppression, Idle);
        crate::key_input::clear_test_synthetic_input();
    }
}

#[cfg(windows)]
#[test]
fn suppression_906_overlay_middle_owner_rejects_same_frame_scrub_without_latching() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = page_canvas();
    canvas.app.erase_mode = true;
    let mut middle = button(canvas_pos(), true, egui::Modifiers::NONE);
    if let egui::Event::PointerButton { button, .. } = &mut middle {
        *button = egui::PointerButton::Middle;
    }
    canvas.merge_using(vec![middle], true, None, |app, ui| {
        let _ = app.advance_viewer_primary_pointer_frame(ui.ctx(), false);
        assert!(app.handle_middle_drag_zoom(ui.ctx(), viewport(), Default::default()));
        (false, false)
    });
    assert_eq!(
        canvas.app.fs_middle_zoom_drag.as_ref().unwrap().input,
        ZoomDragInput::Middle
    );
    canvas.merge_using(
        click(canvas_pos(), egui::Modifiers::CTRL),
        true,
        None,
        |app, ui| {
            let _ = app.advance_viewer_primary_pointer_frame(ui.ctx(), false);
            assert!(app.handle_overlay_space_pan_drag(
                ui.ctx(),
                false,
                true,
                viewport().center(),
                true,
                true,
                false,
                true,
                Some(canvas_pos())
            ));
            assert_eq!(app.fs_primary_suppression, ReleasedThisPass(Other));
            assert_eq!(
                app.fs_middle_zoom_drag.as_ref().unwrap().input,
                ZoomDragInput::Middle
            );
            (false, false)
        },
    );
    canvas.merge_using(
        click(canvas_pos(), egui::Modifiers::NONE),
        true,
        None,
        |app, ui| {
            assert_eq!(
                app.advance_viewer_primary_pointer_frame(ui.ctx(), false),
                Allow
            );
            assert_eq!(app.fs_primary_suppression, Idle);
            (false, false)
        },
    );
}

#[cfg(windows)]
#[test]
fn suppression_906_overlay_ctrl_added_to_space_pan_uses_final_release_level() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = page_canvas();
    canvas.app.erase_mode = true;
    let run = |app: &mut App, ui: &mut egui::Ui| {
        use crate::app::canvas_gesture::{CanvasTool, canvas_pointer_inputs};
        let _ = app.advance_viewer_primary_pointer_frame(ui.ctx(), false);
        let scope = app.canvas_gesture_scope(0, CanvasTool::Erase(crate::app::EraseTool::Brush));
        for input in canvas_pointer_inputs(ui.ctx(), scope, Default::default(), true, |_| true) {
            let ctrl = input.modifiers().ctrl;
            // Real transport emits a down=true Move before its Release sample.
            let used = app.handle_overlay_space_pan_drag(
                ui.ctx(),
                !ctrl,
                ctrl,
                viewport().center(),
                true,
                input.pressed(),
                input.down(),
                input.released(),
                input.position(),
            );
            if input.down() {
                assert!(used);
            }
        }
        (false, false)
    };
    canvas.merge_using(
        vec![button(canvas_pos(), true, egui::Modifiers::NONE)],
        true,
        None,
        run,
    );
    assert!(canvas.app.fs_pan_drag_start.is_some());
    canvas.merge_using(
        vec![button(canvas_pos(), false, egui::Modifiers::CTRL)],
        true,
        None,
        run,
    );
    assert!(canvas.app.fs_pan_drag_start.is_none());
    assert_eq!(canvas.app.fs_primary_suppression, ReleasedThisPass(Other));
    canvas.merge_using(
        click(canvas_pos(), egui::Modifiers::NONE),
        true,
        None,
        |app, ui| {
            assert_eq!(
                app.advance_viewer_primary_pointer_frame(ui.ctx(), false),
                Allow
            );
            assert_eq!(app.fs_primary_suppression, Idle);
            (false, false)
        },
    );
}

#[cfg(windows)]
#[test]
fn suppression_906_overlay_owner_switch_uses_final_level_not_move_sample() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = page_canvas();
    canvas.app.erase_mode = true;
    canvas.merge_using(
        vec![button(canvas_pos(), true, egui::Modifiers::CTRL)],
        true,
        None,
        |app, ui| {
            let _ = app.advance_viewer_primary_pointer_frame(ui.ctx(), false);
            assert!(app.handle_overlay_space_pan_drag(
                ui.ctx(),
                false,
                true,
                viewport().center(),
                true,
                true,
                true,
                false,
                Some(canvas_pos())
            ));
            (false, false)
        },
    );
    assert!(canvas.app.fs_middle_zoom_drag.is_some());
    canvas.app.analysis_mode = true;
    canvas.merge_using(
        vec![button(canvas_pos(), false, egui::Modifiers::CTRL)],
        true,
        None,
        |app, ui| {
            let _ = app.advance_viewer_primary_pointer_frame(ui.ctx(), false);
            assert!(!ui.ctx().input(|i| i.pointer.primary_down()));
            // The real canvas transport sends this final Move with down=true before
            // Release. The existing admission condition remains true for that sample.
            assert!(app.handle_overlay_space_pan_drag(
                ui.ctx(),
                false,
                true,
                viewport().center(),
                true,
                false,
                true,
                false,
                Some(canvas_pos())
            ));
            assert!(app.fs_middle_zoom_drag.is_none());
            assert_eq!(app.fs_primary_suppression, ReleasedThisPass(Other));
            (false, false)
        },
    );
    canvas.merge_using(
        click(canvas_pos(), egui::Modifiers::NONE),
        true,
        None,
        |app, ui| {
            assert_eq!(
                app.advance_viewer_primary_pointer_frame(ui.ctx(), false),
                Allow
            );
            assert_eq!(app.fs_primary_suppression, Idle);
            (false, false)
        },
    );
}

#[cfg(windows)]
#[test]
fn suppression_906_next_single_click_dismisses_overflow_after_restore() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::for_focus_restore();
    canvas.app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    f2(&mut canvas, true);
    let pos = egui::pos2(100.0, 180.0);
    assert_eq!(
        canvas.merge(click(pos, egui::Modifiers::NONE)),
        (false, false)
    );
    assert_eq!(
        canvas.app.fs_overflow_panel_state,
        FsOverflowPanelState::Root
    );
    f2(&mut canvas, false);
    // End egui's pair so this is an unambiguous single click.
    canvas.time += 1.0;
    assert_eq!(
        canvas.merge(click(pos, egui::Modifiers::NONE)),
        (false, false)
    );
    assert_eq!(
        canvas.app.fs_overflow_panel_state,
        FsOverflowPanelState::Closed
    );
}

#[test]
fn suppression_906_released_state_reason_composition_and_touch_ownership() {
    let mut state = Idle;
    assert_eq!(state.arm(false, FocusRestore), Suppress(FocusRestore));
    assert_eq!(state, ReleasedThisPass(FocusRestore));
    assert!(state.is_active());
    assert_eq!(state.arm(false, Other), Suppress(Other));
    assert_eq!(state.arm(false, FocusRestore), Suppress(Other));
    assert_eq!(state, ReleasedThisPass(Other));
    for released in [false, true] {
        state = ReleasedThisPass(FocusRestore);
        assert_eq!(
            state.apply(FullscreenPrimarySuppressionEvent::PointerFrame { released }),
            Allow
        );
        assert_eq!(state, Idle);
    }
    for down in [false, true] {
        state = TouchStream;
        assert_eq!(state.arm(down, FocusRestore), Suppress(Other));
        assert_eq!(
            state,
            if down {
                PointerStream(Other)
            } else {
                TouchStream
            }
        );
    }
    state = ReleasedThisPass(Other);
    assert_eq!(state.arm(true, FocusRestore), Suppress(Other));
    assert_eq!(state, PointerStream(Other));
    for (active, completed, cancelled, suppresses, decision, expected) in [
        (
            false,
            false,
            false,
            false,
            Preserve,
            ReleasedThisPass(FocusRestore),
        ),
        (
            true,
            false,
            false,
            false,
            Allow,
            ReleasedThisPass(FocusRestore),
        ),
        (true, false, false, true, Suppress(Other), TouchStream),
        (false, true, false, false, Allow, Idle),
        (false, false, true, true, Suppress(Other), Idle),
    ] {
        state = ReleasedThisPass(FocusRestore);
        assert_eq!(
            state.apply(FullscreenPrimarySuppressionEvent::TouchFrame {
                active,
                completed,
                cancelled,
                suppresses_current_frame: suppresses,
            }),
            decision
        );
        assert_eq!(state, expected);
    }
}

#[test]
fn suppression_906_arm_reads_final_context_level_and_keeps_touch_owner() {
    let mut canvas = Canvas::new();
    for (initial, events, expected, reason) in [
        (
            Idle,
            click(canvas_pos(), egui::Modifiers::NONE),
            ReleasedThisPass(FocusRestore),
            FocusRestore,
        ),
        (
            ReleasedThisPass(Other),
            vec![button(canvas_pos(), true, egui::Modifiers::NONE)],
            PointerStream(Other),
            Other,
        ),
        (
            TouchStream,
            vec![button(canvas_pos(), false, egui::Modifiers::NONE)],
            TouchStream,
            Other,
        ),
    ] {
        canvas.app.fs_primary_suppression = initial;
        canvas.merge_using(events, true, None, |app, ui| {
            assert_eq!(
                app.arm_fs_primary_suppression(ui.ctx(), FocusRestore),
                Suppress(reason)
            );
            assert_eq!(app.fs_primary_suppression, expected);
            assert!(app.fs_primary_suppression.is_active());
            (false, false)
        });
    }
}

#[cfg(windows)]
#[test]
fn suppression_906_detached_uses_actual_viewport_and_preserves_sibling_context() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = page_canvas();
    let build = |owner: &mut App| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/906/detached.png"))];
        owner.visible_indices = vec![0];
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.fs_zoom = 1.0;
        owner.fs_pan = egui::Vec2::ZERO;
    };
    let a = canvas.app.build_window_context_for_test(19061, |owner| {
        build(owner);
        owner
            .items
            .push(GridItem::Image(PathBuf::from("c:/906/next.png")));
        owner.visible_indices = vec![0, 1];
        owner.reading_direction = crate::settings::ReadingDirection::Ltr;
    });
    let b = canvas.app.build_window_context_for_test(19062, build);
    let ctx = canvas.ctx.clone();
    canvas.app.with_window_context_for_test(a, |owner| {
        owner.settings.page_turn_animation_enabled = false;
        for idx in 0..2 {
            crate::app::source_texture_test_support::seed(owner, &ctx, idx);
        }
        assert_eq!(owner.get_nav_indices().as_slice(), &[0, 1]);
    });
    canvas.app.with_window_context_for_test(b, |owner| {
        owner.fs_zoom = 2.5;
        owner.fs_pan = egui::vec2(10.0, 20.0);
        owner.items_generation = 9062;
    });
    let viewport_a = canvas.app.with_window_context_for_test(a, |owner| {
        owner.begin_active_detached_session(19061, crate::app::DetachedSource::Image);
        owner.fullscreen_viewport_id()
    });
    let main_items = canvas.app.items.clone();
    let main_generation = canvas.app.items_generation;
    assert_ne!(viewport_a, egui::ViewportId::ROOT);
    // ROOT still has a physical hold; A must read A's final up state instead.
    canvas.merge_using(
        vec![button(canvas_pos(), true, egui::Modifiers::NONE)],
        true,
        None,
        |_, ui| {
            assert_eq!(ui.ctx().viewport_id(), egui::ViewportId::ROOT);
            assert!(ui.ctx().input(|i| i.pointer.primary_down()));
            (false, false)
        },
    );
    let run = |app: &mut App, ui: &mut egui::Ui| {
        assert_eq!(ui.ctx().viewport_id(), viewport_a);
        app.with_window_context_for_test(a, |owner| {
            owner.begin_active_detached_session(19061, crate::app::DetachedSource::Image);
            let (result, operation) =
                super::super::wheel_frame_tests::actual_merge(owner, ui, false);
            let facts = (result.close, !result.page_nav.is_none());
            if facts.1 {
                owner.handle_fs_navigation_with_operation(
                    ui.ctx(),
                    result.close,
                    false,
                    None,
                    None,
                    None,
                    result.page_nav,
                    None,
                    0,
                    operation,
                );
                assert_eq!(owner.fullscreen_idx, Some(1));
            }
            facts
        })
    };
    canvas.merge_using_on(
        viewport_a,
        vec![egui::Event::PointerMoved(canvas_pos())],
        true,
        None,
        run,
    );
    f2(&mut canvas, true);
    assert_eq!(
        canvas.merge_using_on(
            viewport_a,
            click(canvas_pos(), egui::Modifiers::NONE),
            true,
            None,
            run
        ),
        (false, false)
    );
    assert_eq!(
        canvas.app.fs_primary_suppression,
        ReleasedThisPass(FocusRestore)
    );
    f2(&mut canvas, false);
    // Register fs_click again on A, not ROOT. No button edge is delivered, so
    // old PointerStream suppression would survive this redraw.
    assert_eq!(
        canvas.merge_using_on(viewport_a, vec![], true, None, run),
        (false, false)
    );
    assert_eq!(canvas.app.fs_primary_suppression, Idle);
    canvas.app.with_window_context_for_test(a, |owner| {
        assert_eq!(owner.fullscreen_idx, Some(0));
    });
    assert_eq!(
        canvas.merge_using_on(
            viewport_a,
            click(canvas_pos(), egui::Modifiers::NONE),
            true,
            None,
            run
        ),
        (false, true)
    );
    assert_eq!(canvas.app.fs_primary_suppression, Idle);
    canvas.app.with_window_context_for_test(a, |owner| {
        assert_eq!(owner.fullscreen_idx, Some(1));
    });
    assert_eq!(canvas.app.items, main_items);
    assert_eq!(canvas.app.items_generation, main_generation);
    canvas.app.with_window_context_for_test(b, |owner| {
        assert_eq!(
            owner.items,
            vec![GridItem::Image(PathBuf::from("c:/906/detached.png"))]
        );
        assert_eq!(owner.fullscreen_idx, Some(0));
        assert_eq!(owner.items_generation, 9062);
        assert_eq!(owner.fs_zoom, 2.5);
        assert_eq!(owner.fs_pan, egui::vec2(10.0, 20.0));
    });
}
