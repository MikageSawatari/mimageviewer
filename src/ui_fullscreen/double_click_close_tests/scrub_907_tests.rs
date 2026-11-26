use super::*;

#[test]
fn primary_semantic_907_keeps_press_position_after_same_pass_release() {
    let mut canvas = Canvas::new();
    let start = canvas_pos();
    let end = start + egui::vec2(60.0, 0.0);
    canvas.merge_using(
        vec![
            button(start, true, egui::Modifiers::NONE),
            egui::Event::PointerMoved(end),
            button(end, false, egui::Modifiers::NONE),
        ],
        true,
        None,
        |app, ui| {
            assert!(
                ui.ctx()
                    .input(|input| input.pointer.press_origin())
                    .is_none()
            );
            let response = ui.interact(
                viewport(),
                egui::Id::new("907-primary-semantic-canvas"),
                egui::Sense::click_and_drag(),
            );
            let input = FsWheelFrameSemanticInput::capture(app, ui.ctx(), &response);
            assert_eq!(input.primary_press_origin, Some(start));
            (false, false)
        },
    );
}

#[cfg(windows)]
fn arm_scrub(canvas: &mut Canvas, viewport_id: egui::ViewportId) {
    canvas.app.keymap =
        crate::keymap::Keymap::from_ini_str("[FsImage]\nFsScrubZoomHold = Ctrl+Tab\n");
    crate::key_input::arm_test_synthetic_input(0xE907, viewport_id);
    super::suppression_906_tests::scrub_key(true);
}

#[cfg(windows)]
fn run(app: &mut App, ui: &mut egui::Ui) -> (bool, bool) {
    app.keyboard_owner_for_pass(ui.ctx());
    let (result, _) = super::super::wheel_frame_tests::actual_merge(app, ui, false);
    (result.close, !result.page_nav.is_none())
}

#[cfg(windows)]
#[test]
fn scrub_907_each_primary_flow_keeps_last_press_but_preserves_existing_suppression() {
    let _input = crate::key_input::lock_test_input();
    for previous_pass in [false, true] {
        for positionless in [false, true] {
            let mut canvas = super::suppression_906_tests::page_canvas();
            arm_scrub(&mut canvas, egui::ViewportId::ROOT);
            if previous_pass {
                assert_eq!(
                    canvas.merge_using(
                        vec![button(canvas_pos(), true, egui::Modifiers::CTRL)],
                        true,
                        None,
                        run,
                    ),
                    (false, false)
                );
                assert!(canvas.app.fs_middle_zoom_drag.is_some());
            }
            let mut events = if previous_pass {
                vec![]
            } else {
                vec![button(canvas_pos(), true, egui::Modifiers::CTRL)]
            };
            events.push(if positionless {
                egui::Event::PointerButtonReleased {
                    button: egui::PointerButton::Primary,
                    modifiers: egui::Modifiers::CTRL,
                }
            } else {
                button(canvas_pos(), false, egui::Modifiers::CTRL)
            });
            let pivot = canvas_pos() + egui::vec2(3.0, 0.0);
            events.push(button(pivot, true, egui::Modifiers::CTRL));
            assert_eq!(canvas.merge_using(events, true, None, run), (false, false));
            assert_eq!(canvas.app.fs_middle_zoom_drag.is_some(), !previous_pass);
            assert_eq!(
                canvas.app.fs_primary_suppression,
                FullscreenPrimarySuppression::PointerStream(
                    FullscreenPrimarySuppressionReason::Other
                )
            );
            let before = canvas.app.fs_zoom;
            canvas.merge_using(
                vec![egui::Event::PointerMoved(pivot + egui::vec2(80.0, 0.0))],
                true,
                None,
                run,
            );
            assert_eq!(canvas.app.fs_zoom != before, !previous_pass);
            canvas.merge_using(
                vec![button(pivot, false, egui::Modifiers::CTRL)],
                true,
                None,
                run,
            );
            assert!(canvas.app.fs_middle_zoom_drag.is_none());
            super::suppression_906_tests::scrub_key(false);
            assert_eq!(canvas.merge_using(vec![], true, None, run), (false, false));
            assert_eq!(
                canvas.app.fs_primary_suppression,
                FullscreenPrimarySuppression::Idle
            );
            // The last held stream has a fresh, unrelated normal successor.
            canvas.app.fs_zoom = 1.0;
            canvas.app.fs_pan = egui::Vec2::ZERO;
            assert_eq!(
                canvas.merge_using(click(canvas_pos(), egui::Modifiers::NONE), true, None, run),
                (false, true)
            );
            crate::key_input::clear_test_synthetic_input();
        }
    }
}

#[cfg(windows)]
#[test]
fn close_mode_900_scrub_907_detached_actual_viewport_preserves_sibling() {
    let _input = crate::key_input::lock_test_input();
    for scrub in [false, true] {
        let mut canvas = super::suppression_906_tests::page_canvas();
        canvas.app.settings.fullscreen_primary_click = if scrub {
            FullscreenPrimaryClickMode::PageTurn
        } else {
            FullscreenPrimaryClickMode::DoubleClickClose
        };
        let main_items = canvas.app.items.clone();
        let main_generation = canvas.app.items_generation;
        let build = |owner: &mut App| {
            owner.items = vec![GridItem::Image(PathBuf::from("c:/907/detached.png"))];
            owner.visible_indices = vec![0];
            owner.fullscreen_idx = Some(0);
            owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            owner.fs_zoom = 1.0;
            owner.fs_pan = egui::Vec2::ZERO;
        };
        let a = canvas.app.build_window_context_for_test(19071, build);
        let b = canvas.app.build_window_context_for_test(19072, |owner| {
            build(owner);
            owner.fs_zoom = 2.5;
            owner.fs_pan = egui::vec2(10.0, 20.0);
            owner.items_generation = 9072;
        });
        let ctx = canvas.ctx.clone();
        let viewport_a = canvas.app.with_window_context_for_test(a, |owner| {
            crate::app::source_texture_test_support::seed(owner, &ctx, 0);
            owner.begin_active_detached_session(19071, crate::app::DetachedSource::Image);
            owner.fullscreen_viewport_id()
        });
        assert_ne!(viewport_a, egui::ViewportId::ROOT);
        let run_detached = |app: &mut App, ui: &mut egui::Ui| {
            assert_eq!(ui.ctx().viewport_id(), viewport_a);
            app.with_window_context_for_test(a, |owner| {
                owner.begin_active_detached_session(19071, crate::app::DetachedSource::Image);
                owner.keyboard_owner_for_pass(ui.ctx());
                let (result, operation) =
                    super::super::wheel_frame_tests::actual_merge(owner, ui, false);
                let facts = (result.close, !result.page_nav.is_none());
                if result.close {
                    owner.handle_fs_navigation_with_operation(
                        ui.ctx(),
                        true,
                        false,
                        None,
                        None,
                        None,
                        result.page_nav,
                        None,
                        0,
                        operation,
                    );
                    assert!(owner.fullscreen_idx.is_none());
                }
                facts
            })
        };
        canvas.merge_using_on(
            viewport_a,
            vec![egui::Event::PointerMoved(canvas_pos())],
            true,
            None,
            run_detached,
        );
        if scrub {
            arm_scrub(&mut canvas, viewport_a);
            assert_eq!(
                canvas.merge_using_on(
                    viewport_a,
                    click(canvas_pos(), egui::Modifiers::CTRL),
                    true,
                    None,
                    run_detached
                ),
                (false, false)
            );
            assert_eq!(
                canvas.app.fs_primary_suppression,
                FullscreenPrimarySuppression::ReleasedThisPass(
                    FullscreenPrimarySuppressionReason::Other
                )
            );
            super::suppression_906_tests::scrub_key(false);
            canvas.merge_using_on(viewport_a, vec![], true, None, run_detached);
            assert_eq!(
                canvas.app.fs_primary_suppression,
                FullscreenPrimarySuppression::Idle
            );
            canvas.app.with_window_context_for_test(a, |owner| {
                assert_eq!(owner.fullscreen_idx, Some(0));
                assert_eq!(owner.fs_zoom, 1.0);
            });
            crate::key_input::clear_test_synthetic_input();
        } else {
            canvas
                .app
                .with_window_context_for_test(a, |owner| owner.fs_zoom = 2.0);
            for closes in [false, false, false, true] {
                assert_eq!(
                    canvas.merge_using_on(
                        viewport_a,
                        click(canvas_pos(), egui::Modifiers::NONE),
                        true,
                        None,
                        run_detached
                    ),
                    (closes, false)
                );
            }
        }
        assert_eq!(canvas.app.items, main_items);
        assert_eq!(canvas.app.items_generation, main_generation);
        assert_eq!(canvas.app.fullscreen_idx, Some(0));
        canvas.app.with_window_context_for_test(b, |owner| {
            assert_eq!(
                owner.items,
                vec![GridItem::Image(PathBuf::from("c:/907/detached.png"))]
            );
            assert_eq!(owner.fullscreen_idx, Some(0));
            assert_eq!(owner.fs_zoom, 2.5);
            assert_eq!(owner.fs_pan, egui::vec2(10.0, 20.0));
            assert_eq!(owner.items_generation, 9072);
        });
    }
}
