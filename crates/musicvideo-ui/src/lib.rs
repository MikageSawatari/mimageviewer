//! ラボと本体で共有する音楽ビデオ編集 UI。
//!
//! ウィンドウ、ファイル I/O、音声再生、書き出しは呼び出し側が所有する。

use std::path::PathBuf;

use egui::TextureHandle;
use musicvideo_core::{
    Animatable, Fill, Glow, GradientStop, GradientStops, ImageId, Layer, LayerKind, Project, Rgba8,
    SpectrumDirection, SpectrumLayer, SpectrumShape, new_pitch_keyboard_layer,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorState {
    selected_layer: usize,
}

impl Default for EditorState {
    fn default() -> Self {
        Self { selected_layer: 1 }
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
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditorResponse {
    pub project_changed: bool,
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
) -> EditorResponse {
    state.reconcile(project);
    ui.heading("レイヤー");
    for (index, layer) in project.layers.iter().enumerate() {
        if ui
            .selectable_label(state.selected_layer == index, &layer.name)
            .clicked()
        {
            state.selected_layer = index;
        }
    }

    let mut project_changed = false;
    if ui.button("＋ ピッチ鍵盤を追加").clicked() {
        project.layers.push(new_pitch_keyboard_layer(project));
        state.selected_layer = project.layers.len() - 1;
        project_changed = true;
    }
    ui.separator();
    if let Some(layer) = project.layers.get_mut(state.selected_layer) {
        project_changed |= edit_layer_properties(ui, layer);
    }
    EditorResponse { project_changed }
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

fn edit_layer_properties(ui: &mut egui::Ui, layer: &mut Layer) -> bool {
    let mut changed = ui.text_edit_singleline(&mut layer.name).changed();
    if let Some(visible) = const_mut(&mut layer.visible) {
        changed |= ui.checkbox(visible, "表示する").changed();
    }
    if let Some(opacity) = const_mut(&mut layer.opacity) {
        changed |= ui
            .add(egui::Slider::new(opacity, 0.0..=1.0).text("不透明度"))
            .changed();
    }
    if let Some(rotation) = const_mut(&mut layer.rotation) {
        changed |= ui
            .add(
                egui::DragValue::new(rotation)
                    .range(-180.0..=180.0)
                    .prefix("回転 ")
                    .suffix("°"),
            )
            .changed();
    }
    if let Some(radius) = const_mut(&mut layer.corner_radius) {
        changed |= ui
            .add(
                egui::DragValue::new(radius)
                    .range(0.0..=1000.0)
                    .suffix(" px 角丸"),
            )
            .changed();
    }
    changed |= edit_glow(ui, &mut layer.glow);
    ui.separator();
    changed |= match &mut layer.kind {
        LayerKind::Solid { fill } => edit_fill(ui, "塗り", fill),
        LayerKind::Image { .. } => false,
        LayerKind::Spectrum(spectrum) => edit_spectrum(ui, spectrum),
        LayerKind::PitchKeyboard(keyboard) => {
            let mut changed = edit_fill(ui, "鍵盤の背景", &mut keyboard.background);
            changed |= edit_fill(ui, "点灯する鍵の塗り", &mut keyboard.fill);
            changed
        }
        LayerKind::ProgressBar(progress) => {
            let mut changed = ui
                .add(
                    egui::DragValue::new(&mut progress.border_thickness_px)
                        .range(0.0..=100.0)
                        .suffix(" px 枠"),
                )
                .changed();
            changed |= edit_fill(ui, "進捗の塗り", &mut progress.fill);
            changed |= edit_fill(ui, "枠の塗り", &mut progress.border_fill);
            changed
        }
        LayerKind::TimeText(text) => edit_fill(ui, "文字の塗り", &mut text.fill),
    };
    changed
}

fn const_mut<T>(value: &mut Animatable<T>) -> Option<&mut T> {
    match value {
        Animatable::Const(value) => Some(value),
        Animatable::Track(_) => None,
    }
}

fn edit_glow(ui: &mut egui::Ui, value: &mut Animatable<Option<Glow>>) -> bool {
    let Some(glow) = const_mut(value) else {
        ui.label("グローのキーフレーム編集は P3 で対応します");
        return false;
    };
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
    changed |= edit_fill(ui, "スペクトラムの塗り", &mut spectrum.fill);
    changed
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

fn edit_fill(ui: &mut egui::Ui, label: &str, value: &mut Animatable<Fill>) -> bool {
    let Some(fill) = const_mut(value) else {
        ui.label(format!("{label}: キーフレーム編集は P3 で対応します"));
        return false;
    };
    ui.collapsing(label, |ui| {
        let mut kind = match fill {
            Fill::Solid(_) => FillKind::Solid,
            Fill::LinearGradient { .. } => FillKind::Linear,
            Fill::RadialGradient { .. } => FillKind::Radial,
            Fill::BandGradient { .. } => FillKind::Band,
        };
        let previous = kind;
        egui::ComboBox::from_id_salt(("fill-kind", label))
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
    })
    .body_returned
    .unwrap_or(false)
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
    use musicvideo_core::default_project;

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
