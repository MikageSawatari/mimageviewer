//! ラボと本体で共有する音楽ビデオ編集 UI。
//!
//! ウィンドウ、ファイル I/O、音声再生、書き出しは呼び出し側が所有する。

use std::path::PathBuf;

mod keyframes;
mod modulators;
mod timeline;

pub use keyframes::{
    AnimProperty, AnimationValue, CommandResult, EditorCommand, EditorCommandError,
    apply_editor_command,
};
pub use timeline::{TimelineResponse, TimelineWaveform, draw_timeline};

use egui::TextureHandle;
use musicvideo_core::{
    Fill, Glow, GradientStop, GradientStops, ImageId, Layer, LayerId, LayerKind, ModulatorValue,
    MusicColorMode, Project, Rgba8, SpectrumDirection, SpectrumLayer, SpectrumShape,
    new_pitch_keyboard_layer,
};

use modulators::ModulatorEditorState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeySelection {
    layer: LayerId,
    property: AnimProperty,
    key_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorState {
    selected_layer: usize,
    selected_key: Option<KeySelection>,
    dragging_key: Option<KeySelection>,
    modulator_editor: ModulatorEditorState,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            selected_layer: 1,
            selected_key: None,
            dragging_key: None,
            modulator_editor: ModulatorEditorState::default(),
        }
    }
}

impl EditorState {
    pub fn selected_layer(&self) -> usize {
        self.selected_layer
    }

    pub fn select_layer(&mut self, index: usize) {
        self.selected_layer = index;
    }

    pub fn reconcile(&mut self, project: &Project) {
        self.selected_layer = self
            .selected_layer
            .min(project.layers.len().saturating_sub(1));
        let selection_exists = self.selected_key.is_some_and(|selection| {
            project
                .layers
                .iter()
                .find(|layer| layer.id == selection.layer)
                .and_then(|layer| keyframes::animation_keys(layer, selection.property))
                .is_some_and(|keys| selection.key_index < keys.len())
        });
        if !selection_exists {
            self.selected_key = None;
            self.dragging_key = None;
        }
    }

    fn select_layer_id(&mut self, project: &Project, id: LayerId) {
        if let Some(index) = project.layers.iter().position(|layer| layer.id == id) {
            self.selected_layer = index;
        }
    }

    fn apply_command(&mut self, project: &mut Project, command: &EditorCommand) -> bool {
        let (layer, property) = command.target();
        let Ok(result) = apply_editor_command(project, command) else {
            return false;
        };
        if result.clear_selection
            && self
                .selected_key
                .is_some_and(|selected| selected.layer == layer && selected.property == property)
        {
            self.selected_key = None;
            self.dragging_key = None;
        } else if let Some(key_index) = result.selected_key {
            let selection = KeySelection {
                layer,
                property,
                key_index,
            };
            self.selected_key = Some(selection);
            if self
                .dragging_key
                .is_some_and(|dragging| dragging.layer == layer && dragging.property == property)
            {
                self.dragging_key = Some(selection);
            }
        }
        result.changed
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditorResponse {
    pub project_changed: bool,
    pub add_image_layer_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingImageNotice {
    pub id: ImageId,
    pub layer_name: String,
    pub path: Option<PathBuf>,
    pub reason: String,
}

/// Draws the layer list and property editor without owning the project.
pub fn draw_editor(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut EditorState,
    current_time_secs: f64,
    modulator_values: &[ModulatorValue],
) -> EditorResponse {
    state.reconcile(project);
    let mut project_changed = ui
        .add(
            egui::DragValue::new(&mut project.display_gain_db)
                .range(-48.0..=48.0)
                .speed(0.1)
                .prefix("表示ゲイン ")
                .suffix(" dB"),
        )
        .on_hover_text("鍵盤の点灯強度だけを調整します。音声の音量は変わりません")
        .changed();
    project_changed |=
        modulators::draw_modulators(ui, project, &mut state.modulator_editor, modulator_values);
    ui.separator();
    ui.heading("レイヤー");
    for (index, layer) in project.layers.iter().enumerate() {
        if ui
            .selectable_label(state.selected_layer == index, &layer.name)
            .clicked()
        {
            state.selected_layer = index;
            if state
                .selected_key
                .is_some_and(|selected| selected.layer != layer.id)
            {
                state.selected_key = None;
                state.dragging_key = None;
            }
        }
    }

    if ui.button("＋ ピッチ鍵盤を追加").clicked() {
        project.layers.push(new_pitch_keyboard_layer(project));
        state.selected_layer = project.layers.len() - 1;
        state.selected_key = None;
        project_changed = true;
    }
    let add_image_layer_requested = ui.button("＋ 画像レイヤーを追加").clicked();
    ui.separator();
    let mut commands = Vec::new();
    let output = project.output;
    let modulators = project.modulators.clone();
    if let Some(layer) = project.layers.get_mut(state.selected_layer) {
        let result = edit_layer_properties(
            ui,
            layer,
            state.selected_key,
            current_time_secs,
            output.width,
            output.height,
            &modulators,
            modulator_values,
        );
        project_changed |= result.changed;
        commands = result.commands;
    }
    for command in commands {
        project_changed |= state.apply_command(project, &command);
    }
    EditorResponse {
        project_changed,
        add_image_layer_requested,
    }
}

/// Draws the already-composited frame. Texture creation and updates stay with the caller.
pub fn draw_preview(ui: &mut egui::Ui, texture: Option<&TextureHandle>, _state: &mut EditorState) {
    if let Some(texture) = texture {
        let source = texture.size_vec2();
        let available = ui.available_size();
        let scale = (available.x / source.x)
            .min(available.y / source.y)
            .max(0.01);
        let size = source * scale;
        ui.centered_and_justified(|ui| {
            ui.image((texture.id(), size));
        });
    } else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
            ui.label("プレビューを準備しています");
        });
    }
}

/// Shows unresolved image details and reports a requested replacement by image ID.
pub fn draw_missing_images(ui: &mut egui::Ui, notices: &[MissingImageNotice]) -> Option<ImageId> {
    let mut replacement = None;
    for notice in notices {
        let path = notice
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "パス未指定".to_owned());
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!(
                    "未解決素材: レイヤー「{}」 / {path} ({})",
                    notice.layer_name, notice.reason
                ),
            );
            if ui.button("画像を差し替える").clicked() {
                replacement = Some(notice.id.clone());
            }
        });
    }
    replacement
}

struct LayerEditResult {
    changed: bool,
    commands: Vec<EditorCommand>,
}

fn edit_layer_properties(
    ui: &mut egui::Ui,
    layer: &mut Layer,
    selected_key: Option<KeySelection>,
    current_time_secs: f64,
    output_width: u32,
    output_height: u32,
    modulators: &[musicvideo_core::Modulator],
    modulator_values: &[ModulatorValue],
) -> LayerEditResult {
    let mut changed = ui.text_edit_singleline(&mut layer.name).changed();
    let mut commands = Vec::new();
    for property in keyframes::animation_properties(layer) {
        edit_animatable_property(
            ui,
            layer,
            property,
            selected_key,
            current_time_secs,
            output_width,
            output_height,
            &mut commands,
        );
    }
    ui.separator();
    changed |= match &mut layer.kind {
        LayerKind::Solid { .. } | LayerKind::Image { .. } | LayerKind::TimeText(_) => false,
        LayerKind::Spectrum(spectrum) => edit_spectrum(ui, spectrum),
        LayerKind::PitchKeyboard(keyboard) => edit_music_color_mode(ui, &mut keyboard.color_mode),
        LayerKind::ProgressBar(progress) => ui
            .add(
                egui::DragValue::new(&mut progress.border_thickness_px)
                    .range(0.0..=100.0)
                    .suffix(" px 枠"),
            )
            .changed(),
    };
    changed |= modulators::draw_bindings(ui, layer, modulators, modulator_values);
    LayerEditResult { changed, commands }
}

#[allow(clippy::too_many_arguments)]
fn edit_animatable_property(
    ui: &mut egui::Ui,
    layer: &Layer,
    property: AnimProperty,
    selected_key: Option<KeySelection>,
    current_time_secs: f64,
    output_width: u32,
    output_height: u32,
    commands: &mut Vec<EditorCommand>,
) {
    let keys = keyframes::animation_keys(layer, property).unwrap_or_default();
    let selected_index = selected_key
        .filter(|selected| selected.layer == layer.id && selected.property == property)
        .map(|selected| selected.key_index);
    let Some(mut value) =
        keyframes::animation_value(layer, property, current_time_secs, selected_index)
    else {
        return;
    };
    egui::CollapsingHeader::new(property.label())
        .id_salt(("animatable-property", layer.id.0, property))
        .default_open(matches!(
            property,
            AnimProperty::Visible | AnimProperty::Opacity | AnimProperty::Rect
        ))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.weak(if keys.is_empty() {
                    "定数".to_owned()
                } else {
                    format!("トラック ({} キー)", keys.len())
                });
                if ui.button("◆ 現在にキー").clicked() {
                    commands.push(EditorCommand::AddKey {
                        layer: layer.id,
                        property,
                        t: current_time_secs,
                    });
                }
                if !keys.is_empty() && ui.button("定数に戻す").clicked() {
                    commands.push(EditorCommand::CollapseToConst {
                        layer: layer.id,
                        property,
                        t: current_time_secs,
                    });
                }
                if let Some(index) = selected_index {
                    ui.label(format!("選択キー #{}", index + 1));
                }
            });
            if edit_animation_value(ui, property, &mut value, output_width, output_height) {
                commands.push(EditorCommand::SetValue {
                    layer: layer.id,
                    property,
                    t: current_time_secs,
                    key_index: selected_index,
                    value,
                });
            }
        });
}

fn edit_animation_value(
    ui: &mut egui::Ui,
    property: AnimProperty,
    value: &mut AnimationValue,
    output_width: u32,
    output_height: u32,
) -> bool {
    match (property, value) {
        (AnimProperty::Visible, AnimationValue::Bool(value)) => {
            ui.checkbox(value, "表示する").changed()
        }
        (AnimProperty::Opacity, AnimationValue::Float(value)) => ui
            .add(egui::Slider::new(value, 0.0..=1.0).text("不透明度"))
            .changed(),
        (AnimProperty::Rotation, AnimationValue::Float(value)) => ui
            .add(
                egui::DragValue::new(value)
                    .range(-180.0..=180.0)
                    .suffix("°"),
            )
            .changed(),
        (AnimProperty::CornerRadius, AnimationValue::Float(value)) => {
            drag_pixels(ui, "半径", value, 0.0..=1000.0)
        }
        (AnimProperty::Rect, AnimationValue::Rect(rect)) => {
            edit_rect(ui, rect, output_width, output_height)
        }
        (AnimProperty::Glow, AnimationValue::Glow(glow)) => edit_glow(ui, glow),
        (_, AnimationValue::Fill(fill)) => edit_fill(ui, fill),
        _ => false,
    }
}

fn edit_rect(
    ui: &mut egui::Ui,
    rect: &mut musicvideo_core::NormRect,
    width: u32,
    height: u32,
) -> bool {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let mut pixels = [
        rect.x * width,
        rect.y * height,
        rect.width * width,
        rect.height * height,
    ];
    let mut changed = false;
    for (label, value) in ["X", "Y", "幅", "高さ"].into_iter().zip(&mut pixels) {
        changed |= ui
            .add(
                egui::DragValue::new(value)
                    .speed(1.0)
                    .prefix(format!("{label} "))
                    .suffix(" px"),
            )
            .changed();
    }
    if changed {
        *rect = musicvideo_core::NormRect::new(
            pixels[0] / width,
            pixels[1] / height,
            (pixels[2] / width).max(0.0),
            (pixels[3] / height).max(0.0),
        );
    }
    changed
}

fn edit_glow(ui: &mut egui::Ui, glow: &mut Option<Glow>) -> bool {
    let mut enabled = glow.is_some();
    let mut changed = ui.checkbox(&mut enabled, "グロー").changed();
    if enabled && glow.is_none() {
        *glow = Some(Glow {
            color: Rgba8::WHITE,
            radius_px: 12.0,
            spread_px: 0.0,
        });
        changed = true;
    } else if !enabled && glow.is_some() {
        *glow = None;
        changed = true;
    }
    if let Some(glow) = glow {
        changed |= edit_color(ui, "グロー色", &mut glow.color);
        changed |= ui
            .add(
                egui::DragValue::new(&mut glow.radius_px)
                    .range(0.0..=128.0)
                    .suffix(" px 半径"),
            )
            .changed();
        changed |= ui
            .add(
                egui::DragValue::new(&mut glow.spread_px)
                    .range(0.0..=128.0)
                    .suffix(" px 広がり"),
            )
            .changed();
    }
    changed
}

fn edit_color(ui: &mut egui::Ui, label: &str, color: &mut Rgba8) -> bool {
    let mut value = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui.color_edit_button_srgba(&mut value).changed();
    });
    if changed {
        *color = Rgba8::new(value.r(), value.g(), value.b(), value.a());
    }
    changed
}

fn edit_spectrum(ui: &mut egui::Ui, spectrum: &mut SpectrumLayer) -> bool {
    let mut changed = ui
        .add(
            egui::DragValue::new(&mut spectrum.band_count)
                .range(1..=128)
                .prefix("解析バンド "),
        )
        .changed();
    changed |= ui
        .add(
            egui::DragValue::new(&mut spectrum.point_count)
                .range(1..=4096)
                .prefix("描画点 "),
        )
        .changed();
    let mut radial = matches!(spectrum.shape, SpectrumShape::Radial { .. });
    let was_radial = radial;
    ui.horizontal(|ui| {
        ui.radio_value(&mut radial, false, "直線");
        ui.radio_value(&mut radial, true, "円形");
    });
    if radial != was_radial {
        spectrum.shape = if radial {
            SpectrumShape::Radial {
                radius_px: 380.0,
                thickness_px: 4.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            }
        } else {
            SpectrumShape::Bars {
                gap_ratio: 0.3,
                min_height_px: 3.0,
                thickness_px: 12.0,
            }
        };
        changed = true;
    }
    match &mut spectrum.shape {
        SpectrumShape::Bars {
            gap_ratio,
            min_height_px,
            thickness_px,
        } => {
            changed |= ui
                .add(egui::Slider::new(gap_ratio, 0.0..=1.0).text("隙間"))
                .changed();
            changed |= drag_pixels(ui, "最小高さ", min_height_px, 0.0..=1000.0);
            changed |= drag_pixels(ui, "太さ", thickness_px, 0.5..=200.0);
        }
        SpectrumShape::Radial {
            radius_px,
            thickness_px,
            start_angle_deg,
            sweep_deg,
            direction,
        } => {
            changed |= drag_pixels(ui, "半径", radius_px, 0.0..=2000.0);
            changed |= drag_pixels(ui, "太さ", thickness_px, 0.5..=200.0);
            changed |= ui
                .add(egui::DragValue::new(start_angle_deg).suffix("° 開始"))
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(sweep_deg)
                        .range(0.0..=360.0)
                        .suffix("° 範囲"),
                )
                .changed();
            ui.horizontal(|ui| {
                changed |= ui
                    .radio_value(direction, SpectrumDirection::Clockwise, "時計回り")
                    .changed();
                changed |= ui
                    .radio_value(direction, SpectrumDirection::CounterClockwise, "反時計回り")
                    .changed();
            });
        }
    }
    changed |= edit_music_color_mode(ui, &mut spectrum.color_mode);
    changed
}

fn edit_music_color_mode(ui: &mut egui::Ui, mode: &mut MusicColorMode) -> bool {
    let previous = *mode;
    ui.horizontal(|ui| {
        ui.label("配色");
        egui::ComboBox::from_id_salt("music-color-mode")
            .selected_text(match mode {
                MusicColorMode::Custom => "カスタム",
                MusicColorMode::Miv => "mIV 本体と同じ",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(mode, MusicColorMode::Custom, "カスタム");
                ui.selectable_value(mode, MusicColorMode::Miv, "mIV 本体と同じ");
            });
    });
    *mode != previous
}

fn drag_pixels(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.add(
        egui::DragValue::new(value)
            .range(range)
            .prefix(format!("{label} "))
            .suffix(" px"),
    )
    .changed()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FillKind {
    Solid,
    Linear,
    Radial,
    Band,
}

fn edit_fill(ui: &mut egui::Ui, fill: &mut Fill) -> bool {
    let mut kind = match fill {
        Fill::Solid(_) => FillKind::Solid,
        Fill::LinearGradient { .. } => FillKind::Linear,
        Fill::RadialGradient { .. } => FillKind::Radial,
        Fill::BandGradient { .. } => FillKind::Band,
    };
    let previous = kind;
    egui::ComboBox::from_id_salt("fill-kind")
        .selected_text(fill_kind_label(kind))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut kind, FillKind::Solid, "単色");
            ui.selectable_value(&mut kind, FillKind::Linear, "線形グラデーション");
            ui.selectable_value(&mut kind, FillKind::Radial, "放射グラデーション");
            ui.selectable_value(&mut kind, FillKind::Band, "バンドグラデーション");
        });
    let mut changed = kind != previous;
    if changed {
        *fill = default_fill(kind);
    }
    match fill {
        Fill::Solid(color) => changed |= edit_color(ui, "色", color),
        Fill::LinearGradient { stops, angle_deg } => {
            changed |= ui
                .add(egui::DragValue::new(angle_deg).suffix("° 角度"))
                .changed();
            changed |= edit_gradient_stops(ui, stops);
        }
        Fill::RadialGradient {
            stops,
            center,
            radius,
        } => {
            changed |= ui
                .add(egui::Slider::new(&mut center.0, 0.0..=1.0).text("中心 X "))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut center.1, 0.0..=1.0).text("中心 Y "))
                .changed();
            changed |= ui
                .add(egui::Slider::new(radius, 0.01..=2.0).text("半径"))
                .changed();
            changed |= edit_gradient_stops(ui, stops);
        }
        Fill::BandGradient { stops } => changed |= edit_gradient_stops(ui, stops),
    }
    changed
}

fn fill_kind_label(kind: FillKind) -> &'static str {
    match kind {
        FillKind::Solid => "単色",
        FillKind::Linear => "線形",
        FillKind::Radial => "放射",
        FillKind::Band => "バンド",
    }
}

fn default_fill(kind: FillKind) -> Fill {
    match kind {
        FillKind::Solid => Fill::Solid(Rgba8::WHITE),
        FillKind::Linear => Fill::LinearGradient {
            stops: default_gradient_stops(),
            angle_deg: 0.0,
        },
        FillKind::Radial => Fill::RadialGradient {
            stops: default_gradient_stops(),
            center: (0.5, 0.5),
            radius: 0.5,
        },
        FillKind::Band => Fill::BandGradient {
            stops: default_gradient_stops(),
        },
    }
}

fn default_gradient_stops() -> GradientStops {
    GradientStops::new(vec![
        GradientStop {
            position: 0.0,
            color: Rgba8::new(120, 255, 35, 255),
        },
        GradientStop {
            position: 1.0,
            color: Rgba8::new(255, 136, 255, 255),
        },
    ])
    .expect("fixed gradient stops are valid")
}

fn edit_gradient_stops(ui: &mut egui::Ui, stops: &mut GradientStops) -> bool {
    let mut values = stops.to_vec();
    let mut changed = false;
    let mut remove = None;
    let can_remove = values.len() > 1;
    for (index, stop) in values.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut stop.position)
                            .range(0.0..=1.0)
                            .speed(0.01),
                    )
                    .changed();
                changed |= edit_color(ui, "色", &mut stop.color);
                if can_remove && ui.small_button("削除").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        values.remove(index);
        changed = true;
    }
    if ui.small_button("ストップを追加").clicked() {
        values.push(GradientStop {
            position: 1.0,
            color: Rgba8::WHITE,
        });
        changed = true;
    }
    if changed {
        *stops = GradientStops::new(values).expect("UI restricts gradient stop positions");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_core::{Animatable, default_project};

    #[test]
    fn new_keyboard_layer_uses_a_transparent_background_and_unique_id() {
        let project = default_project(10.0);
        let layer = new_pitch_keyboard_layer(&project);
        assert_eq!(layer.id.0, 5);
        let LayerKind::PitchKeyboard(keyboard) = layer.kind else {
            panic!("追加レイヤはピッチ鍵盤である必要があります");
        };
        assert_eq!(
            keyboard.background,
            Animatable::Const(Fill::Solid(Rgba8::new(0, 0, 0, 0)))
        );
    }
}
