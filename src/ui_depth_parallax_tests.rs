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
}

fn harness(size: egui::Vec2, batched_clicks: bool) -> Harness<'static, State> {
    crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(size)
        .build_ui_state(
            |ui, state: &mut State| {
                if !state.hidden {
                    let panel = super::draw_depth_parallax_panel(
                        ui.ctx(),
                        &mut state.settings,
                        "",
                        Some("I / Tab: パネルを隠す・出す".into()),
                    );
                    state.close_requested |= panel.close_requested;
                    state.focus_adjustments += usize::from(panel.focus_adjusting);
                }
            },
            State {
                batched_clicks,
                ..State::default()
            },
        )
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
