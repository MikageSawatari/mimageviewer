//! Opt-in CPU-only measurement. No App, renderer, event loop, clicks, or settings saves.
//! Run with the optimized dev-runtime profile; the synthetic clock is not wall time.

use super::*;
use egui::epaint::{ClippedShape, Primitive, Shape};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const WARMUP: usize = 40;
const MEASURED: usize = 240;
const STOPPED: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    Keyboard,
    Commands,
    Preferences,
    Outside,
}

#[derive(Clone, Copy)]
struct ScrollObservation {
    viewport: egui::Rect,
    offset_y: f32,
    content_height: f32,
}

fn footer(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        assert!(!ui.button("  OK  ").clicked());
        assert!(!ui.button("キャンセル").clicked());
    });
}

fn draw(
    ctx: &egui::Context,
    state: &mut PreferencesState,
    scene: Scene,
) -> Option<ScrollObservation> {
    let mut open = true;
    let mut scroll = None;
    if matches!(scene, Scene::Keyboard | Scene::Commands) {
        let safe = ctx.content_rect().shrink2(egui::vec2(24.0, 32.0));
        let size = egui::vec2(900.0, 640.0).min(safe.size());
        egui::Window::new("操作カスタマイズ")
            .drag_area(egui::WindowDrag::Anywhere)
            .id(egui::Id::unique("operation_customize_dialog_v2"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(egui::Rect::from_center_size(safe.center(), size).min)
            .default_size(size)
            .min_size([560.0, 420.0])
            .max_size(safe.size())
            .constrain_to(safe)
            .show(ctx, |ui| {
                draw_operation_customize_tabs(ui, state);
                ui.separator();
                let available = ui.available_size();
                let height = (available.y - 42.0 - 12.0).max(140.0);
                let panel = egui::vec2(available.x, height);
                ui.allocate_ui_with_layout(panel, egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_max_size(panel);
                    ui.spacing_mut().scroll = pref_panel_scroll_style();
                    let output = egui::ScrollArea::vertical()
                        .scroll_source(egui::scroll_area::ScrollSource {
                            drag: egui::scroll_area::DragScroll::Always,
                            ..Default::default()
                        })
                        .id_salt("operation_customize_panel")
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        )
                        .auto_shrink([false, false])
                        .max_height(height)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            draw_operation_customize_page(ui, state, false);
                        });
                    scroll = Some(ScrollObservation {
                        viewport: output.inner_rect,
                        offset_y: output.state.clamped_offset().y,
                        content_height: output.content_size.y,
                    });
                });
                footer(ui);
            });
    } else {
        egui::Window::new("環境設定")
            .drag_area(egui::WindowDrag::Anywhere)
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos([280.0, 150.0])
            .default_size([720.0, 520.0])
            .show(ctx, |ui| {
                let height = (ui.available_height() - 36.0 - 12.0).max(200.0);
                let outer = ui.available_rect_before_wrap();
                let left = egui::Rect::from_min_size(outer.min, egui::vec2(180.0, height));
                let right = egui::Rect::from_min_size(
                    outer.min + egui::vec2(188.0, 0.0),
                    egui::vec2((outer.width() - 188.0).max(100.0), height),
                );
                let mut left_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(left)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                draw_preference_search(&mut left_ui, state);
                let tree_height = left_ui.available_height().max(0.0);
                egui::ScrollArea::vertical()
                    .scroll_source(egui::scroll_area::ScrollSource {
                        drag: egui::scroll_area::DragScroll::Always,
                        ..Default::default()
                    })
                    .id_salt("pref_tree")
                    .max_height(tree_height)
                    .auto_shrink([false, false])
                    .show(&mut left_ui, |ui| {
                        ui.set_min_width(168.0);
                        draw_tree(ui, state);
                    });
                ui.painter().vline(
                    outer.min.x + 183.0,
                    outer.min.y..=outer.min.y + height,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
                let mut right_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(right)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                right_ui.spacing_mut().scroll = pref_panel_scroll_style();
                egui::ScrollArea::vertical()
                    .scroll_source(egui::scroll_area::ScrollSource {
                        drag: egui::scroll_area::DragScroll::Always,
                        ..Default::default()
                    })
                    .id_salt(("pref_panel", state.right_panel_scroll_generation))
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .auto_shrink([false, false])
                    .max_height(height)
                    .show(&mut right_ui, |ui| {
                        ui.set_width(ui.available_width());
                        draw_page(ui, state, false);
                    });
                // Advance the parent just as the production two-child layout does.
                ui.allocate_space(egui::vec2(outer.width(), height));
                footer(ui);
            });
    }
    assert!(open);
    scroll
}

fn text_rect(shapes: &[ClippedShape], label: &str) -> Option<egui::Rect> {
    fn find(shape: &Shape, clip: egui::Rect, label: &str) -> Option<egui::Rect> {
        match shape {
            Shape::Text(text) if text.galley.text() == label => {
                let rect = text.visual_bounding_rect().intersect(clip);
                (rect.is_positive()).then_some(rect)
            }
            Shape::Vec(children) => children.iter().find_map(|s| find(s, clip, label)),
            _ => None,
        }
    }
    shapes
        .iter()
        .find_map(|s| find(&s.shape, s.clip_rect, label))
}

fn raw(frame: usize, pointer: Option<egui::Pos2>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 900.0),
        )),
        time: Some(frame as f64 / 60.0),
        predicted_dt: 1.0 / 60.0,
        events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
        ..Default::default()
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub(super) fn summary(values: impl Iterator<Item = f64>) -> Value {
    let mut values: Vec<_> = values.collect();
    assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    let percentile = |q: f64| {
        let rank = (values.len() - 1) as f64 * q;
        let lower = rank.floor() as usize;
        let upper = rank.ceil() as usize;
        values[lower] + (values[upper] - values[lower]) * rank.fract()
    };
    json!({"median": percentile(0.5), "p90": percentile(0.9),
        "max": values.last().unwrap()})
}

fn sample(
    ctx: &egui::Context,
    state: &mut PreferencesState,
    scene: Scene,
    frame: usize,
    pointer: Option<egui::Pos2>,
    stopped: bool,
    scroll_delta_y: f32,
) -> (Value, Vec<ClippedShape>) {
    let mut input = raw(frame, pointer);
    if scroll_delta_y != 0.0 {
        input.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, scroll_delta_y),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::default(),
        });
    }
    let mut scroll = None;
    let start = Instant::now();
    let mut output = ctx.run_pass(input, |ctx| scroll = draw(ctx, state, scene));
    let run_ms = ms(start.elapsed());
    output.textures_delta.clear(); // CPU-only harness: there is no GPU texture consumer.
    let passes = output.platform_output.num_completed_passes;
    if frame >= WARMUP {
        assert_eq!(passes, 1, "measured frames must have a single egui pass");
    }
    let causes: Vec<_> = ctx
        .repaint_causes()
        .iter()
        .map(|c| {
            json!({
                "file": c.file, "line": c.line, "reason": c.reason.to_string()
            })
        })
        .collect();
    let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
    let shape_count = output.shapes.len();
    let shapes = output.shapes;
    let tessellation_shapes = shapes.clone();
    let start = Instant::now();
    let primitives = ctx.tessellate(tessellation_shapes, output.pixels_per_point);
    let tessellate_ms = ms(start.elapsed());
    let (vertices, indices) = primitives
        .iter()
        .fold((0, 0), |(v, i), p| match &p.primitive {
            Primitive::Mesh(mesh) => (v + mesh.vertices.len(), i + mesh.indices.len()),
            Primitive::Callback(_) => (v, i),
        });
    (
        json!({"frame": frame, "phase": if stopped {"stopped"} else {"moving"},
        "synthetic_time_s": frame as f64 / 60.0, "run_ms": run_ms, "passes": passes,
        "tessellate_ms": tessellate_ms, "shapes": shape_count,
        "primitives": primitives.len(), "vertices": vertices, "indices": indices,
        "repaint_delay_ms": (delay != Duration::MAX).then(|| ms(delay)),
        "zero_repaint_delay": delay.is_zero(), "prev_pass_repaint_causes": causes,
        "accesskit_update": output.platform_output.accesskit_update.is_some(),
        "scroll_delta_y": scroll_delta_y,
        "scroll_offset_y": scroll.map(|s| s.offset_y),
        "scroll_content_height": scroll.map(|s| s.content_height),
        "scroll_viewport": scroll.map(|s| [s.viewport.min.x, s.viewport.min.y, s.viewport.max.x, s.viewport.max.y]),
        "pointer": pointer.map(|p| [p.x,p.y])}),
        shapes,
    )
}

fn state(overrides: bool) -> PreferencesState {
    let mut settings = Settings::default();
    if overrides {
        // Existing valid defaults, explicitly overridden, exercise parsing and override lookup
        // without manufacturing invalid shortcuts or hiding user-facing commands.
        for &action in crate::keymap::KeyAction::all()
            .iter()
            .filter(|a| {
                a.is_user_facing() && !a.default_chords().iter().collect::<Vec<_>>().is_empty()
            })
            .take(20)
        {
            settings
                .keymap
                .set_override_chords(action, action.default_chords().iter().collect());
        }
        assert_eq!(settings.keymap.overrides.len(), 20);
        let parsed = crate::keymap::Keymap::from_settings(&settings.keymap);
        for binding in &settings.keymap.overrides {
            let action = crate::keymap::KeyAction::parse_ini_name(&binding.action).unwrap();
            assert!(parsed.override_chords(action).is_some());
        }
    }
    let mut state = super::tests::preferences_state_for_test(&settings);
    state.operation_tab = OperationCustomizeTab::Keyboard;
    assert!(state.operation_keyboard_context.is_none());
    assert_eq!(state.selected, PreferencesPage::General);
    state
}

fn case(scene: Scene, overridden: bool, accessible: bool) -> Value {
    assert!(scene != Scene::Commands);
    let ctx = egui::Context::default();
    if accessible {
        ctx.enable_accesskit();
    }
    crate::ui_fonts::configure_fonts(&ctx);
    let mut state = state(overridden);
    let initial_settings = serde_json::to_value(&state.settings).unwrap();
    let initial_expanded = state.expanded.clone();
    let mut shapes = Vec::new();
    for frame in 0..WARMUP {
        shapes = sample(&ctx, &mut state, scene, frame, None, false, 0.0).1;
    }
    let labels: Vec<String> = if scene == Scene::Keyboard {
        (13..=24).map(|n| format!("F{n}")).collect()
    } else {
        TREE.iter()
            .map(|category| {
                if category.children.is_empty() {
                    category.label.to_string()
                } else {
                    format!("▶ {}", category.label)
                }
            })
            .collect()
    };
    let interactive_rects = ctx.interactive_rects_last_pass();
    let rects: Vec<_> = labels
        .iter()
        .map(|label| {
            let text = text_rect(&shapes, label)
                .expect("hover target must be painted and clipped in view");
            let rect = interactive_rects
                .iter()
                .copied()
                .filter(|r| r.contains(text.center()))
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .expect("painted target must have an interactive egui widget");
            assert!(rect.height() < 40.0 && rect.width() < 200.0);
            rect.intersect(text.expand(12.0))
        })
        .collect();
    let targets: Vec<_> = labels
        .iter()
        .zip(&rects)
        .map(|(label, r)| {
            json!({
                "label": label, "rect": [r.min.x,r.min.y,r.max.x,r.max.y]
            })
        })
        .collect();
    let mut visited = HashSet::new();
    let mut verified_hover_frames = 0;
    let started = Instant::now();
    let mut samples = Vec::new();
    let mut last_pointer = egui::Pos2::ZERO;
    for index in 0..MEASURED {
        // A continuous two-point/frame sweep traverses actual widget rectangles,
        // including the small layout gaps between keys/rows, then reverses direction.
        let first = rects.first().unwrap().center();
        let last = rects.last().unwrap().center();
        let distance = if scene == Scene::Keyboard {
            last.x - first.x
        } else {
            last.y - first.y
        };
        assert!(distance > 0.0);
        let travel = (index as f32 * 2.0) % (distance * 2.0);
        let offset = if travel <= distance {
            travel
        } else {
            distance * 2.0 - travel
        };
        let pointer = if scene == Scene::Outside {
            egui::pos2(30.0 + (index % 20) as f32, 30.0)
        } else if scene == Scene::Keyboard {
            egui::pos2(first.x + offset, first.y)
        } else {
            // Use the leading column common to all labels while sweeping up/down.
            egui::pos2(
                rects
                    .iter()
                    .map(|r| r.min.x)
                    .fold(f32::NEG_INFINITY, f32::max)
                    + 3.0,
                first.y + offset,
            )
        };
        let which = rects.iter().position(|rect| rect.contains(pointer));
        let (mut data, painted) = sample(
            &ctx,
            &mut state,
            scene,
            WARMUP + index,
            Some(pointer),
            false,
            0.0,
        );
        if scene == Scene::Outside {
            assert!(
                !ctx.is_pointer_over_egui(),
                "baseline pointer must stay outside the preferences Window"
            );
        } else {
            assert!(ctx.is_pointer_over_egui());
            if let Some(which) = which {
                let text = text_rect(&painted, &labels[which]).unwrap();
                assert!(text.expand(12.0).contains(pointer));
                assert!(
                    ctx.interactive_rects_last_pass()
                        .iter()
                        .any(|r| r.contains(pointer)
                            && r.contains(text.center())
                            && r.height() < 40.0)
                );
                visited.insert(which);
                verified_hover_frames += 1;
            }
        }
        data["hover_target"] = json!(if scene == Scene::Outside {
            "outside"
        } else {
            which.map(|i| labels[i].as_str()).unwrap_or("layout_gap")
        });
        samples.push(data);
        last_pointer = pointer;
    }
    let moving_wall_ms = ms(started.elapsed());
    // No new PointerMoved event: retain the last pointer and inspect settling requests.
    for index in 0..STOPPED {
        samples.push(
            sample(
                &ctx,
                &mut state,
                scene,
                WARMUP + MEASURED + index,
                None,
                true,
                0.0,
            )
            .0,
        );
    }
    assert_eq!(ctx.input(|i| i.pointer.hover_pos()), Some(last_pointer));
    assert_eq!(
        initial_settings,
        serde_json::to_value(&state.settings).unwrap()
    );
    assert_eq!(initial_expanded, state.expanded);
    assert_eq!(state.selected, PreferencesPage::General);
    assert_eq!(state.operation_tab, OperationCustomizeTab::Keyboard);
    assert!(state.operation_assignment_editor.is_none());
    assert!(state.command_capture_slot.is_none());
    assert!(state.command_selected.is_none());
    assert!(state.search_query.is_empty());
    if scene != Scene::Outside {
        assert!(
            visited.len() >= 4,
            "sweep must cross several actual keys/tree items"
        );
        assert!(
            verified_hover_frames >= MEASURED / 2,
            "most frames must hover actual widgets"
        );
    }
    let moving = &samples[..MEASURED];
    let number = |field: &str| summary(moving.iter().map(|s| s[field].as_f64().unwrap()));
    let base_name = match (scene, overridden) {
        (Scene::Keyboard, false) => "K_default",
        (Scene::Keyboard, true) => "K_20_overrides",
        (Scene::Preferences, _) => "P_general_tree_hover",
        (Scene::Outside, _) => "B_general_outside",
        (Scene::Commands, _) => unreachable!(),
    };
    json!({"case": if accessible { format!("{base_name}_accesskit_on") } else { base_name.to_owned() },
        "accesskit": accessible,
        "measured_frames": MEASURED, "warmup_frames": WARMUP, "stopped_frames": STOPPED,
        "hover_target_geometry": targets, "verified_hover_frames": verified_hover_frames,
        "distinct_hovered_targets": visited.len(),
        "cause_note": "prev_pass_repaint_causes is egui's previous-pass list; repaint_delay_ms belongs to this frame",
        "synthetic_moving_duration_s": MEASURED as f64 / 60.0, "moving_wall_ms": moving_wall_ms,
        "run_ms": number("run_ms"), "tessellate_ms": number("tessellate_ms"),
        "shapes": number("shapes"), "primitives": number("primitives"),
        "vertices": number("vertices"), "indices": number("indices"),
        "zero_repaint_delay_ratio": moving.iter().filter(|s| s["zero_repaint_delay"] == true).count() as f64 / MEASURED as f64,
        "samples": samples})
}

fn command_edit_rects(ctx: &egui::Context, shapes: &[ClippedShape]) -> Vec<egui::Rect> {
    fn collect(shape: &Shape, clip: egui::Rect, texts: &mut Vec<egui::Rect>) {
        match shape {
            Shape::Text(text) if text.galley.text() == "編集" => {
                let rect = text.visual_bounding_rect().intersect(clip);
                if rect.is_positive() {
                    texts.push(rect);
                }
            }
            Shape::Vec(children) => {
                for child in children {
                    collect(child, clip, texts);
                }
            }
            _ => {}
        }
    }
    let mut texts = Vec::new();
    for shape in shapes {
        collect(&shape.shape, shape.clip_rect, &mut texts);
    }
    let interactive = ctx.interactive_rects_last_pass();
    let mut rects: Vec<_> = texts
        .into_iter()
        .filter_map(|text| {
            interactive
                .iter()
                .copied()
                .filter(|r| r.contains(text.center()) && r.width() < 80.0 && r.height() < 40.0)
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .map(|r| r.intersect(text.expand(12.0)))
        })
        .collect();
    rects.sort_by(|a, b| a.center().y.total_cmp(&b.center().y));
    assert!(
        rects.len() >= 4,
        "Commands must show several actual edit buttons"
    );
    rects
}

fn commands_case(overridden: bool, accessible: bool, scrolling: bool) -> Value {
    let ctx = egui::Context::default();
    if accessible {
        ctx.enable_accesskit();
    }
    crate::ui_fonts::configure_fonts(&ctx);
    let mut state = state(overridden);
    state.operation_tab = OperationCustomizeTab::Commands;
    let initial_settings = serde_json::to_value(&state.settings).unwrap();
    let initial_expanded = state.expanded.clone();
    let mut shapes = Vec::new();
    let mut last = Value::Null;
    for frame in 0..WARMUP {
        (last, shapes) = sample(&ctx, &mut state, Scene::Commands, frame, None, false, 0.0);
    }
    let rects = command_edit_rects(&ctx, &shapes);
    let viewport = &last["scroll_viewport"];
    let viewport = egui::Rect::from_min_max(
        egui::pos2(
            viewport[0].as_f64().unwrap() as f32,
            viewport[1].as_f64().unwrap() as f32,
        ),
        egui::pos2(
            viewport[2].as_f64().unwrap() as f32,
            viewport[3].as_f64().unwrap() as f32,
        ),
    );
    let initial_offset = last["scroll_offset_y"].as_f64().unwrap();
    assert_eq!(initial_offset, 0.0);
    assert!(last["scroll_content_height"].as_f64().unwrap() > viewport.height() as f64 + 480.0);
    let targets: Vec<_> = rects.iter().enumerate().map(|(index, r)| {
        json!({"label": format!("visible_edit_{index}"), "rect": [r.min.x,r.min.y,r.max.x,r.max.y]})
    }).collect();
    let mut visited = HashSet::new();
    let mut verified_hover_frames = 0;
    let mut scrolled_frames = 0;
    let mut previous_offset = initial_offset;
    let mut last_pointer = egui::Pos2::ZERO;
    let mut samples = Vec::new();
    let started = Instant::now();
    for index in 0..MEASURED {
        let first = rects.first().unwrap().center();
        let last = rects.last().unwrap().center();
        let distance = last.y - first.y;
        assert!(distance > 0.0);
        let travel = (index as f32 * 2.0) % (distance * 2.0);
        let offset = travel.min(distance * 2.0 - travel);
        let pointer = if scrolling {
            // Keep the pointer over the content while the wheel moves the real ScrollArea.
            egui::pos2(first.x, viewport.center().y)
        } else {
            egui::pos2(first.x, first.y + offset)
        };
        assert!(viewport.contains(pointer));
        let (mut data, painted) = sample(
            &ctx,
            &mut state,
            Scene::Commands,
            WARMUP + index,
            Some(pointer),
            false,
            if scrolling { -2.0 } else { 0.0 },
        );
        assert_eq!(data["accesskit_update"], accessible);
        assert!(ctx.is_pointer_over_egui());
        let current_rects = command_edit_rects(&ctx, &painted);
        let which = current_rects.iter().position(|r| r.contains(pointer));
        if let Some(which) = which {
            verified_hover_frames += 1;
            if !scrolling {
                visited.insert(which);
            }
        }
        let current_offset = data["scroll_offset_y"].as_f64().unwrap();
        if scrolling {
            assert!(
                current_offset > previous_offset,
                "every scrolling frame must move the ScrollArea"
            );
            assert!(
                current_offset - previous_offset < 8.0,
                "scrolling must move only a few points per frame"
            );
            scrolled_frames += 1;
        } else {
            assert_eq!(
                current_offset, initial_offset,
                "hover must not move the ScrollArea"
            );
        }
        data["hover_target"] = json!(
            which
                .map(|i| format!("visible_edit_{i}"))
                .unwrap_or_else(|| "layout_gap".to_owned())
        );
        samples.push(data);
        previous_offset = current_offset;
        last_pointer = pointer;
    }
    let moving_wall_ms = ms(started.elapsed());
    for index in 0..STOPPED {
        samples.push(
            sample(
                &ctx,
                &mut state,
                Scene::Commands,
                WARMUP + MEASURED + index,
                None,
                true,
                0.0,
            )
            .0,
        );
    }
    assert_eq!(ctx.input(|i| i.pointer.hover_pos()), Some(last_pointer));
    assert_eq!(
        initial_settings,
        serde_json::to_value(&state.settings).unwrap()
    );
    assert_eq!(initial_expanded, state.expanded);
    assert_eq!(state.selected, PreferencesPage::General);
    assert_eq!(state.operation_tab, OperationCustomizeTab::Commands);
    assert!(state.operation_keyboard_context.is_none());
    assert!(state.operation_assignment_editor.is_none());
    assert!(state.command_selected.is_none());
    assert!(state.command_edit_loaded_for.is_none());
    assert!(state.command_capture_slot.is_none());
    assert!(state.command_editor_source_chord.is_none());
    assert!(state.command_chord_inputs.iter().all(String::is_empty));
    assert!(state.command_edit_error.is_none() && state.command_edit_notice.is_none());
    assert!(state.command_filter.is_empty() && state.command_key_filter.is_empty());
    assert!(state.search_query.is_empty());
    if scrolling {
        assert_eq!(scrolled_frames, MEASURED);
        assert!(previous_offset - initial_offset > 400.0);
    } else {
        assert!(
            visited.len() >= 4,
            "hover sweep must cross several actual edit buttons"
        );
        assert!(verified_hover_frames >= MEASURED / 2);
    }
    let moving = &samples[..MEASURED];
    let number = |field: &str| summary(moving.iter().map(|s| s[field].as_f64().unwrap()));
    json!({"case": format!("C_{}_accesskit_{}_{}", if overridden {"20_overrides"} else {"default"},
            if accessible {"on"} else {"off"}, if scrolling {"scroll"} else {"hover"}),
        "accesskit": accessible, "operation_context": "all", "input": if scrolling {"wheel"} else {"pointer_move"},
        "measured_frames": MEASURED, "warmup_frames": WARMUP, "stopped_frames": STOPPED,
        "hover_target_geometry": targets, "verified_hover_frames": verified_hover_frames,
        "distinct_hovered_targets": if scrolling {Value::Null} else {json!(visited.len())},
        "scrolled_frames": scrolled_frames, "initial_scroll_offset_y": initial_offset,
        "final_measured_scroll_offset_y": previous_offset,
        "cause_note": "prev_pass_repaint_causes is egui's previous-pass list; repaint_delay_ms belongs to this frame",
        "synthetic_moving_duration_s": MEASURED as f64 / 60.0, "moving_wall_ms": moving_wall_ms,
        "run_ms": number("run_ms"), "tessellate_ms": number("tessellate_ms"),
        "shapes": number("shapes"), "primitives": number("primitives"),
        "vertices": number("vertices"), "indices": number("indices"),
        "zero_repaint_delay_ratio": moving.iter().filter(|s| s["zero_repaint_delay"] == true).count() as f64 / MEASURED as f64,
        "samples": samples})
}

#[test]
#[ignore = "opt-in optimized headless hover CPU measurement; no application launch"]
fn headless_preferences_hover_measurement() {
    let guard = crate::settings_db::DataDirOverrideGuard::new();
    let mut cases = Vec::new();
    for (scene, overrides) in [
        (Scene::Keyboard, false),
        (Scene::Keyboard, true),
        (Scene::Preferences, false),
        (Scene::Outside, false),
    ] {
        let result = case(scene, overrides, false);
        println!("HOVER_PERF {}", serde_json::to_string(&result).unwrap());
        cases.push(result);
    }
    for overrides in [false, true] {
        let result = case(Scene::Keyboard, overrides, true);
        println!("HOVER_PERF {}", serde_json::to_string(&result).unwrap());
        cases.push(result);
    }
    for overrides in [false, true] {
        for accessible in [false, true] {
            for scrolling in [false, true] {
                let result = commands_case(overrides, accessible, scrolling);
                println!("HOVER_PERF {}", serde_json::to_string(&result).unwrap());
                cases.push(result);
            }
        }
    }
    for overrides in [false, true] {
        let result = pages::hover_measurement::breakdown(&mut state(overrides), WARMUP, MEASURED);
        println!("HOVER_PERF {}", serde_json::to_string(&result).unwrap());
        cases.push(result);
    }
    for commands in [false, true] {
        for overrides in [false, true] {
            for accessible in [false, true] {
                let result = pages::hover_measurement::phase_breakdown(
                    &mut state(overrides),
                    WARMUP,
                    MEASURED,
                    commands,
                    accessible,
                );
                println!("HOVER_PERF {}", serde_json::to_string(&result).unwrap());
                cases.push(result);
            }
        }
    }
    assert!(
        !guard.path().join("settings.db").exists(),
        "measurement must never save settings"
    );
    if let Some(relative) = std::env::var_os("MIV_HOVER_PERF_JSON") {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let relative = PathBuf::from(relative);
        assert!(!relative.is_absolute());
        assert!(
            relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        );
        let path = root.join(relative);
        let parent = path
            .parent()
            .unwrap()
            .canonicalize()
            .expect("artifact parent must already exist");
        assert!(
            parent.starts_with(&root),
            "artifact must remain inside this repository"
        );
        assert!(!path.exists(), "refuse to overwrite an existing artifact");
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"cases": cases})).unwrap(),
        )
        .unwrap();
    }
}
