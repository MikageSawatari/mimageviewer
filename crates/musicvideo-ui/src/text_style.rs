use egui::Ui;
use musicvideo_core::render_limits::{MAX_TEXT_OUTLINE_PX, MAX_TEXT_SHADOW_SPREAD_PX};
use musicvideo_core::{Rgba8, StrokeStyle, TextColor, TextStyle};

pub(super) fn edit(ui: &mut Ui, style: &mut TextStyle) -> bool {
    let before = style.clone();
    toggle(ui, "縁取り", &mut style.outline);
    if let Some(stroke) = &mut style.outline {
        edit_stroke(ui, stroke);
    }
    ui.label("追加縁取り");
    let mut remove = None;
    for (index, stroke) in style.extra_outlines.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            edit_stroke(ui, stroke);
            if ui.button("縁取りを削除").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        style.extra_outlines.remove(index);
    }
    if ui.button("縁取りを追加").clicked() {
        let width = (style
            .extra_outlines
            .iter()
            .map(|s| s.width_px)
            .chain(style.outline.map(|s| s.width_px))
            .fold(0.0, f32::max)
            + 2.0)
            .min(MAX_TEXT_OUTLINE_PX);
        style.extra_outlines.push(StrokeStyle {
            width_px: width,
            ..Default::default()
        });
    }
    toggle(ui, "ドロップシャドウ", &mut style.shadow);
    if let Some(shadow) = &mut style.shadow {
        color(ui, "影の色", &mut shadow.color);
        offset(ui, &mut shadow.offset);
        pixels(ui, "ぼかし", &mut shadow.blur_px, 0.0..=128.0);
        pixels(
            ui,
            "広がり",
            &mut shadow.spread_px,
            0.0..=MAX_TEXT_SHADOW_SPREAD_PX,
        );
    }
    toggle(ui, "エコー", &mut style.echo);
    if let Some(echo) = &mut style.echo {
        color(ui, "エコーの色", &mut echo.color);
        offset(ui, &mut echo.offset);
        ui.add(
            egui::DragValue::new(&mut echo.count)
                .range(1..=12)
                .suffix(" 枚"),
        );
    }
    toggle(ui, "背景ボックス", &mut style.background);
    if let Some(bg) = &mut style.background {
        color(ui, "背景色", &mut bg.fill);
        pixels(ui, "余白", &mut bg.padding_px, 0.0..=256.0);
        pixels(ui, "背景の角丸", &mut bg.corner_px, 0.0..=256.0);
    }
    pixels(ui, "字間", &mut style.letter_gap, -128.0..=256.0);
    pixels(ui, "行間", &mut style.line_gap, -128.0..=256.0);
    *style != before
}
fn toggle<T: Default>(ui: &mut Ui, label: &str, value: &mut Option<T>) {
    let mut enabled = value.is_some();
    if ui.checkbox(&mut enabled, label).changed() {
        *value = enabled.then(T::default);
    }
}
fn pixels(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    super::drag_pixels(ui, label, value, range);
}
fn color(ui: &mut Ui, label: &str, color: &mut TextColor) {
    let mut c = Rgba8::new(color.r, color.g, color.b, color.a);
    if super::edit_color(ui, label, &mut c) {
        *color = TextColor::new(c.r, c.g, c.b, c.a);
    }
}
fn edit_stroke(ui: &mut Ui, s: &mut StrokeStyle) {
    color(ui, "縁取り色", &mut s.color);
    pixels(ui, "幅", &mut s.width_px, 0.0..=MAX_TEXT_OUTLINE_PX);
}
fn offset(ui: &mut Ui, value: &mut (f32, f32)) {
    pixels(ui, "ずらし X", &mut value.0, -512.0..=512.0);
    pixels(ui, "ずらし Y", &mut value.1, -512.0..=512.0);
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnimProperty, AnimationValue, EditorCommand, EditorState, keyframes};
    use musicvideo_core::*;
    #[test]
    fn t1_style_keys_and_values_round_trip_undo_for_all_three_kinds() {
        let mut p = default_project(10.0);
        p.layers = vec![
            new_text_layer(&p),
            new_time_text_layer(&p),
            new_lyrics_layer(&p),
        ];
        // Constructors on an unchanged project share ids; use distinct ids here.
        for (i, layer) in p.layers.iter_mut().enumerate() {
            layer.id = LayerId(100 + i as u64);
        }
        let mut state = EditorState::default();
        state.reset_for_project(&p);
        for index in 0..3 {
            state.select_layer(&p, index);
            let layer = p.layers[index].id;
            let before = p.clone();
            assert!(
                keyframes::animation_properties(&p.layers[index])
                    .contains(&AnimProperty::TextStyle)
            );
            assert!(state.apply_external_command(
                &mut p,
                &EditorCommand::AddKey {
                    layer,
                    property: AnimProperty::TextStyle,
                    t: 1.0
                }
            ));
            let style = TextStyle {
                outline: Some(Default::default()),
                letter_gap: 4.0,
                ..Default::default()
            };
            assert!(state.apply_external_command(
                &mut p,
                &EditorCommand::SetValue {
                    layer,
                    property: AnimProperty::TextStyle,
                    t: 1.0,
                    key_index: Some(0),
                    value: AnimationValue::TextStyle(style.clone())
                }
            ));
            assert_eq!(
                keyframes::animation_value(&p.layers[index], AnimProperty::TextStyle, 1.0, Some(0)),
                Some(AnimationValue::TextStyle(style))
            );
            let after = p.clone();
            assert!(state.undo(&mut p));
            assert!(state.undo(&mut p));
            assert_eq!(p, before);
            assert!(state.redo(&mut p));
            assert!(state.redo(&mut p));
            assert_eq!(p, after);
            assert!(state.apply_external_command(
                &mut p,
                &EditorCommand::MoveKey {
                    layer,
                    property: AnimProperty::TextStyle,
                    key_index: 0,
                    t: 2.0
                }
            ));
            assert_eq!(
                keyframes::animation_keys(&p.layers[index], AnimProperty::TextStyle).unwrap()[0].t,
                2.0
            );
            assert!(state.apply_external_command(
                &mut p,
                &EditorCommand::SetInterpolation {
                    layer,
                    property: AnimProperty::TextStyle,
                    key_index: 0,
                    interp: Interp::Step
                }
            ));
            assert!(state.apply_external_command(
                &mut p,
                &EditorCommand::DeleteKey {
                    layer,
                    property: AnimProperty::TextStyle,
                    key_index: 0
                }
            ));
            assert!(
                keyframes::animation_keys(&p.layers[index], AnimProperty::TextStyle)
                    .unwrap()
                    .is_empty()
            );
        }
    }
    fn labels(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text.visual_bounding_rect()))
                }
                egui::Shape::Vec(children) => {
                    for s in children {
                        collect(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut result = Vec::new();
        for s in &output.shapes {
            collect(&s.shape, &mut result);
        }
        result
    }
    #[test]
    fn d17_nine_add_outline_respects_the_shared_coverage_limit() {
        let ctx = crate::test_context();
        let mut style = TextStyle {
            outline: Some(StrokeStyle {
                width_px: MAX_TEXT_OUTLINE_PX,
                ..Default::default()
            }),
            ..Default::default()
        };
        let frame = |events, style: &mut TextStyle| {
            crate::headless_paint_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(360.0, 1800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        edit(ui, style);
                    });
                },
            ))
        };
        let output = frame(vec![], &mut style);
        let pos = labels(&output)
            .into_iter()
            .find(|(text, _)| text == "縁取りを追加")
            .unwrap()
            .1
            .center();
        for pressed in [true, false] {
            let _ = frame(
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                &mut style,
            );
        }
        assert_eq!(style.extra_outlines.len(), 1);
        assert_eq!(style.extra_outlines[0].width_px, MAX_TEXT_OUTLINE_PX);
    }
    #[test]
    fn t1_shared_style_ui_headless_layout_and_toggle_add_remove() {
        let ctx = crate::test_context();
        let mut style = TextStyle::default();
        let mut time = 0.0;
        let mut frame = |events: Vec<egui::Event>, style: &mut TextStyle| {
            time += 0.1;
            crate::headless_paint_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(360.0, 1800.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        edit(ui, style);
                    });
                },
            ))
        };
        let mut output = frame(vec![], &mut style);
        // Exercise actual headless pointer routing and controls, not a mirrored model mutation.
        for label in [
            "縁取り",
            "縁取りを追加",
            "縁取りを追加",
            "ドロップシャドウ",
            "エコー",
            "背景ボックス",
            "縁取りを削除",
        ] {
            let pos = labels(&output)
                .into_iter()
                .find(|(text, _)| text == label)
                .unwrap()
                .1
                .center();
            for pressed in [true, false] {
                let _ = frame(
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    &mut style,
                );
            }
            // Let egui's layout settle after structural mutations.
            output = frame(vec![], &mut style);
        }
        assert!(style.outline.is_some());
        assert_eq!(style.extra_outlines.len(), 1);
        assert!(style.shadow.is_some());
        assert!(style.echo.is_some());
        assert!(style.background.is_some());
        let labels = labels(&output);
        for required in ["縁取り色", "影の色", "エコーの色", "背景色"] {
            assert!(labels.iter().any(|(text, _)| text == required));
        }
        assert!(
            labels.iter().all(|(_, rect)| rect.right() <= 360.0),
            "narrow inspector overflows horizontally"
        );
    }
    #[test]
    fn t1_style_property_rejects_nontext_layers_and_wrong_value_type() {
        let mut p = default_project(10.0);
        let layer = p.layers[0].id;
        assert_eq!(
            keyframes::apply_editor_command(
                &mut p,
                &EditorCommand::AddKey {
                    layer,
                    property: AnimProperty::TextStyle,
                    t: 0.0
                }
            ),
            Err(crate::EditorCommandError::PropertyDoesNotBelongToLayer)
        );
        let layer = new_text_layer(&p);
        let id = layer.id;
        p.layers.push(layer);
        assert_eq!(
            keyframes::apply_editor_command(
                &mut p,
                &EditorCommand::SetValue {
                    layer: id,
                    property: AnimProperty::TextStyle,
                    t: 0.0,
                    key_index: None,
                    value: AnimationValue::Float(3.0)
                }
            ),
            Err(crate::EditorCommandError::ValueTypeMismatch)
        );
    }
    #[test]
    fn t1_semitransparent_ui_colors_are_stored_unmultiplied_and_alpha_applies_once() {
        let color = crate::color_from_egui(egui::Color32::from_rgba_premultiplied(128, 64, 0, 128));
        assert_eq!(color, Rgba8::new(255, 128, 0, 128));
        let mut p = default_project(1.0);
        p.output = Output {
            width: 1,
            height: 1,
            fps: 30.0,
        };
        let mut layer = new_solid_layer(&p);
        layer.rect = Animatable::Const(NormRect::FULL);
        let LayerKind::Solid { fill } = &mut layer.kind else {
            panic!()
        };
        *fill = Animatable::Const(Fill::Solid(color));
        p.layers = vec![layer];
        let mut frame = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &evaluate(&p, 0.0, &SpectrumFrame::default()),
            &Assets::default(),
            &mut frame,
        )
        .unwrap();
        assert_eq!(frame.rgba, vec![128, 64, 0, 255]);
    }
}
