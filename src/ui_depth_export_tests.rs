//! Native-style pointer batches and first-frame geometry, without an App or worker.
use super::*;
use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
use egui_kittest::{Harness, kittest::Queryable};

#[derive(Clone, Copy)]
enum TestPhase {
    Settings,
    Progress,
    Finished,
}

struct State {
    phase: Option<TestPhase>,
    options: DepthExportOptions,
    panel: crate::depth_parallax::DepthParallaxSettings,
    panel_export: egui::Rect,
    dialog: Option<DialogResponse>,
    last_action: Option<Action>,
    backing_actions: usize,
}

fn draw_phase(
    ctx: &egui::Context,
    phase: TestPhase,
    options: &mut DepthExportOptions,
    message: &str,
    failed: bool,
    escape: bool,
) -> DialogResponse {
    draw(
        ctx,
        match phase {
            TestPhase::Settings => Phase::Settings {
                options,
                source_long_edge: 4000,
                period_secs: 3.0,
                save_enabled: true,
            },
            TestPhase::Progress => Phase::Progress {
                completed: 1,
                total: 45,
                stopping: false,
            },
            TestPhase::Finished => Phase::Finished { message, failed },
        },
        escape,
    )
}

fn harness(size: egui::Vec2) -> Harness<'static, State> {
    crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(size)
        .build_ui_state(
            move |ui, state: &mut State| {
                let ctx = ui.ctx();
                egui::Area::new(egui::Id::new("export_test_canvas"))
                    .order(egui::Order::Middle)
                    .fixed_pos(egui::Pos2::ZERO)
                    .movable(false)
                    .show(ctx, |ui| {
                        if ui.allocate_response(size, egui::Sense::click()).clicked() {
                            state.backing_actions += 1;
                        }
                    });
                let panel = crate::ui_depth_parallax::draw_depth_parallax_panel_with_export(
                    ctx,
                    &mut state.panel,
                    "",
                    true,
                );
                state.panel_export = panel.export_rect;
                if panel.changed || panel.close_requested || panel.export_requested {
                    state.backing_actions += 1;
                }
                if panel.export_requested && state.phase.is_none() {
                    state.phase = Some(TestPhase::Settings);
                }
                if let Some(phase) = state.phase {
                    let escape =
                        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
                    let response = draw_phase(
                        ctx,
                        phase,
                        &mut state.options,
                        "保存しました: photo_3d.mp4 (8.4 MB)",
                        false,
                        escape,
                    );
                    if let Some(action) = response.action {
                        state.last_action = Some(action);
                        if action != Action::Save {
                            state.phase = None;
                        }
                    }
                    state.dialog = Some(response);
                }
            },
            State {
                phase: None,
                options: DepthExportOptions::default(),
                panel: Default::default(),
                panel_export: egui::Rect::NOTHING,
                dialog: None,
                last_action: None,
                backing_actions: 0,
            },
        )
}

fn click(harness: &mut Harness<'_, State>, pos: egui::Pos2, batched: bool) {
    if batched {
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
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
    }
    harness.run();
}

fn assert_modal_ownership(harness: &Harness<'_, State>) {
    let backdrop = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("depth_export_barrier"),
    );
    let dialog = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("depth_export_dialog"),
    );
    let panel = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("depth_parallax_panel"),
    );
    harness.ctx.memory(|memory| {
        let layers: Vec<_> = memory.layer_ids().collect();
        let at = |layer| {
            layers
                .iter()
                .position(|&candidate| candidate == layer)
                .unwrap()
        };
        assert!(at(panel) < at(backdrop));
        assert_eq!(at(backdrop) + 1, at(dialog));
        assert_eq!(memory.top_modal_layer(), Some(backdrop));
        assert!(!memory.allows_interaction(panel));
        assert!(memory.allows_interaction(dialog));
    });
}

#[test]
fn depth_export_outside_and_panel_clicks_preserve_modal_buttons_drag_and_reopen() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1920.0, 1080.0)] {
        for batched in [false, true] {
            let mut harness = harness(size);
            harness.run();
            // Positive controls: the real backing canvas and export button work.
            click(&mut harness, egui::pos2(30.0, 30.0), batched);
            assert_eq!(harness.state().backing_actions, 1);
            let export = harness.state().panel_export.center();
            click(&mut harness, export, batched);
            assert!(harness.state().phase.is_some());
            assert_eq!(harness.state().backing_actions, 2);
            harness.state_mut().backing_actions = 0;

            for phase in [
                TestPhase::Settings,
                TestPhase::Progress,
                TestPhase::Finished,
            ] {
                for title_close in [false, true] {
                    harness.state_mut().phase = Some(phase);
                    harness.state_mut().last_action = None;
                    harness.run();
                    if matches!(phase, TestPhase::Settings) {
                        harness.get_by_label("GIF").click();
                        harness.run();
                        assert_eq!(harness.state().options.format, DepthExportFormat::Gif);
                    }
                    click(&mut harness, egui::pos2(30.0, 30.0), batched);
                    let export = harness.state().panel_export.center();
                    click(&mut harness, export, batched);
                    assert_eq!(
                        harness.state().backing_actions,
                        0,
                        "canvas and panel must not act through the modal"
                    );
                    assert_modal_ownership(&harness);
                    if matches!(phase, TestPhase::Settings) {
                        harness.get_by_label("APNG").click();
                        harness.run();
                        assert_eq!(harness.state().options.format, DepthExportFormat::Apng);
                        harness.get_by_label("保存先を選ぶ…").click();
                        harness.run();
                        assert_eq!(harness.state().last_action, Some(Action::Save));
                    }
                    let before = harness.state().dialog.as_ref().unwrap().rect;
                    let title = egui::pos2(before.center().x, before.top() + 12.0);
                    harness.hover_at(title);
                    harness.event(egui::Event::PointerButton {
                        pos: title,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    });
                    harness.step();
                    harness.event(egui::Event::PointerMoved(title + egui::vec2(20.0, 10.0)));
                    harness.step();
                    harness.event(egui::Event::PointerButton {
                        pos: title + egui::vec2(20.0, 10.0),
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                    harness.run();
                    assert_ne!(
                        harness.state().dialog.as_ref().unwrap().rect.min,
                        before.min
                    );
                    let label = if title_close {
                        "Close window"
                    } else if matches!(phase, TestPhase::Finished) {
                        "閉じる"
                    } else {
                        "キャンセル"
                    };
                    harness
                        .get_by_role_and_label(egui::accesskit::Role::Button, label)
                        .click();
                    harness.run();
                    assert!(harness.state().phase.is_none());
                    assert_eq!(
                        harness.state().last_action,
                        Some(if matches!(phase, TestPhase::Finished) {
                            Action::Close
                        } else {
                            Action::Cancel
                        })
                    );
                }
            }
            // Reopen after Esc in the same fullscreen Context/layer order.
            harness.state_mut().phase = Some(TestPhase::Settings);
            harness.run();
            click(&mut harness, egui::pos2(30.0, 30.0), batched);
            harness.key_press(egui::Key::Escape);
            harness.run();
            assert!(harness.state().phase.is_none());
            click(&mut harness, export, batched);
            harness.get_by_label("GIF").click();
            harness.run();
            assert_eq!(harness.state().options.format, DepthExportFormat::Gif);
            assert_eq!(harness.state().backing_actions, 1);
            assert_modal_ownership(&harness);
        }
    }
}

#[test]
fn depth_export_first_frame_geometry_is_stable_across_phases_reopen_and_formats() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1920.0, 1080.0)] {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let mut options = DepthExportOptions::default();
        let mut frame = 0;
        // Reuse Resize/Area state: initial settings, all format switches, progress,
        // completion, close, then a fresh settings dialog in the same fullscreen.
        for phase in [
            Some(TestPhase::Settings),
            Some(TestPhase::Progress),
            Some(TestPhase::Finished),
            None,
            Some(TestPhase::Settings),
        ] {
            for format in [
                DepthExportFormat::Mp4,
                DepthExportFormat::Gif,
                DepthExportFormat::Apng,
            ] {
                options.format = format;
                let mut initial = None;
                for pass in 0..8 {
                    let mut measured = None;
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            time: Some(frame as f64 / 60.0),
                            // Idle frames followed by mouse motion over the dialog.
                            events: if pass < 4 {
                                vec![]
                            } else {
                                vec![egui::Event::PointerMoved(size.to_pos2() * 0.5)]
                            },
                            ..Default::default()
                        },
                        |_| {
                            if let Some(phase) = phase {
                                let response = draw_phase(
                                    &ctx,
                                    phase,
                                    &mut options,
                                    "保存しました: photo_3d.mp4 (8.4 MB)",
                                    false,
                                    false,
                                );
                                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                                assert!(screen.contains_rect(response.rect));
                                for action in &response.action_rects {
                                    assert!(response.rect.contains_rect(*action));
                                }
                                if let Some(scroll) = &response.scroll {
                                    ScrollLayout::from_output(scroll).assert_height(
                                        HeightExpectation::Fits,
                                        response.rect,
                                        ctx.pixels_per_point(),
                                    );
                                }
                                measured = Some((response.rect, response.action_rects));
                            }
                        },
                    );
                    output.drop_without_applying_deltas();
                    if pass == 0 {
                        initial = Some(measured);
                    } else {
                        assert_eq!(
                            Some(measured),
                            initial,
                            "frame={frame}, pass={pass}, format={format:?}"
                        );
                    }
                    frame += 1;
                }
            }
        }
    }
}

#[test]
fn depth_export_long_terminal_geometry_is_bounded_and_stable_from_first_frame() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1920.0, 1080.0)] {
        for failed in [false, true] {
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let mut options = DepthExportOptions::default();
            let message = if failed {
                format!(
                    "3D 写真を書き出せませんでした: {}",
                    "長いエラーの詳細。".repeat(80)
                )
            } else {
                format!(
                    "保存しました: C:\\{}photo_3d.mp4 (8.4 MB)",
                    "長いフォルダ名\\".repeat(80)
                )
            };
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let mut initial = None;
            for frame in 0..8 {
                let mut measured = None;
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        time: Some(frame as f64 / 60.0),
                        ..Default::default()
                    },
                    |_| {
                        let response = draw_phase(
                            &ctx,
                            TestPhase::Finished,
                            &mut options,
                            &message,
                            failed,
                            false,
                        );
                        assert!(screen.contains_rect(response.rect));
                        for action in &response.action_rects {
                            assert!(response.rect.contains_rect(*action));
                        }
                        ScrollLayout::from_output(response.scroll.as_ref().unwrap()).assert_height(
                            HeightExpectation::Scrolls,
                            response.rect,
                            ctx.pixels_per_point(),
                        );
                        measured = Some((response.rect, response.action_rects));
                    },
                );
                output.drop_without_applying_deltas();
                if frame == 0 {
                    initial = measured;
                } else {
                    assert_eq!(measured, initial, "failed={failed}, frame={frame}");
                }
            }
        }
    }
}
