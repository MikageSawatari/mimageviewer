//! Actual key, wheel, click and deferred-owner boundaries.
use super::*;

pub(super) fn click(pos: egui::Pos2, double: bool) -> Vec<egui::Event> {
    let mut events = vec![egui::Event::PointerMoved(pos)];
    for _ in 0..if double { 2 } else { 1 } {
        events.extend([true, false].map(|pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }));
    }
    events
}

#[test]
fn plane_key_noop_and_rejection_preserve_pending_initial_placement_and_pan() {
    let _keys = crate::key_input::lock_test_input();
    for (blocked, stale) in [(false, false), (true, false), (false, true)] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        app.keymap = crate::keymap::Keymap::from_ini_str(
            "[FsImage]\nFsFisheyeIn = F13\nFsFisheyeOut = F14\n",
        );
        app.view_trim_mode = blocked;
        viewer.frame_with_before(
            &mut app,
            key(if blocked {
                egui::Key::F13
            } else {
                egui::Key::F14
            }),
            |app, ctx| {
                let old_pose = stale.then(|| PlaneViewPose {
                    anchor: PlaneViewAnchor {
                        slice: crate::page_split::PageSlice::Left,
                        ..app.plane_view_anchor(0)
                    },
                    strength: 0.25,
                    lens_center: egui::Vec2::splat(0.5),
                });
                if let Some(pose) = old_pose {
                    app.set_fs_exclusive_view(
                        FsExclusiveView::PlaneView(PlaneViewState::new(pose)),
                        "old_slice_fixture",
                    );
                }
                app.fs_pan = egui::vec2(12345., -6789.);
                app.request_fs_initial_placement_with_kind(0, FsInitialPlacementKind::LandingStart);
                let pending = app
                    .fs_initial_placement_request
                    .expect("waiting for complete material");
                let before = (app.fs_zoom, app.fs_pan);
                app.handle_fs_key_input(ctx, 0, false);
                assert!(
                    !app.keymap.consume_action(
                        ctx,
                        if blocked {
                            KeyAction::FsFisheyeIn
                        } else {
                            KeyAction::FsFisheyeOut
                        }
                    ),
                    "the real handler consumed the assigned action"
                );
                assert_eq!(app.fs_initial_placement_request, Some(pending));
                assert_eq!(
                    (app.fs_zoom, app.fs_pan),
                    before,
                    "no pan clamp on a rejected/no-op key"
                );
                assert_eq!(
                    app.plane_view_pose(),
                    old_pose,
                    "no-op key must leave stale-owner retirement to the continuation check"
                );
                if blocked {
                    assert!(
                        matches!(
                            app.fs_boundary_hint,
                            Some(FsBoundaryHint::NavNoOp {
                                reason: FsNavNoOpReason::PlaneEntry(PlaneViewEntryBlock::Trim),
                                ..
                            })
                        ),
                        "rejection publishes its reason through the fullscreen boundary notice"
                    );
                }
            },
        );
    }
}

#[test]
fn plane_wheel_noop_and_rejection_preserve_pending_initial_placement_and_pan() {
    let _keys = crate::key_input::lock_test_input();
    for blocked in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let viewer = viewer(&mut app);
        app.view_trim_mode = blocked;
        viewer
            .ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size)),
                    events: vec![
                        egui::Event::PointerMoved(viewer.size.to_pos2() * 0.5),
                        wheel(
                            0.,
                            if blocked { 40. } else { -40. },
                            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                        ),
                    ],
                    ..Default::default()
                },
                |ui| {
                    app.fs_pan = egui::vec2(12345., -6789.);
                    app.request_fs_initial_placement_with_kind(
                        0,
                        FsInitialPlacementKind::LandingStart,
                    );
                    let pending = app
                        .fs_initial_placement_request
                        .expect("waiting for complete material");
                    let before = (app.fs_zoom, app.fs_pan);
                    let (result, _) =
                        crate::ui_fullscreen::wheel_frame_tests::actual_merge(&mut app, ui, false);
                    assert!(result.page_nav.is_none());
                    assert!(!result.close);
                    assert_eq!(app.fs_initial_placement_request, Some(pending));
                    assert_eq!(
                        (app.fs_zoom, app.fs_pan),
                        before,
                        "no pan clamp in prediction or application"
                    );
                    assert!(!app.fs_exclusive_view.is_plane_view());
                    if blocked {
                        assert!(
                            matches!(
                                app.fs_boundary_hint,
                                Some(FsBoundaryHint::NavNoOp {
                                    reason: FsNavNoOpReason::PlaneEntry(PlaneViewEntryBlock::Trim),
                                    ..
                                })
                            ),
                            "rejection publishes its reason through the fullscreen boundary notice"
                        );
                    }
                },
            )
            .drop_without_applying_deltas();
    }
}

#[test]
fn plane_same_pass_entry_exit_clicks_turn_close_or_reset_from_the_predicted_view() {
    let _keys = crate::key_input::lock_test_input();
    use crate::settings::FullscreenPrimaryClickMode::{DoubleClickClose, PageTurn};
    for mode in [PageTurn, DoubleClickClose] {
        for leaving in [false, true] {
            for double in [false, true] {
                let mut app = crate::app::setup_app_for_test();
                let mut viewer = viewer(&mut app);
                app.settings.fullscreen_primary_click = mode;
                if leaving {
                    enter(&mut app, &mut viewer);
                }
                app.fs_focus_regained_at = None;
                let pos = viewer.size.to_pos2() * 0.5;
                let expected = plane_view_wheel_transition(wheel_input_for_current_view(
                    &mut app,
                    &viewer.ctx,
                    egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size),
                    Some(pos),
                    if leaving { -1. } else { 1. },
                ));
                assert!(
                    expected.zoom > 1.,
                    "cover enlargement remains after leaving"
                );
                let mut events = vec![wheel(
                    0.,
                    if leaving { -40. } else { 40. },
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                )];
                events.extend(click(pos, double));
                viewer.frame(&mut app, events);
                assert_eq!(
                    app.fullscreen_idx,
                    Some(0),
                    "{mode:?} leaving={leaving} double={double}"
                );
                let remains = !leaving && !double;
                assert_eq!(
                    app.fs_exclusive_view.is_plane_view(),
                    remains,
                    "{mode:?} leaving={leaving} double={double}"
                );
                if !double {
                    assert_eq!(app.plane_view_pose(), expected.pose);
                    assert!((app.fs_zoom - expected.zoom).abs() < 1.0e-5);
                    assert!((app.fs_pan - expected.pan).length() < 0.01);
                    if remains {
                        assert_eq!(app.plane_view_pose().unwrap().strength, 0.25);
                    }
                } else {
                    assert!(
                        (app.fs_zoom - 1.).abs() < 1.0e-5,
                        "double click resets either the projection or the retained ordinary zoom"
                    );
                    assert_eq!(app.fs_pan, egui::Vec2::ZERO);
                    assert_eq!(app.fs_free_rotation, 0.);
                }
                // At the resulting fit view, a later real click still follows the chosen mode.
                if !double {
                    app.frame_counter += 60;
                    viewer.frame(&mut app, click(pos, true));
                    assert_eq!(app.fullscreen_idx, Some(0));
                    assert!((app.fs_zoom - 1.).abs() < 1.0e-5);
                    assert!(!app.fs_exclusive_view.is_plane_view());
                }
                app.frame_counter += 60;
                app.fs_focus_regained_at = None;
                viewer.frame(&mut app, click(pos, mode == DoubleClickClose));
                assert_eq!(
                    app.fullscreen_idx,
                    if mode == PageTurn { Some(1) } else { None }
                );
            }
        }
    }
}

#[test]
fn plane_deferred_real_clicks_do_not_rebind_to_another_page_or_slice() {
    let _keys = crate::key_input::lock_test_input();
    for slice_change in [false, true] {
        for double in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = viewer(&mut app);
            if slice_change {
                app.spread_mode = crate::settings::SpreadMode::SplitLtr;
                app.fullscreen_idx = Some(2);
                app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
                app.record_page_dims_for_spread(2, (240, 120));
                viewer.frame(&mut app, vec![]);
                assert_eq!(
                    app.fullscreen_page_slice,
                    crate::page_split::PageSlice::Left
                );
            }
            if double {
                enter(&mut app, &mut viewer);
            }
            app.fs_focus_regained_at = None;
            let pos = viewer.size.to_pos2() * 0.5;
            let mut events = vec![
                wheel(0., -40., egui::Modifiers::NONE),
                wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
            ];
            events.extend(click(pos, double));
            viewer
                .ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size)),
                        time: Some((app.frame_counter + 1) as f64 / 60.),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let (mut result, mut operation) =
                            crate::ui_fullscreen::wheel_frame_tests::collect_actual_merge(
                                &mut app, ui, false,
                            );
                        let mut proposal = result
                            .deferred
                            .take()
                            .expect("mixed delivery defers its final merge");
                        assert!(
                            matches!(proposal.canvas_click, FsCanvasClickAction::ResetTransform)
                                == double
                        );
                        assert!(
                            double
                                || matches!(
                                    proposal.canvas_click,
                                    FsCanvasClickAction::PageSide(_)
                                )
                        );
                        // A later owner consumes wheel input and commits a new display unit
                        // before this retained click proposal is applied.
                        crate::wheel_input::consume_popup_wheel_input(ui.ctx());
                        let previous_idx = app.fullscreen_idx.unwrap();
                        let navigation = if slice_change {
                            app.spread_page_nav(1)
                        } else {
                            FsPageNav::Target(3)
                        };
                        app.handle_fs_navigation_with_operation(
                            ui.ctx(),
                            false,
                            false,
                            None,
                            None,
                            None,
                            navigation,
                            None,
                            previous_idx,
                            None,
                        );
                        assert_eq!(app.fullscreen_idx, Some(if slice_change { 2 } else { 3 }));
                        assert_eq!(
                            app.fullscreen_page_slice,
                            if slice_change {
                                crate::page_split::PageSlice::Right
                            } else {
                                crate::page_split::PageSlice::Full
                            }
                        );
                        app.fs_zoom = 2.;
                        app.fs_pan = egui::vec2(15., -20.);
                        app.fs_free_rotation = 0.2;
                        let owner = (app.fullscreen_idx, app.fullscreen_page_slice);
                        app.claim_fs_wheel_frame_after_ui(&mut proposal);
                        let result =
                            app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                        assert!(
                            result.page_nav.is_none(),
                            "the old PageSide click must be discarded"
                        );
                        assert!(!result.close);
                        assert!(!result.primary_click_close);
                        assert_eq!((app.fullscreen_idx, app.fullscreen_page_slice), owner);
                        assert_eq!(
                            (app.fs_zoom, app.fs_pan, app.fs_free_rotation),
                            (2., egui::vec2(15., -20.), 0.2),
                            "the old double click must not reset the new unit"
                        );
                    },
                )
                .drop_without_applying_deltas();
        }
    }
}
