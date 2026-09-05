//! ピッチ鍵盤の描画判断。
//!
//! egui や特定のラスタライザには依存せず、呼び出し側が保持する時間方向の状態を更新して、
//! 鍵と Bass マーカーの配置・表示強度だけを返す。本体の音楽ビューと音楽ビデオ書き出しは
//! この同じ結果をそれぞれの描画 API へ渡す。

use std::collections::VecDeque;

use crate::{
    SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumVisualRect, apply_display_gain_db,
    midi_to_hz, spectrum_axis_x_unclamped,
};

const KEY_HIGHLIGHT_DECAY: f32 = 0.925;
const KEY_HIGHLIGHT_MIN_PEAK: f32 = 0.035;
const KEY_SUSTAIN_ATTACK: f32 = 0.18;
const KEY_SUSTAIN_RELEASE: f32 = 0.965;
const KEY_HARMONIC_BOOST: f32 = 0.9;
const KEY_LOCAL_WINDOW: usize = 6;
const KEY_LOCAL_TARGET: usize = 3;
const KEY_LOCAL_RATIO_LO: f32 = 0.85;
const KEY_LOCAL_RATIO_HI: f32 = 1.25;
const KEY_LOCAL_FLOOR: f32 = 0.2;
const KEY_LOCAL_FULL_HZ: f32 = 160.0;
const KEY_LOCAL_FADE_HZ: f32 = 700.0;
const KEY_LOCAL_MIN_STRENGTH: f32 = 0.45;
const KEY_HARMONIC_SMOOTH: f32 = 0.45;
const KEY_GRADIENT_SPREAD: f32 = 1.0;
const BASS_MARKER_MIN_CONF: f32 = 0.06;
const BASS_MARKER_CONF_GAIN: f32 = 1.6;
const BASS_MARKER_DECAY: f32 = 0.92;
const BASS_MARKER_STICKY: f32 = 0.15;
const BASS_MARKER_DRAW_MIN: f32 = 0.03;
const BASS_MARKER_MIN_MIDI: u8 = 24;
const BASS_MARKER_MAX_MIDI: u8 = 55;
const BASS_MARKER_W: f32 = 7.0;
const BASS_MARKER_H: f32 = 5.0;
const EVEN_HARMONIC_OFFSETS: [usize; 4] = [12, 24, 31, 36];
const EVEN_HARMONIC_WEIGHTS: [f32; 4] = [0.5, 0.25, 0.166_7, 0.125];
const ODD_HARMONIC_OFFSETS: [usize; 3] = [19, 28, 34];
const ODD_HARMONIC_WEIGHTS: [f32; 3] = [0.333_3, 0.2, 0.142_9];
const KEY_BLACK_WIDTH_RATIO: f32 = 0.70;
const KEY_BLACK_HEIGHT_RATIO: f32 = 0.68;
const KEYBOARD_DISPLAY_MIN_MIDI: u8 = 12;
const KEYBOARD_DISPLAY_MAX_MIDI: u8 = 143;

pub const KEYBOARD_NOTE_COUNT: usize =
    (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KeyboardRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl KeyboardRect {
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(self) -> f32 {
        self.right - self.left
    }

    pub fn height(self) -> f32 {
        self.bottom - self.top
    }

    pub fn center_x(self) -> f32 {
        (self.left + self.right) * 0.5
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KeyboardPoint {
    pub x: f32,
    pub y: f32,
}

impl KeyboardPoint {
    const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KeyboardLevels {
    pub center: f32,
    pub even: f32,
    pub odd: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardFrame {
    pub note_min_midi: u8,
    pub levels: Vec<KeyboardLevels>,
}

impl Default for KeyboardFrame {
    fn default() -> Self {
        Self {
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            levels: vec![KeyboardLevels::default(); KEYBOARD_NOTE_COUNT],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyboardAnimationState {
    sustain: Vec<f32>,
    sustained: Vec<f32>,
    trail: Vec<f32>,
    even: Vec<f32>,
    odd: Vec<f32>,
    raw: Vec<f32>,
    even_energy: Vec<f32>,
    odd_energy: Vec<f32>,
    boosted: Vec<f32>,
    visual_center: Vec<f32>,
    visual_even: Vec<f32>,
    visual_odd: Vec<f32>,
    bass_marker_trail: Vec<f32>,
}

impl KeyboardAnimationState {
    pub fn clear(&mut self) {
        self.sustain.clear();
        self.sustained.clear();
        self.trail.clear();
        self.even.clear();
        self.odd.clear();
        self.raw.clear();
        self.even_energy.clear();
        self.odd_energy.clear();
        self.boosted.clear();
        self.visual_center.clear();
        self.visual_even.clear();
        self.visual_odd.clear();
        self.bass_marker_trail.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyPlacement {
    pub midi: u8,
    pub black: bool,
    pub rect: KeyboardRect,
    pub real_key: bool,
    pub note_position: Option<f32>,
    pub levels: KeyboardLevels,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyboardLayout {
    pub octave_guides_x: Vec<f32>,
    /// 白鍵が先、黒鍵が後の描画順。
    pub keys: Vec<KeyPlacement>,
}

const MAX_KEYBOARD_LAYOUT_CACHE_ENTRIES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeyboardLayoutCacheKey {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    note_min_midi: u8,
    note_count: usize,
}

impl KeyboardLayoutCacheKey {
    fn new(frame: &KeyboardFrame, rect: KeyboardRect) -> Self {
        Self {
            left: rect.left.to_bits(),
            top: rect.top.to_bits(),
            right: rect.right.to_bits(),
            bottom: rect.bottom.to_bits(),
            note_min_midi: frame.note_min_midi,
            note_count: frame.levels.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct KeyboardLayoutCacheEntry {
    key: KeyboardLayoutCacheKey,
    layout: KeyboardLayout,
}

/// 呼び出し側所有の小さな鍵盤レイアウト LRU。
///
/// 同じ矩形と解析音域なら、対数軸・鍵形状は再計算せず、フレームごとの輝度だけを
/// 既存の配置バッファへ反映する。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyboardLayoutCache {
    entries: VecDeque<KeyboardLayoutCacheEntry>,
    rebuilds: u64,
}

impl KeyboardLayoutCache {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn rebuilds(&self) -> u64 {
        self.rebuilds
    }

    pub fn entries(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BassMarkerPlacement {
    pub midi: u8,
    pub intensity: f32,
    pub triangle: [KeyboardPoint; 3],
}

/// 1 描画フレーム分の鍵盤輝度を更新する。
///
/// 定数、分岐、演算順は出荷済み音楽ビューの実装を保っている。`state` は呼び出し側が
/// フレーム間で保持し、seek や対象変更時に `clear` する。
pub fn prepare_keyboard_frame(
    notes: &[f32],
    note_min_midi: u8,
    norm_gain_db: f32,
    state: &mut KeyboardAnimationState,
) -> KeyboardFrame {
    let mut frame = KeyboardFrame::default();
    prepare_keyboard_frame_into(notes, note_min_midi, norm_gain_db, state, &mut frame);
    frame
}

/// 1 描画フレーム分の鍵盤輝度を既存バッファへ書き込み、フレームごとの確保を避ける。
pub fn prepare_keyboard_frame_into(
    notes: &[f32],
    note_min_midi: u8,
    norm_gain_db: f32,
    state: &mut KeyboardAnimationState,
    frame: &mut KeyboardFrame,
) {
    let note_count = KEYBOARD_NOTE_COUNT;
    for slot in [
        &mut state.sustain,
        &mut state.sustained,
        &mut state.trail,
        &mut state.even,
        &mut state.odd,
        &mut state.raw,
        &mut state.even_energy,
        &mut state.odd_energy,
        &mut state.boosted,
        &mut state.visual_center,
        &mut state.visual_even,
        &mut state.visual_odd,
    ] {
        if slot.len() != note_count {
            slot.clear();
            slot.resize(note_count, 0.0);
        }
    }
    if frame.levels.len() != note_count {
        frame.levels.clear();
        frame.levels.resize(note_count, KeyboardLevels::default());
    }
    update_keyboard_sustain(notes, &mut state.sustain, &mut state.sustained);
    compute_keyboard_visuals(
        &state.sustained,
        note_count,
        note_min_midi,
        norm_gain_db,
        &mut state.raw,
        &mut state.even_energy,
        &mut state.odd_energy,
        &mut state.boosted,
        &mut state.visual_center,
        &mut state.visual_even,
        &mut state.visual_odd,
    );
    for idx in 0..note_count {
        state.trail[idx] = (state.trail[idx] * KEY_HIGHLIGHT_DECAY).max(state.visual_center[idx]);
        state.even[idx] = state.even[idx] * (1.0 - KEY_HARMONIC_SMOOTH)
            + state.visual_even[idx] * KEY_HARMONIC_SMOOTH;
        state.odd[idx] = state.odd[idx] * (1.0 - KEY_HARMONIC_SMOOTH)
            + state.visual_odd[idx] * KEY_HARMONIC_SMOOTH;
        frame.levels[idx] = KeyboardLevels {
            center: state.trail[idx],
            even: state.even[idx].clamp(0.0, KEY_GRADIENT_SPREAD),
            odd: state.odd[idx].clamp(0.0, KEY_GRADIENT_SPREAD),
        };
    }
    frame.note_min_midi = note_min_midi;
}

/// 鍵盤の座標と描画順を計算する。座標は `rect` と同じ座標系で返す。
pub fn layout_keyboard(frame: &KeyboardFrame, rect: KeyboardRect) -> KeyboardLayout {
    let mut octave_guides_x = Vec::new();
    for c_midi in (KEYBOARD_DISPLAY_MIN_MIDI..=144_u8).step_by(12) {
        let x = shared_spectrum_axis_x_unclamped(rect, midi_to_hz(c_midi));
        if x > rect.left && x < rect.right {
            octave_guides_x.push(x);
        }
    }

    let mut keys = Vec::new();
    for want_black in [false, true] {
        for midi in KEYBOARD_DISPLAY_MIN_MIDI..=KEYBOARD_DISPLAY_MAX_MIDI {
            if is_black_key(midi) != want_black {
                continue;
            }
            let Some(key_rect) = conventional_key_rect(rect, midi, want_black) else {
                continue;
            };
            let index = midi.checked_sub(frame.note_min_midi).map(usize::from);
            let levels = index
                .and_then(|idx| frame.levels.get(idx).copied())
                .unwrap_or_default();
            let note_position = index
                .filter(|idx| *idx < frame.levels.len())
                .map(|idx| idx as f32 / frame.levels.len().saturating_sub(1).max(1) as f32);
            keys.push(KeyPlacement {
                midi,
                black: want_black,
                rect: key_rect,
                real_key: index.is_some_and(|idx| idx < frame.levels.len()),
                note_position,
                levels,
            });
        }
    }
    KeyboardLayout {
        octave_guides_x,
        keys,
    }
}

/// キャッシュ済みの鍵盤座標へ当該フレームの輝度を反映して返す。
pub fn layout_keyboard_cached<'a>(
    frame: &KeyboardFrame,
    rect: KeyboardRect,
    cache: &'a mut KeyboardLayoutCache,
) -> &'a KeyboardLayout {
    let key = KeyboardLayoutCacheKey::new(frame, rect);
    let index = cache.entries.iter().position(|entry| entry.key == key);
    let mut entry = if let Some(index) = index {
        cache.entries.remove(index).expect("cache index is valid")
    } else {
        cache.rebuilds = cache.rebuilds.saturating_add(1);
        KeyboardLayoutCacheEntry {
            key,
            layout: layout_keyboard(frame, rect),
        }
    };
    update_layout_levels(frame, &mut entry.layout);
    if cache.entries.len() >= MAX_KEYBOARD_LAYOUT_CACHE_ENTRIES {
        cache.entries.pop_front();
    }
    cache.entries.push_back(entry);
    &cache.entries.back().expect("layout was inserted").layout
}

fn update_layout_levels(frame: &KeyboardFrame, layout: &mut KeyboardLayout) {
    for key in &mut layout.keys {
        let index = key.midi.checked_sub(frame.note_min_midi).map(usize::from);
        key.levels = index
            .and_then(|idx| frame.levels.get(idx).copied())
            .unwrap_or_default();
        key.real_key = index.is_some_and(|idx| idx < frame.levels.len());
        key.note_position = index
            .filter(|idx| *idx < frame.levels.len())
            .map(|idx| idx as f32 / frame.levels.len().saturating_sub(1).max(1) as f32);
    }
}

/// Bass 推定のピッチクラスを鍵盤上の ▼ 配置へ変換し、残光状態を更新する。
pub fn layout_bass_markers(
    notes: &[f32],
    note_min_midi: u8,
    keyboard_rect: KeyboardRect,
    state: &mut KeyboardAnimationState,
    bass_marker: Option<(u8, f32)>,
) -> Vec<BassMarkerPlacement> {
    let mut markers = Vec::new();
    layout_bass_markers_into(
        notes,
        note_min_midi,
        keyboard_rect,
        state,
        bass_marker,
        None,
        &mut markers,
    );
    markers
}

/// Bass マーカーを既存バッファへ書き込む。layout があれば鍵座標も再利用する。
#[allow(clippy::too_many_arguments)]
pub fn layout_bass_markers_into(
    notes: &[f32],
    note_min_midi: u8,
    keyboard_rect: KeyboardRect,
    state: &mut KeyboardAnimationState,
    bass_marker: Option<(u8, f32)>,
    layout: Option<&KeyboardLayout>,
    markers: &mut Vec<BassMarkerPlacement>,
) {
    let note_count = KEYBOARD_NOTE_COUNT;
    let trail = &mut state.bass_marker_trail;
    if trail.len() != note_count {
        trail.clear();
        trail.resize(note_count, 0.0);
    }
    for value in trail.iter_mut() {
        *value *= BASS_MARKER_DECAY;
    }
    if let Some((pitch_class, confidence)) = bass_marker
        && confidence >= BASS_MARKER_MIN_CONF
    {
        let pc = pitch_class % 12;
        let mut best_idx = None;
        let mut best_score = 0.0_f32;
        for midi in BASS_MARKER_MIN_MIDI..=BASS_MARKER_MAX_MIDI {
            let Some(idx) = midi.checked_sub(note_min_midi).map(usize::from) else {
                continue;
            };
            if midi % 12 != pc || idx >= note_count {
                continue;
            }
            let energy = notes.get(idx).copied().unwrap_or(0.0);
            let score = energy + trail[idx] * BASS_MARKER_STICKY;
            if score > best_score {
                best_score = score;
                best_idx = Some(idx);
            }
        }
        if let Some(idx) = best_idx {
            let target = (confidence * BASS_MARKER_CONF_GAIN).clamp(0.0, 1.0);
            trail[idx] = trail[idx].max(target);
        }
    }

    markers.clear();
    for (idx, intensity) in trail.iter().copied().enumerate() {
        if intensity < BASS_MARKER_DRAW_MIN {
            continue;
        }
        let Some(midi) = u8::try_from(idx)
            .ok()
            .and_then(|idx| note_min_midi.checked_add(idx))
        else {
            continue;
        };
        let key_rect = if let Some(layout) = layout {
            layout
                .keys
                .iter()
                .find(|key| key.midi == midi)
                .map(|key| key.rect)
        } else {
            conventional_key_rect(keyboard_rect, midi, is_black_key(midi))
        };
        let Some(key_rect) = key_rect else {
            continue;
        };
        let cx = key_rect.center_x();
        let base_y = keyboard_rect.top - 1.0;
        markers.push(BassMarkerPlacement {
            midi,
            intensity,
            triangle: [
                KeyboardPoint::new(cx - BASS_MARKER_W * 0.5, base_y - BASS_MARKER_H),
                KeyboardPoint::new(cx + BASS_MARKER_W * 0.5, base_y - BASS_MARKER_H),
                KeyboardPoint::new(cx, base_y),
            ],
        });
    }
}

fn update_keyboard_sustain(notes: &[f32], sustain: &mut [f32], sustained: &mut [f32]) {
    let raw_peak = (0..sustain.len())
        .map(|idx| notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .fold(0.0_f32, f32::max);
    let broad_threshold = raw_peak * 0.42;
    let broad_count = if raw_peak > KEY_HIGHLIGHT_MIN_PEAK {
        (0..sustain.len())
            .filter(|idx| notes.get(*idx).copied().unwrap_or(0.0).clamp(0.0, 1.0) > broad_threshold)
            .count()
    } else {
        0
    };
    let broad_ratio = broad_count as f32 / sustain.len().max(1) as f32;
    let attack_scale = if broad_ratio > 0.34 { 0.35 } else { 1.0 };

    for (idx, (slot, output)) in sustain.iter_mut().zip(sustained).enumerate() {
        let current = notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        if current >= *slot {
            let attack = KEY_SUSTAIN_ATTACK * attack_scale;
            *slot = *slot * (1.0 - attack) + current * attack;
        } else {
            *slot = (*slot * KEY_SUSTAIN_RELEASE).max(current);
        }
        *output = *slot;
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_keyboard_visuals(
    notes: &[f32],
    note_count: usize,
    note_min_midi: u8,
    norm_gain_db: f32,
    raw: &mut [f32],
    even_energy: &mut [f32],
    odd_energy: &mut [f32],
    boosted: &mut [f32],
    center: &mut [f32],
    even: &mut [f32],
    odd: &mut [f32],
) {
    for (idx, value) in raw.iter_mut().enumerate() {
        *value = notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
    }
    let peak = raw.iter().copied().fold(0.0_f32, f32::max);
    if peak < KEY_HIGHLIGHT_MIN_PEAK {
        center.fill(0.0);
        even.fill(0.0);
        odd.fill(0.0);
        return;
    }

    for idx in 0..note_count {
        let (even_value, odd_value) = harmonic_energies(raw, idx);
        even_energy[idx] = even_value;
        odd_energy[idx] = odd_value;
    }

    for idx in 0..note_count {
        let support = ((even_energy[idx] + odd_energy[idx]) / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0);
        boosted[idx] = raw[idx] * (1.0 + KEY_HARMONIC_BOOST * support);
    }

    let boosted_peak = boosted.iter().copied().fold(0.0_f32, f32::max).max(1.0e-6);
    let presence_peak = apply_display_gain_db(peak, norm_gain_db);
    let presence = ((presence_peak - KEY_HIGHLIGHT_MIN_PEAK) / 0.28)
        .clamp(0.0, 1.0)
        .powf(0.7);
    for idx in 0..note_count {
        let lo = idx.saturating_sub(KEY_LOCAL_WINDOW);
        let hi = (idx + KEY_LOCAL_WINDOW).min(note_count - 1);
        let local_ref = kth_largest(&boosted[lo..=hi], KEY_LOCAL_TARGET);
        let ratio = boosted[idx] / (local_ref + 1.0e-4);
        let prominence = smoothstep(KEY_LOCAL_RATIO_LO, KEY_LOCAL_RATIO_HI, ratio);
        let local_factor = KEY_LOCAL_FLOOR + (1.0 - KEY_LOCAL_FLOOR) * prominence;
        let hz = midi_to_hz(note_min_midi + idx as u8);
        let strength = local_declutter_strength(hz);
        let eff = 1.0 - strength * (1.0 - local_factor);
        center[idx] = ((boosted[idx] / boosted_peak) * presence * eff).clamp(0.0, 1.0);
        even[idx] = (even_energy[idx] / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0);
        odd[idx] = (odd_energy[idx] / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0);
    }
}

fn kth_largest(values: &[f32], k: usize) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut buf = [0.0_f32; KEY_LOCAL_WINDOW * 2 + 1];
    buf[..values.len()].copy_from_slice(values);
    let buf = &mut buf[..values.len()];
    buf.sort_unstable_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    buf[k.clamp(1, buf.len()) - 1]
}

fn local_declutter_strength(hz: f32) -> f32 {
    let fade = smoothstep(
        KEY_LOCAL_FULL_HZ.log2(),
        KEY_LOCAL_FADE_HZ.log2(),
        hz.max(1.0).log2(),
    );
    1.0 - (1.0 - KEY_LOCAL_MIN_STRENGTH) * fade
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn harmonic_energies(notes: &[f32], idx: usize) -> (f32, f32) {
    let sample = |offset: usize| -> f32 {
        let center = idx + offset;
        let mut best = notes.get(center).copied().unwrap_or(0.0);
        if center >= 1 {
            best = best.max(notes.get(center - 1).copied().unwrap_or(0.0));
        }
        best = best.max(notes.get(center + 1).copied().unwrap_or(0.0));
        best.clamp(0.0, 1.0)
    };
    let mut even = 0.0_f32;
    for (offset, weight) in EVEN_HARMONIC_OFFSETS.iter().zip(EVEN_HARMONIC_WEIGHTS) {
        even += weight * sample(*offset);
    }
    let mut odd = 0.0_f32;
    for (offset, weight) in ODD_HARMONIC_OFFSETS.iter().zip(ODD_HARMONIC_WEIGHTS) {
        odd += weight * sample(*offset);
    }
    (even, odd)
}

fn conventional_key_rect(rect: KeyboardRect, midi: u8, black: bool) -> Option<KeyboardRect> {
    let pc = midi % 12;
    let octave_c = midi - pc;
    let (x0, white_w) = conventional_octave_geometry(rect, octave_c)?;
    let (left, right, bottom) = if black {
        let r = KEY_BLACK_WIDTH_RATIO;
        let center = match pc {
            1 => 1.0 - r / 6.0,
            3 => 2.0 + r / 6.0,
            6 => 4.0 - r / 4.0,
            8 => 5.0,
            10 => 6.0 + r / 4.0,
            _ => return None,
        };
        let w = white_w * r;
        (
            x0 + white_w * center - w * 0.5,
            x0 + white_w * center + w * 0.5,
            rect.top + rect.height() * KEY_BLACK_HEIGHT_RATIO,
        )
    } else {
        let index = match pc {
            0 => 0,
            2 => 1,
            4 => 2,
            5 => 3,
            7 => 4,
            9 => 5,
            11 => 6,
            _ => return None,
        } as f32;
        (
            x0 + white_w * index,
            x0 + white_w * (index + 1.0),
            rect.bottom,
        )
    };
    let left = left + 0.25;
    let right = right - 0.25;
    if right <= left {
        return None;
    }
    if right < rect.left || left > rect.right {
        return None;
    }
    Some(KeyboardRect::new(left, rect.top, right, bottom))
}

fn conventional_octave_geometry(rect: KeyboardRect, octave_c: u8) -> Option<(f32, f32)> {
    let c_x = shared_spectrum_axis_x_unclamped(rect, midi_to_hz(octave_c));
    let next_c_x = shared_spectrum_axis_x_unclamped(rect, midi_to_hz(octave_c.saturating_add(12)));
    let octave_w = next_c_x - c_x;
    if octave_w <= 1.0 {
        return None;
    }
    let white_w = octave_w / 7.0;
    Some((c_x - white_w * 0.5, white_w))
}

fn shared_spectrum_axis_x_unclamped(rect: KeyboardRect, hz: f32) -> f32 {
    spectrum_axis_x_unclamped(
        SpectrumVisualRect::new(rect.left, rect.top, rect.right, rect.bottom),
        hz,
    )
}

fn is_black_key(midi: u8) -> bool {
    matches!(midi % 12, 1 | 3 | 6 | 8 | 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_flat() {
        let mut state = KeyboardAnimationState::default();
        let frame = prepare_keyboard_frame(
            &vec![0.0; KEYBOARD_NOTE_COUNT],
            SPECTRUM_NOTE_MIN_MIDI,
            0.0,
            &mut state,
        );
        assert_eq!(frame.levels.len(), KEYBOARD_NOTE_COUNT);
        assert!(
            frame
                .levels
                .iter()
                .all(|levels| *levels == KeyboardLevels::default())
        );
    }

    #[test]
    fn fixed_input_captures_brightness_geometry_and_draw_order() {
        let mut notes = vec![0.0; KEYBOARD_NOTE_COUNT];
        notes[24] = 0.8;
        notes[36] = 0.5;
        notes[43] = 0.35;
        notes[48] = 0.3;
        let mut state = KeyboardAnimationState::default();
        let frame = prepare_keyboard_frame(&notes, SPECTRUM_NOTE_MIN_MIDI, 0.0, &mut state);
        let layout = layout_keyboard(&frame, KeyboardRect::new(18.0, 240.0, 1902.0, 274.0));

        assert_eq!(layout.octave_guides_x.len(), 10);
        assert_eq!(layout.keys.len(), 119);
        assert!(
            layout
                .keys
                .windows(2)
                .all(|pair| !pair[0].black || pair[1].black)
        );
        let key = layout.keys.iter().find(|key| key.midi == 45).unwrap();
        assert!(!key.black);
        assert!(key.real_key);
        assert_eq!(layout.keys.first().unwrap().midi, 16);
        assert_eq!(layout.keys.last().unwrap().midi, 133);
        assert!((layout.octave_guides_x[0] - 154.193_08).abs() < 1.0e-5);
        assert!((key.rect.left - 469.830_44).abs() < 1.0e-5);
        assert!((key.rect.right - 496.755_46).abs() < 1.0e-5);
        assert!((key.note_position.unwrap() - 0.275_862_07).abs() < 1.0e-7);
        assert!((key.levels.center - 0.516_639_9).abs() < 1.0e-6);
        assert!((key.levels.even - 0.182_685_63).abs() < 1.0e-6);
        assert!((key.levels.odd - 0.065_572_895).abs() < 1.0e-6);
    }

    #[test]
    fn hot_path_reuses_frame_scratch_and_layout_buffers() {
        let mut notes = vec![0.0; KEYBOARD_NOTE_COUNT];
        notes[24] = 0.8;
        let mut state = KeyboardAnimationState::default();
        let mut frame = KeyboardFrame::default();
        prepare_keyboard_frame_into(&notes, SPECTRUM_NOTE_MIN_MIDI, 0.0, &mut state, &mut frame);
        let frame_levels = frame.levels.as_ptr();
        let scratch = [
            state.sustain.as_ptr(),
            state.sustained.as_ptr(),
            state.raw.as_ptr(),
            state.even_energy.as_ptr(),
            state.odd_energy.as_ptr(),
            state.boosted.as_ptr(),
            state.visual_center.as_ptr(),
            state.visual_even.as_ptr(),
            state.visual_odd.as_ptr(),
        ];
        prepare_keyboard_frame_into(&notes, SPECTRUM_NOTE_MIN_MIDI, 0.0, &mut state, &mut frame);
        assert_eq!(frame.levels.as_ptr(), frame_levels);
        assert_eq!(
            scratch,
            [
                state.sustain.as_ptr(),
                state.sustained.as_ptr(),
                state.raw.as_ptr(),
                state.even_energy.as_ptr(),
                state.odd_energy.as_ptr(),
                state.boosted.as_ptr(),
                state.visual_center.as_ptr(),
                state.visual_even.as_ptr(),
                state.visual_odd.as_ptr(),
            ]
        );

        let rect = KeyboardRect::new(18.0, 240.0, 1902.0, 274.0);
        let mut cache = KeyboardLayoutCache::default();
        let keys = layout_keyboard_cached(&frame, rect, &mut cache)
            .keys
            .as_ptr();
        assert_eq!(cache.rebuilds(), 1);
        assert_eq!(cache.entries(), 1);
        assert_eq!(
            layout_keyboard_cached(&frame, rect, &mut cache)
                .keys
                .as_ptr(),
            keys
        );
        assert_eq!(cache.rebuilds(), 1);
    }

    #[test]
    fn cached_layout_stays_identical_to_fresh_layout_when_levels_change() {
        let rect = KeyboardRect::new(18.0, 240.0, 1902.0, 274.0);
        let mut frame = KeyboardFrame {
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            levels: vec![KeyboardLevels::default(); KEYBOARD_NOTE_COUNT],
        };
        frame.levels[24] = KeyboardLevels {
            center: 0.75,
            even: 0.2,
            odd: 0.4,
        };
        let mut cache = KeyboardLayoutCache::default();
        assert_eq!(
            layout_keyboard_cached(&frame, rect, &mut cache),
            &layout_keyboard(&frame, rect)
        );

        frame.levels[24] = KeyboardLevels::default();
        frame.levels[51] = KeyboardLevels {
            center: 0.3,
            even: 0.9,
            odd: 0.1,
        };
        assert_eq!(
            layout_keyboard_cached(&frame, rect, &mut cache),
            &layout_keyboard(&frame, rect)
        );
        assert_eq!(cache.rebuilds(), 1);
    }

    #[test]
    fn layout_cache_has_a_fixed_entry_bound() {
        let frame = KeyboardFrame {
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            levels: vec![KeyboardLevels::default(); KEYBOARD_NOTE_COUNT],
        };
        let mut cache = KeyboardLayoutCache::default();
        for offset in 0..MAX_KEYBOARD_LAYOUT_CACHE_ENTRIES * 2 {
            let left = offset as f32;
            let _ = layout_keyboard_cached(
                &frame,
                KeyboardRect::new(left, 10.0, left + 800.0, 80.0),
                &mut cache,
            );
            assert!(cache.entries() <= MAX_KEYBOARD_LAYOUT_CACHE_ENTRIES);
        }
        assert_eq!(cache.entries(), MAX_KEYBOARD_LAYOUT_CACHE_ENTRIES);
    }

    #[test]
    fn bass_marker_prefers_strongest_matching_octave_and_has_afterglow() {
        let mut notes = vec![0.0; KEYBOARD_NOTE_COUNT];
        notes[(48 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.8;
        notes[(36 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.3;
        let mut state = KeyboardAnimationState::default();
        let rect = KeyboardRect::new(18.0, 240.0, 1902.0, 274.0);
        let current = layout_bass_markers(
            &notes,
            SPECTRUM_NOTE_MIN_MIDI,
            rect,
            &mut state,
            Some((0, 0.5)),
        );
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].midi, 48);
        assert_eq!(current[0].intensity, 0.8);

        let afterglow = layout_bass_markers(&notes, SPECTRUM_NOTE_MIN_MIDI, rect, &mut state, None);
        assert_eq!(afterglow.len(), 1);
        assert_eq!(afterglow[0].midi, 48);
        assert!((afterglow[0].intensity - 0.736).abs() < 1.0e-6);
    }

    #[test]
    fn black_key_classification_matches_semitones() {
        let expected = [
            false, true, false, true, false, false, true, false, true, false, true, false,
        ];
        for (pc, want) in expected.iter().enumerate() {
            assert_eq!(is_black_key(60 + pc as u8), *want, "pc={pc}");
        }
    }
}
