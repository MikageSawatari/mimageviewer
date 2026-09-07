use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use musicvideo_core::{Interp, LayerId, Project};

use crate::history::EditSurface;
use crate::keyframes::{
    AnimProperty, EditorCommand, KeyMeta, animation_keys, animation_properties,
};
use crate::{DetailSelection, EditorState};

const LABEL_WIDTH: f32 = 190.0;
const WAVEFORM_HEIGHT: f32 = 72.0;
const ROW_HEIGHT: f32 = 24.0;

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineWaveform {
    pub duration_secs: f64,
    pub stereo_peaks: Vec<[f32; 2]>,
}

impl TimelineWaveform {
    pub fn new(duration_secs: f64, stereo_peaks: Vec<[f32; 2]>) -> Self {
        Self {
            duration_secs: if duration_secs.is_finite() {
                duration_secs.max(0.0)
            } else {
                0.0
            },
            stereo_peaks,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TimelineResponse {
    pub project_changed: bool,
    pub seek_to: Option<f64>,
}

struct RowSnapshot {
    layer: LayerId,
    layer_name: String,
    property: AnimProperty,
    keys: Vec<KeyMeta>,
}

struct LyricRowSnapshot {
    layer: LayerId,
    layer_name: String,
    markers: Vec<(usize, f64, String)>,
}

pub fn draw_timeline(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut EditorState,
    position_secs: f64,
    waveform: Option<&TimelineWaveform>,
) -> TimelineResponse {
    state.reconcile(project);
    let checkpoint = state.begin_external_edit(project);
    let context = ui.ctx().clone();
    let duration = project.duration_secs.max(0.001);
    let mut response = TimelineResponse::default();
    ui.horizontal(|ui| {
        ui.strong("タイムライン");
        ui.label(format!("{position_secs:.3} 秒"));
        ui.weak("波形または行をドラッグしてシーク / ◆をドラッグしてキー・歌詞を移動");
    });

    draw_waveform_lane(ui, duration, position_secs, waveform, &mut response);

    let rows = tracked_rows(project);
    let lyric_rows = lyric_rows(project);

    if rows.is_empty() && lyric_rows.is_empty() {
        ui.weak("キーのあるプロパティと歌詞レイヤーが、ここに表示されます");
    }

    egui::ScrollArea::vertical()
        .id_salt("musicvideo-keyframe-rows")
        .max_height(190.0)
        .show(ui, |ui| {
            for row in rows {
                draw_property_row(
                    ui,
                    project,
                    state,
                    duration,
                    position_secs,
                    row,
                    &mut response,
                );
            }
            for row in lyric_rows {
                draw_lyric_row(
                    ui,
                    project,
                    state,
                    duration,
                    position_secs,
                    row,
                    &mut response,
                );
            }
        });
    draw_key_inspector(ui, project, state, duration, &mut response);
    let pointer_active = context.input(|input| input.pointer.primary_down());
    let text_focus = context.memory(|memory| memory.focused());
    state.observe_ui_pass(
        EditSurface::Timeline,
        project,
        checkpoint,
        response.project_changed,
        pointer_active,
        text_focus,
    );
    response
}

fn lyric_rows(project: &Project) -> Vec<LyricRowSnapshot> {
    project
        .layers
        .iter()
        .filter_map(|layer| {
            let musicvideo_core::LayerKind::Lyrics(lyrics) = &layer.kind else {
                return None;
            };
            Some(LyricRowSnapshot {
                layer: layer.id,
                layer_name: layer.name.clone(),
                markers: lyrics
                    .lines
                    .iter()
                    .enumerate()
                    .filter_map(|(index, line)| {
                        line.time_secs
                            .filter(|time| time.is_finite())
                            .map(|time| (index, time, line.text.clone()))
                    })
                    .collect(),
            })
        })
        .collect()
}

fn tracked_rows(project: &Project) -> Vec<RowSnapshot> {
    project
        .layers
        .iter()
        .flat_map(|layer| {
            animation_properties(layer)
                .into_iter()
                .filter_map(|property| {
                    let keys = animation_keys(layer, property).unwrap_or_default();
                    (!keys.is_empty()).then(|| RowSnapshot {
                        layer: layer.id,
                        layer_name: layer.name.clone(),
                        property,
                        keys,
                    })
                })
        })
        .collect()
}

fn draw_waveform_lane(
    ui: &mut egui::Ui,
    duration: f64,
    position_secs: f64,
    waveform: Option<&TimelineWaveform>,
    output: &mut TimelineResponse,
) {
    let width = ui.available_width().max(LABEL_WIDTH + 80.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, WAVEFORM_HEIGHT), Sense::click_and_drag());
    let axis = timeline_axis(rect);
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, Color32::from_gray(24));
    painter.text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "音声波形",
        egui::FontId::proportional(12.0),
        Color32::LIGHT_GRAY,
    );
    painter.rect_filled(axis, 0.0, Color32::from_gray(16));
    draw_time_ticks(painter, axis, duration);
    if let Some(waveform) = waveform {
        draw_waveform(
            painter,
            axis.shrink2(Vec2::new(0.0, 6.0)),
            duration,
            waveform,
        );
    } else {
        painter.text(
            axis.center(),
            egui::Align2::CENTER_CENTER,
            "音声を開くと波形を表示します",
            egui::FontId::proportional(12.0),
            Color32::GRAY,
        );
    }
    draw_playhead(painter, axis, duration, position_secs);
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
        && axis.contains(pointer)
    {
        output.seek_to = Some(x_to_time(axis, duration, pointer.x));
    }
}

fn draw_waveform(
    painter: &egui::Painter,
    rect: Rect,
    project_duration: f64,
    waveform: &TimelineWaveform,
) {
    if waveform.stereo_peaks.is_empty() || waveform.duration_secs <= 0.0 || rect.width() <= 0.0 {
        return;
    }
    let pixels = rect.width().ceil().max(1.0) as usize;
    let center = rect.center().y;
    let half_height = rect.height() * 0.48;
    for pixel in 0..pixels {
        let x0 = rect.min.x + pixel as f32;
        let t0 = pixel as f64 / pixels as f64 * project_duration;
        let t1 = (pixel + 1) as f64 / pixels as f64 * project_duration;
        let first = ((t0 / waveform.duration_secs) * waveform.stereo_peaks.len() as f64)
            .floor()
            .max(0.0) as usize;
        let end = ((t1 / waveform.duration_secs) * waveform.stereo_peaks.len() as f64)
            .ceil()
            .max(first as f64 + 1.0) as usize;
        if first >= waveform.stereo_peaks.len() {
            continue;
        }
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for peak in &waveform.stereo_peaks[first..end.min(waveform.stereo_peaks.len())] {
            left = left.max(peak[0].clamp(0.0, 1.0));
            right = right.max(peak[1].clamp(0.0, 1.0));
        }
        painter.line_segment(
            [
                Pos2::new(x0, center - left * half_height),
                Pos2::new(x0, center + right * half_height),
            ],
            Stroke::new(1.0, Color32::from_rgb(72, 188, 220)),
        );
    }
}

fn draw_property_row(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut EditorState,
    duration: f64,
    position_secs: f64,
    row: RowSnapshot,
    output: &mut TimelineResponse,
) {
    let width = ui.available_width().max(LABEL_WIDTH + 80.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click_and_drag());
    let axis = timeline_axis(rect);
    let selected = project
        .layers
        .get(state.selected_layer(project))
        .is_some_and(|layer| {
            layer.id == row.layer
                && state
                    .selected_key()
                    .is_some_and(|(property, _)| property == row.property)
        });
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        0.0,
        if selected {
            Color32::from_gray(42)
        } else {
            Color32::from_gray(29)
        },
    );
    painter.text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{} / {}", row.layer_name, row.property.label()),
        egui::FontId::proportional(11.0),
        Color32::LIGHT_GRAY,
    );
    painter.rect_filled(axis, 0.0, Color32::from_gray(20));
    draw_playhead(painter, axis, duration, position_secs);

    let pointer = response.interact_pointer_pos();
    let hovered_key = pointer.and_then(|pointer| {
        row.keys.iter().enumerate().rev().find_map(|(index, key)| {
            let x = time_to_x(axis, duration, key.t);
            ((pointer.x - x).abs() <= 7.0).then_some(index)
        })
    });
    for (index, key) in row.keys.iter().enumerate() {
        let center = Pos2::new(time_to_x(axis, duration, key.t), rect.center().y);
        let is_selected = project
            .layers
            .get(state.selected_layer(project))
            .is_some_and(|layer| {
                layer.id == row.layer
                    && state.selected_detail
                        == Some(DetailSelection::Key {
                            property: row.property,
                            key_index: index,
                        })
            });
        let radius = if is_selected { 6.0 } else { 4.5 };
        painter.add(egui::Shape::convex_polygon(
            vec![
                Pos2::new(center.x, center.y - radius),
                Pos2::new(center.x + radius, center.y),
                Pos2::new(center.x, center.y + radius),
                Pos2::new(center.x - radius, center.y),
            ],
            if is_selected {
                Color32::YELLOW
            } else {
                Color32::from_rgb(105, 210, 255)
            },
            Stroke::NONE,
        ));
    }

    if response.drag_started() {
        if let Some(index) = hovered_key {
            let selection = DetailSelection::Key {
                property: row.property,
                key_index: index,
            };
            state.select_key(project, row.layer, row.property, index);
            state.dragging_detail = Some(selection);
        } else {
            state.dragging_detail = None;
        }
    }
    if response.clicked() {
        if let Some(index) = hovered_key {
            state.select_key(project, row.layer, row.property, index);
            output.seek_to = Some(row.keys[index].t);
        } else if let Some(pointer) = pointer.filter(|pointer| axis.contains(*pointer)) {
            output.seek_to = Some(x_to_time(axis, duration, pointer.x));
        }
    }
    if response.dragged()
        && let Some(pointer) = pointer
    {
        if let Some(DetailSelection::Key {
            property,
            key_index,
        }) = state.dragging_detail
            && project
                .layers
                .get(state.selected_layer(project))
                .map(|layer| layer.id)
                == Some(row.layer)
            && property == row.property
        {
            let t = x_to_time(axis, duration, pointer.x);
            let command = EditorCommand::MoveKey {
                layer: row.layer,
                property: row.property,
                key_index,
                t,
            };
            output.project_changed |= state.apply_command(project, &command);
            output.seek_to = Some(t);
        } else if axis.contains(pointer) {
            output.seek_to = Some(x_to_time(axis, duration, pointer.x));
        }
    }
    if response.drag_stopped() {
        state.dragging_detail = None;
    }
}

fn draw_lyric_row(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut EditorState,
    duration: f64,
    position_secs: f64,
    row: LyricRowSnapshot,
    output: &mut TimelineResponse,
) {
    let width = ui.available_width().max(LABEL_WIDTH + 80.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click_and_drag());
    let axis = timeline_axis(rect);
    let selected = project
        .layers
        .get(state.selected_layer(project))
        .is_some_and(|layer| layer.id == row.layer && state.selected_lyric().is_some());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        0.0,
        if selected {
            Color32::from_gray(42)
        } else {
            Color32::from_gray(29)
        },
    );
    painter.text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{} / 歌詞", row.layer_name),
        egui::FontId::proportional(11.0),
        Color32::LIGHT_GRAY,
    );
    painter.rect_filled(axis, 0.0, Color32::from_gray(20));
    draw_playhead(painter, axis, duration, position_secs);

    let pointer = response.interact_pointer_pos();
    let hovered = pointer.and_then(|pointer| {
        row.markers
            .iter()
            .rev()
            .find(|(_, time, _)| (pointer.x - time_to_x(axis, duration, *time)).abs() <= 7.0)
    });
    for (line_index, time, text) in &row.markers {
        let center = Pos2::new(time_to_x(axis, duration, *time), rect.center().y);
        let is_selected = project
            .layers
            .get(state.selected_layer(project))
            .is_some_and(|layer| {
                layer.id == row.layer && state.selected_lyric() == Some(*line_index)
            });
        let radius = if is_selected { 6.0 } else { 4.5 };
        painter.add(egui::Shape::convex_polygon(
            vec![
                Pos2::new(center.x, center.y - radius),
                Pos2::new(center.x + radius, center.y),
                Pos2::new(center.x, center.y + radius),
                Pos2::new(center.x - radius, center.y),
            ],
            if is_selected {
                Color32::YELLOW
            } else if text.is_empty() {
                Color32::from_rgb(180, 125, 215)
            } else {
                Color32::from_rgb(255, 155, 105)
            },
            Stroke::NONE,
        ));
    }

    if response.drag_started() {
        if let Some((line_index, _, _)) = hovered {
            let selection = DetailSelection::Lyric {
                line_index: *line_index,
            };
            state.select_lyric(project, row.layer, *line_index);
            state.dragging_detail = Some(selection);
        } else {
            state.dragging_detail = None;
        }
    }
    if response.clicked() {
        if let Some((line_index, time, _)) = hovered {
            state.select_lyric(project, row.layer, *line_index);
            output.seek_to = Some(*time);
        } else if let Some(pointer) = pointer.filter(|pointer| axis.contains(*pointer)) {
            output.seek_to = Some(x_to_time(axis, duration, pointer.x));
        }
    }
    if response.dragged()
        && let Some(pointer) = pointer
    {
        if let Some(DetailSelection::Lyric { line_index }) = state.dragging_detail
            && project
                .layers
                .get(state.selected_layer(project))
                .map(|layer| layer.id)
                == Some(row.layer)
        {
            let time = x_to_time(axis, duration, pointer.x);
            output.project_changed |= state.apply_command(
                project,
                &EditorCommand::SetLyricTime {
                    layer: row.layer,
                    line_index,
                    time_secs: Some(time),
                },
            );
            output.seek_to = Some(time);
        } else if axis.contains(pointer) {
            output.seek_to = Some(x_to_time(axis, duration, pointer.x));
        }
    }
    if response.drag_stopped() {
        state.dragging_detail = None;
    }
}

fn draw_key_inspector(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut EditorState,
    duration: f64,
    output: &mut TimelineResponse,
) {
    if let Some(line_index) = state.selected_lyric() {
        draw_lyric_inspector(ui, project, state, line_index, duration, output);
        return;
    }
    let Some((property, key_index)) = state.selected_key() else {
        ui.small("◆を選ぶと、キーまたは歌詞の時刻を編集できます");
        return;
    };
    let Some(layer) = project
        .layers
        .get(state.selected_layer(project))
        .map(|layer| layer.id)
    else {
        state.clear_detail_selection();
        return;
    };
    let Some((layer_name, key)) = project
        .layers
        .iter()
        .find(|candidate| candidate.id == layer)
        .and_then(|layer| {
            animation_keys(layer, property)
                .and_then(|keys| keys.get(key_index).copied())
                .map(|key| (layer.name.clone(), key))
        })
    else {
        state.clear_detail_selection();
        return;
    };

    let mut command = None;
    ui.horizontal(|ui| {
        ui.label(format!("選択: {layer_name} / {}", property.label()));
        let mut time = key.t;
        if ui
            .add(
                egui::DragValue::new(&mut time)
                    .range(0.0..=duration)
                    .speed(0.01)
                    .suffix(" 秒"),
            )
            .changed()
        {
            command = Some(EditorCommand::MoveKey {
                layer,
                property,
                key_index,
                t: time,
            });
            output.seek_to = Some(time);
        }
        if property.is_continuous() {
            let mut interp = key.interp;
            egui::ComboBox::from_id_salt(("key-interp", layer.0, property))
                .selected_text(interp_label(interp))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut interp, Interp::Step, "ステップ");
                    ui.selectable_value(&mut interp, Interp::Linear, "線形");
                    ui.selectable_value(&mut interp, Interp::EaseIn, "イーズイン");
                    ui.selectable_value(&mut interp, Interp::EaseOut, "イーズアウト");
                    ui.selectable_value(&mut interp, Interp::EaseInOut, "イーズイン・アウト");
                });
            if interp != key.interp {
                command = Some(EditorCommand::SetInterpolation {
                    layer,
                    property,
                    key_index,
                    interp,
                });
            }
        } else {
            ui.label("補間: ステップ");
        }
        if ui.button("キーを削除").clicked() {
            command = Some(EditorCommand::DeleteKey {
                layer,
                property,
                key_index,
            });
        }
    });
    if let Some(command) = command {
        output.project_changed |= state.apply_command(project, &command);
    }
}

fn draw_lyric_inspector(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut EditorState,
    line_index: usize,
    duration: f64,
    output: &mut TimelineResponse,
) {
    let Some((layer_id, layer_name, text, original_time)) = project
        .layers
        .get(state.selected_layer(project))
        .and_then(|layer| match &layer.kind {
            musicvideo_core::LayerKind::Lyrics(lyrics) => {
                lyrics.lines.get(line_index).and_then(|line| {
                    line.time_secs
                        .map(|time| (layer.id, layer.name.clone(), line.text.clone(), time))
                })
            }
            _ => None,
        })
    else {
        state.clear_detail_selection();
        return;
    };
    let mut command = None;
    ui.horizontal(|ui| {
        let label = if text.is_empty() {
            "（空行）"
        } else {
            &text
        };
        ui.label(format!(
            "選択: {layer_name} / {} 行目 / {label}",
            line_index + 1
        ));
        let mut time = original_time;
        if ui
            .add(
                egui::DragValue::new(&mut time)
                    .range(0.0..=duration)
                    .speed(0.01)
                    .suffix(" 秒"),
            )
            .changed()
        {
            command = Some(EditorCommand::SetLyricTime {
                layer: layer_id,
                line_index,
                time_secs: Some(time),
            });
            output.seek_to = Some(time);
        }
        if ui.button("時刻を未設定に戻す").clicked() {
            command = Some(EditorCommand::SetLyricTime {
                layer: layer_id,
                line_index,
                time_secs: None,
            });
        }
    });
    if let Some(command) = command {
        output.project_changed |= state.apply_command(project, &command);
    }
}

fn draw_time_ticks(painter: &egui::Painter, rect: Rect, duration: f64) {
    let desired_ticks = (rect.width() / 90.0).max(1.0) as usize;
    let raw_step = duration / desired_ticks as f64;
    let magnitude = 10.0_f64.powf(raw_step.max(0.001).log10().floor());
    let normalized = raw_step / magnitude;
    let step = if normalized <= 1.0 {
        magnitude
    } else if normalized <= 2.0 {
        2.0 * magnitude
    } else if normalized <= 5.0 {
        5.0 * magnitude
    } else {
        10.0 * magnitude
    };
    let mut t = 0.0;
    while t <= duration {
        let x = time_to_x(rect, duration, t);
        painter.line_segment(
            [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
            Stroke::new(1.0, Color32::from_gray(45)),
        );
        painter.text(
            Pos2::new(x + 2.0, rect.min.y + 2.0),
            egui::Align2::LEFT_TOP,
            format_time(t),
            egui::FontId::proportional(9.0),
            Color32::GRAY,
        );
        t += step;
    }
}

fn draw_playhead(painter: &egui::Painter, rect: Rect, duration: f64, position_secs: f64) {
    let x = time_to_x(rect, duration, position_secs);
    painter.line_segment(
        [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
        Stroke::new(1.5, Color32::from_rgb(255, 90, 90)),
    );
}

fn timeline_axis(rect: Rect) -> Rect {
    Rect::from_min_max(Pos2::new(rect.min.x + LABEL_WIDTH, rect.min.y), rect.max)
}

fn time_to_x(rect: Rect, duration: f64, t: f64) -> f32 {
    rect.min.x + (t.clamp(0.0, duration) / duration) as f32 * rect.width()
}

fn x_to_time(rect: Rect, duration: f64, x: f32) -> f64 {
    f64::from(((x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0)) * duration
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn interp_label(interp: Interp) -> &'static str {
    match interp {
        Interp::Step => "ステップ",
        Interp::Linear => "線形",
        Interp::EaseIn => "イーズイン",
        Interp::EaseOut => "イーズアウト",
        Interp::EaseInOut => "イーズイン・アウト",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyframes::{AnimProperty, EditorCommand, apply_editor_command};
    use musicvideo_core::{default_project, new_lyrics_layer};

    #[test]
    fn waveform_constructor_sanitizes_duration_without_touching_samples() {
        let peaks = vec![[0.2, 0.3], [0.4, 0.5]];
        let waveform = TimelineWaveform::new(f64::NAN, peaks.clone());
        assert_eq!(waveform.duration_secs, 0.0);
        assert_eq!(waveform.stereo_peaks, peaks);
    }

    #[test]
    fn timeline_coordinate_mapping_clamps_to_project_duration() {
        let rect = Rect::from_min_max(Pos2::new(10.0, 0.0), Pos2::new(110.0, 20.0));
        assert_eq!(time_to_x(rect, 10.0, -1.0), 10.0);
        assert_eq!(time_to_x(rect, 10.0, 11.0), 110.0);
        assert_eq!(x_to_time(rect, 10.0, 60.0), 5.0);
    }

    #[test]
    fn timeline_only_contains_properties_with_keys_and_hides_last_deleted_track() {
        let mut project = default_project(10.0);
        assert_eq!(tracked_rows(&project).len(), 0);
        let layer = project.layers[0].id;
        apply_editor_command(
            &mut project,
            &EditorCommand::AddKey {
                layer,
                property: AnimProperty::Opacity,
                t: 2.0,
            },
        )
        .unwrap();
        assert_eq!(tracked_rows(&project).len(), 1);
        apply_editor_command(
            &mut project,
            &EditorCommand::DeleteKey {
                layer,
                property: AnimProperty::Opacity,
                key_index: 0,
            },
        )
        .unwrap();
        assert_eq!(tracked_rows(&project).len(), 0);
    }

    #[test]
    fn lyrics_have_one_timeline_row_with_only_timed_markers() {
        let mut project = default_project(10.0);
        let mut layer = new_lyrics_layer(&project);
        let musicvideo_core::LayerKind::Lyrics(lyrics) = &mut layer.kind else {
            unreachable!()
        };
        lyrics.lines = vec![
            musicvideo_core::LyricLine {
                text: "一".into(),
                time_secs: Some(1.0),
            },
            musicvideo_core::LyricLine {
                text: "未設定".into(),
                time_secs: None,
            },
            musicvideo_core::LyricLine {
                text: String::new(),
                time_secs: Some(2.0),
            },
        ];
        project.layers.push(layer);
        let rows = lyric_rows(&project);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].markers.len(), 2);
        assert_eq!(rows[0].markers[1].2, "");
    }
}
