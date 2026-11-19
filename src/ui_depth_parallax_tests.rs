//! Pointer-level regressions for the actual 3D-photo panel painter.

use crate::depth_parallax::{DepthParallaxSettings, MotionKind};
use eframe::egui;
use egui_kittest::{Harness, kittest::Queryable};

#[derive(Default)]
struct State {
    settings: DepthParallaxSettings,
    hidden: bool,
    close_requested: bool,
    batched_clicks: bool,
    focus_adjustments: usize,
    changes: usize,
    toggle_discards: usize,
    inputless_rebuilds: usize,
    slideshow_progress: Option<f32>,
    slideshow_progress_after_draw: Option<Option<f32>>,
    panel: Option<super::DepthPanelResponse>,
}

fn harness(size: egui::Vec2, batched_clicks: bool) -> Harness<'static, State> {
    harness_with_initial_collapse(size, batched_clicks, false)
}

fn harness_with_initial_collapse(
    size: egui::Vec2,
    batched_clicks: bool,
    collapsed: bool,
) -> Harness<'static, State> {
    crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(size)
        .build_ui_state(
            |ui, state: &mut State| {
                if !state.hidden {
                    assert_eq!(ui.ctx().options(|options| options.max_passes.get()), 2);
                    let panel = super::draw_depth_parallax_panel_with_export(
                        ui.ctx(),
                        &mut state.settings,
                        "",
                        false,
                        Some("I / Tab: パネルを隠す・出す".into()),
                        state.slideshow_progress,
                    );
                    if let Some(progress) = state.slideshow_progress_after_draw {
                        panel.set_slideshow_progress(progress);
                    }
                    state.close_requested |= panel.close_requested;
                    state.focus_adjustments += usize::from(panel.focus_adjusting);
                    state.changes += usize::from(panel.changed);
                    state.toggle_discards += usize::from(panel.changed && ui.ctx().will_discard());
                    if ui.ctx().current_pass_index() > 0 {
                        assert!(ui.ctx().input(|input| input.raw.events.is_empty()));
                        assert!(!panel.changed, "rebuilding must not toggle a second time");
                        assert!(!ui.ctx().will_discard(), "rebuilding must settle");
                        state.inputless_rebuilds += 1;
                    }
                    state.panel = Some(panel);
                }
            },
            State {
                batched_clicks,
                settings: DepthParallaxSettings {
                    panel_collapsed: collapsed,
                    ..Default::default()
                },
                ..State::default()
            },
        )
}

fn progress_circles_in_rect(shapes: &[egui::epaint::ClippedShape], rect: egui::Rect) -> usize {
    fn count(shape: &egui::Shape, rect: egui::Rect) -> usize {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().map(|shape| count(shape, rect)).sum(),
            egui::Shape::Circle(circle)
                if circle.radius > 0.0
                    && circle.radius <= 9.0
                    && circle.center.distance(rect.center()) < 0.01 =>
            {
                assert!(rect.contains_rect(egui::Rect::from_center_size(
                    circle.center,
                    egui::Vec2::splat(circle.radius * 2.0),
                )));
                1
            }
            _ => 0,
        }
    }
    shapes.iter().map(|shape| count(&shape.shape, rect)).sum()
}

#[test]
fn depth_panel_slideshow_progress_stays_inside_header_and_is_not_clickable() {
    for size in [egui::vec2(1093., 614.), egui::vec2(546.5, 307.)] {
        for collapsed in [false, true] {
            for progress in [None, Some(0.65)] {
                let mut harness = harness_with_initial_collapse(size, false, collapsed);
                harness.state_mut().slideshow_progress = progress;
                harness.run();
                let panel = harness.state().panel.as_ref().unwrap();
                let rect = panel.slideshow_progress_rect;
                let title = harness
                    .get_by_role_and_label(egui::accesskit::Role::Button, "3D 写真")
                    .rect();
                assert!(rect.width() > 0. && rect.width() <= 18.);
                assert!(rect.height() > 0. && rect.height() <= 18.);
                assert!(panel.header_rect.contains_rect(rect));
                assert!(rect.left() >= title.right());
                assert!(rect.right() <= panel.close_rect.left());
                assert_eq!(
                    progress_circles_in_rect(&harness.output().shapes, rect),
                    usize::from(progress.is_some()),
                );

                harness.hover_at(rect.center());
                harness.run();
                assert_ne!(
                    harness.output().platform_output.cursor_icon,
                    egui::CursorIcon::PointingHand,
                );
                click_at(&mut harness, rect.center());
                assert_eq!(harness.state().settings.panel_collapsed, collapsed);
                assert_eq!(harness.state().changes, 0);
                assert_eq!(harness.state().focus_adjustments, 0);
                assert!(!harness.state().close_requested);
                assert!(!harness.state().panel.as_ref().unwrap().export_requested);
                assert_eq!(
                    harness
                        .state()
                        .panel
                        .as_ref()
                        .unwrap()
                        .slideshow_progress_rect,
                    rect,
                    "progress must not move after hover or click",
                );
            }
        }
    }
}

#[test]
fn depth_panel_slideshow_progress_replaces_the_same_frame_paint_slot() {
    for collapsed in [false, true] {
        for (initial, latest) in [
            (None, Some(0.65)),
            (Some(0.25), Some(0.65)),
            (Some(0.65), None),
        ] {
            let mut harness =
                harness_with_initial_collapse(egui::vec2(546.5, 307.), false, collapsed);
            harness.state_mut().slideshow_progress = initial;
            harness.state_mut().slideshow_progress_after_draw = Some(latest);
            harness.run();
            let panel = harness.state().panel.as_ref().unwrap();
            assert_eq!(
                progress_circles_in_rect(&harness.output().shapes, panel.slideshow_progress_rect),
                usize::from(latest.is_some()),
                "only the latest progress may remain painted: initial={initial:?}, latest={latest:?}",
            );
            assert_eq!(harness.state().changes, 0);
            assert_eq!(harness.state().settings.panel_collapsed, collapsed);
        }
    }
}

#[test]
fn depth_panel_initially_collapsed_paints_title_and_close_inside_header() {
    for size in [egui::vec2(1093., 614.), egui::vec2(546.5, 307.)] {
        let mut harness = harness_with_initial_collapse(size, false, true);
        harness.run();
        let panel = harness.state().panel.as_ref().unwrap();
        let title = harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "3D 写真")
            .rect();
        assert!(title.width() > 70. && title.height() > 10.);
        assert!(
            panel.header_rect.contains_rect(title),
            "title={title:?}, header={:?}",
            panel.header_rect
        );
        assert!(
            panel.header_rect.contains_rect(panel.close_rect),
            "close={:?}, header={:?}",
            panel.close_rect,
            panel.header_rect
        );
        fn contains_text(shape: &egui::Shape, label: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text() == label,
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, label)),
                _ => false,
            }
        }
        for label in ["3D 写真", "×"] {
            assert!(
                harness
                    .output()
                    .shapes
                    .iter()
                    .any(|shape| contains_text(&shape.shape, label)),
                "missing paint {label}, panel={:?}, header={:?}, close={:?}",
                panel.rect,
                panel.header_rect,
                panel.close_rect
            );
        }
        let close = panel.close_rect;
        click_at(&mut harness, close.center());
        assert!(harness.state().close_requested);
    }
}

#[test]
fn depth_panel_collapse_changes_once_keeps_header_and_releases_old_body_layer() {
    for size in [egui::vec2(1093., 614.), egui::vec2(546.5, 307.)] {
        for batched in [false, true] {
            let mut harness = harness(size, batched);
            harness.run();
            let panel = harness.state().panel.as_ref().unwrap();
            let expanded = panel.rect;
            let header = panel.header_rect;
            let close = panel.close_rect;
            let title = harness
                .get_by_role_and_label(egui::accesskit::Role::Button, "3D 写真")
                .rect();
            harness.hover_at(title.center());
            harness.run();
            assert_eq!(
                harness.output().platform_output.cursor_icon,
                egui::CursorIcon::PointingHand
            );
            // Both the triangle and the text belong to this single click target.
            let click = if batched {
                title.left_center() + egui::vec2(4., 0.)
            } else {
                title.center()
            };
            let rebuilds = harness.state().inputless_rebuilds;
            click_at(&mut harness, click);
            assert!(harness.state().settings.panel_collapsed);
            assert_eq!(harness.state().changes, 1);
            assert_eq!(harness.state().toggle_discards, 1);
            assert_eq!(harness.state().inputless_rebuilds, rebuilds + 1);
            let panel = harness.state().panel.as_ref().unwrap();
            assert_eq!(panel.header_rect, header);
            assert_eq!(panel.close_rect, close);
            assert_eq!(panel.rect.width(), expanded.width());
            assert!(panel.rect.height() < 45. && panel.rect.height() < expanded.height() * 0.3);
            assert_eq!(panel.footer_rect, egui::Rect::NOTHING);
            assert_eq!(panel.export_rect, egui::Rect::NOTHING);
            assert!(panel.scroll.is_none());
            let body = expanded.center_bottom() - egui::vec2(0., 20.);
            assert_ne!(
                harness.ctx.layer_id_at(body).map(|layer| layer.id),
                Some(egui::Id::unique("depth_parallax_panel"))
            );
            harness.run();
            assert_eq!(harness.state().changes, 1);
            assert_eq!(harness.state().toggle_discards, 1);
            assert_eq!(
                harness.output().platform_output.num_completed_passes,
                1,
                "ordinary repaint must not discard"
            );
            click_at(&mut harness, title.center());
            assert!(!harness.state().settings.panel_collapsed);
            assert_eq!(harness.state().changes, 2);
            assert_eq!(harness.state().toggle_discards, 2);
            assert!(harness.state().panel.as_ref().unwrap().scroll.is_some());
            click_at(&mut harness, title.center());
            click_at(&mut harness, close.center());
            assert!(harness.state().close_requested);
            assert_eq!(harness.state().changes, 3);
            assert_eq!(harness.state().toggle_discards, 3);
        }
    }
}

#[test]
fn depth_focus_slider_and_default_trigger_highlight_but_hover_and_shaping_do_not() {
    let mut harness = harness(egui::vec2(1093., 614.), false);
    harness.run();
    let slider = harness
        .get_by_role_and_label(egui::accesskit::Role::Slider, "止まる深さ")
        .rect();
    harness.hover_at(slider.center());
    harness.run();
    assert_eq!(harness.state().focus_adjustments, 0);
    pointer(&mut harness, slider.center(), true);
    harness.event(egui::Event::PointerMoved(slider.right_center()));
    harness.step();
    assert!(harness.state().focus_adjustments > 0);
    pointer(&mut harness, slider.right_center(), false);
    harness.run();
    let before = harness.state().focus_adjustments;
    let reset = harness
        .get_by_role_and_label(egui::accesskit::Role::Button, "既定値")
        .rect();
    click_at(&mut harness, reset.center());
    assert_eq!(harness.state().settings.focus, 0.5);
    assert!(harness.state().focus_adjustments > before);
    let before = harness.state().focus_adjustments;
    harness
        .get_by_role_and_label(egui::accesskit::Role::Slider, "止まる深さ")
        .focus();
    harness.event(egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
    harness.event(egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run();
    assert!(harness.state().focus_adjustments > before);
    let focus = harness.state().settings.focus;
    let before = harness.state().focus_adjustments;
    let shape = harness
        .get_by_role_and_label(egui::accesskit::Role::CheckBox, "奥行きを整える")
        .rect();
    click_at(&mut harness, shape.center());
    assert_eq!(harness.state().settings.focus, focus);
    assert_eq!(harness.state().focus_adjustments, before);
}

fn pointer(harness: &mut Harness<'_, State>, pos: egui::Pos2, pressed: bool) {
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
}

fn click_at(harness: &mut Harness<'_, State>, pos: egui::Pos2) {
    if harness.state().batched_clicks {
        // Harness::event + step deliberately spreads queued events over frames.
        // Put the native batch directly in RawInput to retain one-pass delivery.
        harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            harness.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.step();
    } else {
        harness.hover_at(pos);
        pointer(harness, pos, true);
        pointer(harness, pos, false);
    }
    harness.run();
}

fn open_motion(harness: &mut Harness<'_, State>) {
    let combo = harness.get_by_role(egui::accesskit::Role::ComboBox).rect();
    click_at(harness, combo.center());
}

fn select_motion(harness: &mut Harness<'_, State>, motion: MotionKind) {
    let row = harness
        .get_by_role_and_label(egui::accesskit::Role::Button, motion.label())
        .rect();
    let panel_id = egui::Id::unique("depth_parallax_panel");
    assert_ne!(
        harness.ctx.layer_id_at(row.center()).map(|layer| layer.id),
        Some(panel_id),
        "the open motion row must own its pixels and pointer input above the panel"
    );
    click_at(harness, row.center());
    assert_eq!(harness.state().settings.motion, motion);
}

#[test]
fn motion_combo_reopens_after_select_and_strength_drag() {
    for size in [egui::vec2(1920.0, 1080.0), egui::vec2(1093.0, 614.0)] {
        for batched_clicks in [true, false] {
            let mut harness = harness(size, batched_clicks);
            harness.run();
            open_motion(&mut harness);
            select_motion(&mut harness, MotionKind::Vertical);

            // A real press/move/release on the rail reproduces the reported 5% drag.
            let rail = harness
                .get_by_role_and_label(egui::accesskit::Role::Slider, "強さ")
                .rect();
            harness.hover_at(rail.center());
            pointer(&mut harness, rail.center(), true);
            harness.event(egui::Event::PointerMoved(rail.right_center()));
            harness.step();
            pointer(&mut harness, rail.right_center(), false);
            harness.run();
            assert!((harness.state().settings.strength - 0.05).abs() < 0.00001);

            open_motion(&mut harness);
            select_motion(&mut harness, MotionKind::Horizontal);

            // Reuse the same egui memory, including hiding and showing the panel.
            harness.state_mut().hidden = true;
            harness.run();
            harness.state_mut().hidden = false;
            harness.run();
            for motion in [
                MotionKind::Vertical,
                MotionKind::Horizontal,
                MotionKind::Mouse,
            ] {
                open_motion(&mut harness);
                select_motion(&mut harness, motion);
            }
            let close = harness
                .get_by_role_and_label(egui::accesskit::Role::Button, "×")
                .rect();
            click_at(&mut harness, close.center());
            assert!(harness.state().close_requested);
        }
    }
}
