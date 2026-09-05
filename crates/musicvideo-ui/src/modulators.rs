use std::collections::HashMap;

use musicvideo_core::{
    BAND_ENERGY_CALIBRATION_HIGH_PERCENTILE, BAND_ENERGY_CALIBRATION_LOW_PERCENTILE,
    BandCalibration, BandRange, BandWeighting, KICK_CALIBRATION_HIGH_PERCENTILE,
    KICK_CALIBRATION_LOW_PERCENTILE, Layer, LayerKind, ModulationBinding, ModulationCurve,
    ModulationTarget, Modulator, ModulatorCalibration, ModulatorId, ModulatorSource,
    ModulatorValue, Project, Smoothing, SpectrumShape, modulator_calibration_requests,
    new_band_energy_modulator,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoteInputs {
    model_value: (u8, u8),
    low: String,
    high: String,
}

impl NoteInputs {
    fn new(low: u8, high: u8) -> Self {
        Self {
            model_value: (low, high),
            low: midi_note_name(low),
            high: midi_note_name(high),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModulatorEditorState {
    note_inputs: HashMap<ModulatorId, NoteInputs>,
}

pub(crate) fn draw_modulators(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut ModulatorEditorState,
    live_values: &[ModulatorValue],
    calibrations: &[ModulatorCalibration],
) -> bool {
    let mut changed = false;
    let mut remove = None;
    egui::CollapsingHeader::new("モジュレータ")
        .default_open(true)
        .show(ui, |ui| {
            ui.small("音声帯域の値を 0〜1 に正規化し、レイヤーのスカラー値へ加算します");
            for (index, modulator) in project.modulators.iter_mut().enumerate() {
                ui.push_id(modulator.id.0, |ui| {
                    let live = live_value(live_values, modulator.id);
                    egui::CollapsingHeader::new(if modulator.name.is_empty() {
                        format!("モジュレータ {}", modulator.id.0)
                    } else {
                        modulator.name.clone()
                    })
                    .default_open(index == 0)
                    .show(ui, |ui| {
                        changed |= ui.text_edit_singleline(&mut modulator.name).changed();
                        changed |= edit_source(ui, modulator, state, calibrations);
                        ui.add(
                            egui::ProgressBar::new(live)
                                .show_percentage()
                                .text(format!("現在値 {live:.3}")),
                        );
                        if ui.button("このモジュレータを削除").clicked() {
                            remove = Some(modulator.id);
                        }
                    });
                });
            }
            if ui.button("＋ キックに合わせる動きを追加").clicked() {
                project.modulators.push(new_band_energy_modulator(project));
                changed = true;
            }
        });
    if let Some(id) = remove {
        project.modulators.retain(|modulator| modulator.id != id);
        for layer in &mut project.layers {
            layer.modulations.retain(|binding| binding.modulator != id);
        }
        state.note_inputs.remove(&id);
        changed = true;
    }
    changed
}

fn edit_source(
    ui: &mut egui::Ui,
    modulator: &mut Modulator,
    state: &mut ModulatorEditorState,
    calibrations: &[ModulatorCalibration],
) -> bool {
    let mut changed = false;
    let mut source_kind = match modulator.source {
        ModulatorSource::BandEnergy { .. } => BandSourceKind::Energy,
        ModulatorSource::BandOnset { .. } => BandSourceKind::Onset,
    };
    let previous_source_kind = source_kind;
    egui::ComboBox::from_label("反応するもの")
        .selected_text(source_kind.label())
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut source_kind,
                BandSourceKind::Onset,
                BandSourceKind::Onset.label(),
            );
            ui.selectable_value(
                &mut source_kind,
                BandSourceKind::Energy,
                BandSourceKind::Energy.label(),
            );
        });
    if source_kind != previous_source_kind {
        set_source_kind(&mut modulator.source, source_kind);
        changed = true;
    }
    let current_calibration = matching_calibration(modulator, calibrations).copied();

    let (range, smoothing, weighting, gain_db, floor, ceiling, calibration, threshold) =
        match &mut modulator.source {
            ModulatorSource::BandEnergy {
                range,
                smoothing,
                weighting,
                gain_db,
                floor,
                ceiling,
                calibration,
                threshold,
            }
            | ModulatorSource::BandOnset {
                range,
                smoothing,
                weighting,
                gain_db,
                floor,
                ceiling,
                calibration,
                threshold,
            } => (
                range,
                smoothing,
                weighting,
                gain_db,
                floor,
                ceiling,
                calibration,
                threshold,
            ),
        };
    let mut semitone = matches!(range, BandRange::Semitone { .. });
    let was_semitone = semitone;
    ui.horizontal(|ui| {
        ui.label("範囲指定");
        ui.radio_value(&mut semitone, false, "Hz");
        ui.radio_value(&mut semitone, true, "音名");
    });
    if semitone != was_semitone {
        *range = match *range {
            BandRange::Hz { low_hz, high_hz } => {
                let (low_midi, high_midi) = hz_range_to_midi(low_hz, high_hz);
                BandRange::Semitone {
                    low_midi,
                    high_midi,
                }
            }
            BandRange::Semitone {
                low_midi,
                high_midi,
            } => BandRange::Hz {
                low_hz: midi_edge_hz(low_midi, -1.0),
                high_hz: midi_edge_hz(high_midi, 1.0),
            },
        };
        changed = true;
    }
    match range {
        BandRange::Hz { low_hz, high_hz } => {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(low_hz)
                            .range(1.0..=24_000.0)
                            .speed(1.0)
                            .suffix(" Hz 下限"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(high_hz)
                            .range(1.0..=24_000.0)
                            .speed(1.0)
                            .suffix(" Hz 上限"),
                    )
                    .changed();
            });
            if changed {
                *low_hz = low_hz.clamp(1.0, 23_999.0);
                *high_hz = high_hz.clamp(*low_hz + 0.01, 24_000.0);
            }
        }
        BandRange::Semitone {
            low_midi,
            high_midi,
        } => {
            let inputs = state
                .note_inputs
                .entry(modulator.id)
                .or_insert_with(|| NoteInputs::new(*low_midi, *high_midi));
            if inputs.model_value != (*low_midi, *high_midi) {
                *inputs = NoteInputs::new(*low_midi, *high_midi);
            }
            let mut text_changed = false;
            ui.horizontal(|ui| {
                ui.label("下限");
                text_changed |= ui
                    .add(egui::TextEdit::singleline(&mut inputs.low).desired_width(56.0))
                    .changed();
                ui.label("上限");
                text_changed |= ui
                    .add(egui::TextEdit::singleline(&mut inputs.high).desired_width(56.0))
                    .changed();
                ui.weak("例: C1–B1 / C#4");
            });
            let parsed = note_name_to_midi(&inputs.low).zip(note_name_to_midi(&inputs.high));
            if let Some((low, high)) = parsed.filter(|(low, high)| low <= high) {
                if text_changed || (*low_midi, *high_midi) != (low, high) {
                    *low_midi = low;
                    *high_midi = high;
                    inputs.model_value = (low, high);
                    changed = true;
                }
            } else {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    "音名は C1、C#4、Db4 の形式で、下限≦上限にしてください",
                );
            }
        }
    }

    changed |= edit_smoothing(ui, smoothing);
    ui.horizontal(|ui| {
        ui.label("周波数重み");
        egui::ComboBox::from_id_salt(("band-weighting", modulator.id.0))
            .selected_text(match weighting {
                BandWeighting::Raw => "なし（素のエネルギー）",
                BandWeighting::MivDisplay => "mIV 表示と同じ",
            })
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(weighting, BandWeighting::Raw, "なし（素のエネルギー）")
                    .changed();
                changed |= ui
                    .selectable_value(weighting, BandWeighting::MivDisplay, "mIV 表示と同じ")
                    .changed();
            });
    });
    changed |= ui
        .add(
            egui::DragValue::new(gain_db)
                .range(-72.0..=72.0)
                .speed(0.1)
                .prefix("ゲイン ")
                .suffix(" dB"),
        )
        .changed();
    let mut automatic = matches!(calibration, BandCalibration::Auto { .. });
    if ui
        .checkbox(&mut automatic, "曲全体から反応範囲を自動調整")
        .on_hover_text("全尺の分布を一度調べ、静かな値と強い値を 0〜1 に割り当てます")
        .changed()
    {
        if automatic {
            let (low_percentile, high_percentile) = source_kind.default_percentiles();
            *calibration = BandCalibration::Auto {
                low_percentile,
                high_percentile,
            };
        } else {
            if let Some(value) = current_calibration {
                *floor = value.floor;
                *ceiling = value.ceiling;
            }
            *calibration = BandCalibration::Manual;
        }
        changed = true;
    }
    match calibration {
        BandCalibration::Manual => {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(egui::Slider::new(floor, 0.0..=0.999).text("弱い側"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(ceiling, 0.001..=1.0).text("強い側"))
                    .changed();
            });
            ui.weak("手動調整中");
        }
        BandCalibration::Auto {
            low_percentile,
            high_percentile,
        } => {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(low_percentile)
                            .range(0.0..=99.0)
                            .speed(0.1)
                            .suffix("% 下限"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(high_percentile)
                            .range(1.0..=100.0)
                            .speed(0.1)
                            .suffix("% 上限"),
                    )
                    .changed();
            });
            if *high_percentile <= *low_percentile {
                *high_percentile = (*low_percentile + 0.1).min(100.0);
            }
            if let Some(value) = current_calibration {
                ui.label(format!(
                    "この曲では {:.3}〜{:.3} を 0〜1 に割り当てました",
                    value.floor, value.ceiling
                ));
            } else {
                ui.weak("この曲の反応範囲を計算しています…");
            }
        }
    }
    changed |= ui
        .add(
            egui::Slider::new(threshold, 0.0..=1.0)
                .text("反応を始める強さ")
                .step_by(0.01),
        )
        .on_hover_text("この値より弱い入力は無音として扱います")
        .changed();
    if changed {
        *floor = floor.clamp(0.0, 0.999);
        *ceiling = ceiling.clamp(*floor + 0.001, 1.0);
        *threshold = threshold.clamp(0.0, 1.0);
    }
    changed
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BandSourceKind {
    Energy,
    Onset,
}

impl BandSourceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Energy => "帯域の音量",
            Self::Onset => "音の立ち上がり（キック向け）",
        }
    }

    fn default_percentiles(self) -> (f32, f32) {
        match self {
            Self::Energy => (
                BAND_ENERGY_CALIBRATION_LOW_PERCENTILE,
                BAND_ENERGY_CALIBRATION_HIGH_PERCENTILE,
            ),
            Self::Onset => (
                KICK_CALIBRATION_LOW_PERCENTILE,
                KICK_CALIBRATION_HIGH_PERCENTILE,
            ),
        }
    }
}

fn set_source_kind(source: &mut ModulatorSource, kind: BandSourceKind) {
    let mut replacement = match source.clone() {
        ModulatorSource::BandEnergy {
            range,
            smoothing,
            weighting,
            gain_db,
            floor,
            ceiling,
            calibration,
            threshold,
        }
        | ModulatorSource::BandOnset {
            range,
            smoothing,
            weighting,
            gain_db,
            floor,
            ceiling,
            calibration,
            threshold,
        } => match kind {
            BandSourceKind::Energy => ModulatorSource::BandEnergy {
                range,
                smoothing,
                weighting,
                gain_db,
                floor,
                ceiling,
                calibration,
                threshold,
            },
            BandSourceKind::Onset => ModulatorSource::BandOnset {
                range,
                smoothing,
                weighting,
                gain_db,
                floor,
                ceiling,
                calibration,
                threshold,
            },
        },
    };
    let calibration = match &mut replacement {
        ModulatorSource::BandEnergy { calibration, .. }
        | ModulatorSource::BandOnset { calibration, .. } => calibration,
    };
    if matches!(calibration, BandCalibration::Auto { .. }) {
        let (low_percentile, high_percentile) = kind.default_percentiles();
        *calibration = BandCalibration::Auto {
            low_percentile,
            high_percentile,
        };
    }
    *source = replacement;
}

fn matching_calibration<'a>(
    modulator: &Modulator,
    calibrations: &'a [ModulatorCalibration],
) -> Option<&'a ModulatorCalibration> {
    let request = modulator_calibration_requests(std::slice::from_ref(modulator))
        .into_iter()
        .next()?;
    calibrations
        .iter()
        .find(|calibration| calibration.request == request)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SmoothingMode {
    None,
    Average,
    Envelope,
}

fn edit_smoothing(ui: &mut egui::Ui, smoothing: &mut Smoothing) -> bool {
    let mut mode = match smoothing {
        Smoothing::None => SmoothingMode::None,
        Smoothing::Average { .. } => SmoothingMode::Average,
        Smoothing::Envelope { .. } => SmoothingMode::Envelope,
    };
    let previous_mode = mode;
    egui::ComboBox::from_label("反応のしかた")
        .selected_text(match mode {
            SmoothingMode::None => "平滑化しない",
            SmoothingMode::Average => "均等になめらか",
            SmoothingMode::Envelope => "すばやく反応・ゆっくり戻る",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut mode, SmoothingMode::None, "平滑化しない");
            ui.selectable_value(&mut mode, SmoothingMode::Average, "均等になめらか");
            ui.selectable_value(
                &mut mode,
                SmoothingMode::Envelope,
                "すばやく反応・ゆっくり戻る",
            );
        });
    let mut changed = mode != previous_mode;
    if changed {
        *smoothing = match mode {
            SmoothingMode::None => Smoothing::None,
            SmoothingMode::Average => Smoothing::Average { factor: 0.7 },
            SmoothingMode::Envelope => Smoothing::Envelope {
                attack_secs: musicvideo_core::KICK_ATTACK_SECS,
                release_secs: musicvideo_core::KICK_RELEASE_SECS,
            },
        };
    }
    match smoothing {
        Smoothing::None => {}
        Smoothing::Average { factor } => {
            changed |= ui
                .add(egui::Slider::new(factor, 0.0..=1.0).text("なめらかさ"))
                .changed();
        }
        Smoothing::Envelope {
            attack_secs,
            release_secs,
        } => {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(attack_secs)
                            .range(0.001..=1.0)
                            .speed(0.001)
                            .prefix("反応 ")
                            .suffix(" 秒"),
                    )
                    .on_hover_text("小さいほど音の立ち上がりへ速く追従します")
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(release_secs)
                            .range(0.001..=5.0)
                            .speed(0.005)
                            .prefix("戻り ")
                            .suffix(" 秒"),
                    )
                    .on_hover_text("大きいほど効果がゆっくり元へ戻ります")
                    .changed();
            });
        }
    }
    changed
}

pub(crate) fn draw_bindings(
    ui: &mut egui::Ui,
    layer: &mut Layer,
    modulators: &[Modulator],
    live_values: &[ModulatorValue],
) -> bool {
    let targets = available_targets(layer);
    let mut changed = false;
    let mut remove = None;
    ui.separator();
    egui::CollapsingHeader::new("変調の割り当て")
        .default_open(!layer.modulations.is_empty())
        .show(ui, |ui| {
            for (index, binding) in layer.modulations.iter_mut().enumerate() {
                ui.push_id(index, |ui| {
                    ui.group(|ui| {
                        egui::ComboBox::from_id_salt("target")
                            .selected_text(target_label(binding.target))
                            .show_ui(ui, |ui| {
                                for target in &targets {
                                    changed |= ui
                                        .selectable_value(
                                            &mut binding.target,
                                            *target,
                                            target_label(*target),
                                        )
                                        .changed();
                                }
                            });
                        egui::ComboBox::from_id_salt("source")
                            .selected_text(modulator_name(modulators, binding.modulator))
                            .show_ui(ui, |ui| {
                                for modulator in modulators {
                                    changed |= ui
                                        .selectable_value(
                                            &mut binding.modulator,
                                            modulator.id,
                                            &modulator.name,
                                        )
                                        .changed();
                                }
                            });
                        ui.horizontal(|ui| {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut binding.amount)
                                        .speed(amount_speed(binding.target))
                                        .prefix("量 ")
                                        .suffix(target_unit(binding.target)),
                                )
                                .changed();
                            egui::ComboBox::from_id_salt("curve")
                                .selected_text(curve_label(binding.curve))
                                .show_ui(ui, |ui| {
                                    changed |= ui
                                        .selectable_value(
                                            &mut binding.curve,
                                            ModulationCurve::Linear,
                                            "線形",
                                        )
                                        .changed();
                                    changed |= ui
                                        .selectable_value(
                                            &mut binding.curve,
                                            ModulationCurve::Squared,
                                            "二乗",
                                        )
                                        .changed();
                                    changed |= ui
                                        .selectable_value(
                                            &mut binding.curve,
                                            ModulationCurve::Sqrt,
                                            "平方根",
                                        )
                                        .changed();
                                });
                        });
                        let source = live_value(live_values, binding.modulator);
                        ui.small(format!(
                            "入力 {source:.3} / 加算 {:+.3}{}",
                            binding.amount * binding.curve.apply(source),
                            target_unit(binding.target)
                        ));
                        if ui.small_button("割り当てを削除").clicked() {
                            remove = Some(index);
                        }
                    });
                });
            }
            let can_add = !modulators.is_empty() && !targets.is_empty();
            if ui
                .add_enabled(can_add, egui::Button::new("＋ 割り当てを追加"))
                .on_disabled_hover_text("先にモジュレータを追加してください")
                .clicked()
            {
                layer.modulations.push(ModulationBinding {
                    target: targets[0],
                    modulator: modulators[0].id,
                    amount: 0.25,
                    curve: ModulationCurve::Linear,
                });
                changed = true;
            }
        });
    if let Some(index) = remove {
        layer.modulations.remove(index);
        changed = true;
    }
    changed
}

fn available_targets(layer: &Layer) -> Vec<ModulationTarget> {
    let mut targets = vec![
        ModulationTarget::Opacity,
        ModulationTarget::Scale,
        ModulationTarget::Rotation,
        ModulationTarget::CornerRadius,
        ModulationTarget::GlowRadius,
        ModulationTarget::GlowAlpha,
    ];
    if matches!(&layer.kind, LayerKind::Image { .. }) {
        targets.insert(0, ModulationTarget::ImageZoom);
    } else {
        targets.push(ModulationTarget::FillBrightness);
    }
    if let LayerKind::Spectrum(spectrum) = &layer.kind {
        targets.push(ModulationTarget::SpectrumThickness);
        if matches!(spectrum.shape, SpectrumShape::Radial { .. }) {
            targets.push(ModulationTarget::SpectrumRadius);
        }
    }
    targets
}

fn live_value(values: &[ModulatorValue], id: ModulatorId) -> f32 {
    values
        .iter()
        .find(|value| value.id == id)
        .map_or(0.0, |value| value.value.clamp(0.0, 1.0))
}

fn modulator_name(modulators: &[Modulator], id: ModulatorId) -> String {
    modulators
        .iter()
        .find(|modulator| modulator.id == id)
        .map_or_else(
            || format!("未解決 {}", id.0),
            |modulator| modulator.name.clone(),
        )
}

fn target_label(target: ModulationTarget) -> &'static str {
    match target {
        ModulationTarget::Opacity => "不透明度",
        ModulationTarget::Scale => "レイヤー矩形の拡縮",
        ModulationTarget::ImageZoom => "絵そのものの拡大",
        ModulationTarget::Rotation => "回転",
        ModulationTarget::CornerRadius => "角丸",
        ModulationTarget::GlowRadius => "グロー半径",
        ModulationTarget::GlowAlpha => "グローの濃さ",
        ModulationTarget::FillBrightness => "塗りの明るさ",
        ModulationTarget::SpectrumThickness => "スペクトラムの太さ",
        ModulationTarget::SpectrumRadius => "円形スペクトラムの半径",
    }
}

fn target_unit(target: ModulationTarget) -> &'static str {
    match target {
        ModulationTarget::Opacity
        | ModulationTarget::Scale
        | ModulationTarget::ImageZoom
        | ModulationTarget::GlowAlpha
        | ModulationTarget::FillBrightness => "",
        ModulationTarget::Rotation => "°",
        ModulationTarget::CornerRadius
        | ModulationTarget::GlowRadius
        | ModulationTarget::SpectrumThickness
        | ModulationTarget::SpectrumRadius => " px",
    }
}

fn amount_speed(target: ModulationTarget) -> f64 {
    match target {
        ModulationTarget::Opacity
        | ModulationTarget::Scale
        | ModulationTarget::ImageZoom
        | ModulationTarget::GlowAlpha
        | ModulationTarget::FillBrightness => 0.01,
        _ => 0.5,
    }
}

fn curve_label(curve: ModulationCurve) -> &'static str {
    match curve {
        ModulationCurve::Linear => "線形",
        ModulationCurve::Squared => "二乗",
        ModulationCurve::Sqrt => "平方根",
    }
}

fn midi_note_name(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[midi as usize % 12], midi as i16 / 12 - 1)
}

fn note_name_to_midi(value: &str) -> Option<u8> {
    let value = value.trim();
    let mut chars = value.chars();
    let note = chars.next()?.to_ascii_uppercase();
    let base = match note {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (accidental, octave) = if let Some(rest) = rest.strip_prefix('#') {
        (1, rest)
    } else if let Some(rest) = rest.strip_prefix(['b', 'B']) {
        (-1, rest)
    } else {
        (0, rest)
    };
    let octave = octave.parse::<i16>().ok()?;
    let midi = (octave + 1) * 12 + base + accidental;
    u8::try_from(midi).ok().filter(|midi| *midi <= 127)
}

fn hz_to_midi_position(hz: f32) -> f32 {
    if !hz.is_finite() || hz <= 0.0 {
        return 0.0;
    }
    (69.0 + 12.0 * (hz / 440.0).log2()).clamp(0.0, 127.0)
}

fn hz_range_to_midi(low_hz: f32, high_hz: f32) -> (u8, u8) {
    let low = hz_to_midi_position(low_hz).ceil() as u8;
    let high = hz_to_midi_position(high_hz).floor() as u8;
    if low <= high {
        (low, high)
    } else {
        let middle = hz_to_midi_position((low_hz * high_hz).sqrt()).round() as u8;
        (middle, middle)
    }
}

fn midi_edge_hz(midi: u8, half_step: f32) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 + half_step * 0.5 - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_round_trip_and_accept_flats() {
        for midi in 0..=127 {
            assert_eq!(note_name_to_midi(&midi_note_name(midi)), Some(midi));
        }
        assert_eq!(note_name_to_midi("C1"), Some(24));
        assert_eq!(note_name_to_midi("Bb1"), Some(34));
        assert_eq!(note_name_to_midi("C#4"), Some(61));
        assert_eq!(note_name_to_midi("H2"), None);
        assert_eq!(
            hz_range_to_midi(midi_edge_hz(24, -1.0), midi_edge_hz(35, 1.0)),
            (24, 35)
        );
    }

    #[test]
    fn deleting_a_modulator_can_remove_its_bindings_without_touching_others() {
        let mut project = musicvideo_core::default_project(10.0);
        let first = new_band_energy_modulator(&project);
        project.modulators.push(first.clone());
        let second = new_band_energy_modulator(&project);
        project.modulators.push(second.clone());
        project.layers[0].modulations = vec![
            ModulationBinding {
                target: ModulationTarget::Scale,
                modulator: first.id,
                amount: 0.1,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::Opacity,
                modulator: second.id,
                amount: -0.5,
                curve: ModulationCurve::Squared,
            },
        ];
        let removed = first.id;
        project.modulators.retain(|item| item.id != removed);
        for layer in &mut project.layers {
            layer.modulations.retain(|item| item.modulator != removed);
        }
        assert_eq!(project.modulators, vec![second.clone()]);
        assert_eq!(project.layers[0].modulations.len(), 1);
        assert_eq!(project.layers[0].modulations[0].modulator, second.id);
    }

    #[test]
    fn changing_feature_keeps_settings_but_uses_feature_specific_percentiles() {
        let project = musicvideo_core::default_project(10.0);
        let mut source = new_band_energy_modulator(&project).source;
        set_source_kind(&mut source, BandSourceKind::Energy);
        let ModulatorSource::BandEnergy {
            range,
            smoothing,
            gain_db,
            calibration,
            ..
        } = source
        else {
            panic!("source kind did not change");
        };
        assert_eq!(
            range,
            BandRange::Hz {
                low_hz: 40.0,
                high_hz: 120.0
            }
        );
        assert_eq!(
            smoothing,
            Smoothing::Envelope {
                attack_secs: musicvideo_core::KICK_ATTACK_SECS,
                release_secs: musicvideo_core::KICK_RELEASE_SECS,
            }
        );
        assert_eq!(gain_db, 3.0);
        assert_eq!(
            calibration,
            BandCalibration::Auto {
                low_percentile: BAND_ENERGY_CALIBRATION_LOW_PERCENTILE,
                high_percentile: BAND_ENERGY_CALIBRATION_HIGH_PERCENTILE,
            }
        );
    }
}
