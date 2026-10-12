//! Pitch-lab harness: run mIV's keyboard pipeline (SpectrumAnalyzer + ui_music_spectrum
//! keyboard visuals, copied verbatim) over a raw f32 interleaved stereo 48 kHz file.
//! Usage: keyboard_dump <in.f32> <out.csv> [frame_ms]
#![allow(dead_code, clippy::all)]
use music_core::{
    SPECTRUM_BAND_COUNT, SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalyzer,
};
use std::io::Write;

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
const EVEN_HARMONIC_OFFSETS: [usize; 4] = [12, 24, 31, 36];
const EVEN_HARMONIC_WEIGHTS: [f32; 4] = [0.5, 0.25, 0.166_7, 0.125];
const ODD_HARMONIC_OFFSETS: [usize; 3] = [19, 28, 34];
const ODD_HARMONIC_WEIGHTS: [f32; 3] = [0.333_3, 0.2, 0.142_9];
const KEY_BLACK_WIDTH_RATIO: f32 = 0.70;
const KEY_BLACK_HEIGHT_RATIO: f32 = 0.68;

fn update_keyboard_sustain(notes: &[f32], sustain: &mut [f32]) -> Vec<f32> {
    let mut sustained = Vec::with_capacity(sustain.len());
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

    for (idx, slot) in sustain.iter_mut().enumerate() {
        let current = notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        if current >= *slot {
            let attack = KEY_SUSTAIN_ATTACK * attack_scale;
            *slot = *slot * (1.0 - attack) + current * attack;
        } else {
            *slot = (*slot * KEY_SUSTAIN_RELEASE).max(current);
        }
        sustained.push(*slot);
    }
    sustained
}

/// 鍵盤 1 フレーム分の描画量。`center` が中央 (基音) の明るさ、`even`/`odd` が縦グラデの
/// 下 / 上方向の伸び (倍音サポート)。
struct KeyboardVisuals {
    center: Vec<f32>,
    even: Vec<f32>,
    odd: Vec<f32>,
}

/// 知覚補正済み notes から、鍵盤の中央明るさ (Layer ② 加点のみ倍音サリエンス + Layer ③ 適応的
/// 局所正規化) と、偶数 / 奇数倍音サポートを求める。減点は一切せず、加点は乗数≥1 なので生より
/// 暗くならない (オクターブ重ねの実音を消さない / 高音域を沈めないため)。
fn compute_keyboard_visuals(
    notes: &[f32],
    note_count: usize,
    norm_gain_db: f32,
) -> KeyboardVisuals {
    let flat = || KeyboardVisuals {
        center: vec![0.0; note_count],
        even: vec![0.0; note_count],
        odd: vec![0.0; note_count],
    };
    let raw: Vec<f32> = (0..note_count)
        .map(|idx| notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .collect();
    let peak = raw.iter().copied().fold(0.0_f32, f32::max);
    if peak < KEY_HIGHLIGHT_MIN_PEAK {
        return flat();
    }

    let mut even_energy = vec![0.0_f32; note_count];
    let mut odd_energy = vec![0.0_f32; note_count];
    for idx in 0..note_count {
        let (even, odd) = harmonic_energies(&raw, idx);
        even_energy[idx] = even;
        odd_energy[idx] = odd;
    }

    // Layer ② 加点のみ (乗数≥1): 基音に倍音支持があるほど明るくする。積 (HPS) ではなく和。
    let mut boosted = vec![0.0_f32; note_count];
    for idx in 0..note_count {
        let support = ((even_energy[idx] + odd_energy[idx]) / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0);
        boosted[idx] = raw[idx] * (1.0 + KEY_HARMONIC_BOOST * support);
    }

    // Layer ③ 適応的局所正規化: 各鍵の周囲 ±KEY_LOCAL_WINDOW 半音の分布に対する相対的な際立ちで
    // 明るさを絞る。局所しきい値 = 窓内の上位 KEY_LOCAL_TARGET 番目の値 (密度適応) なので、密な帯
    // (低域クラッタ) は強い鍵だけ残し、疎な帯は主音がしきい値を明確に超えて強調される (= オクターブ
    // 内で光る鍵数が一定に近づく)。「選択」(local_factor) と「明るさ」(絶対レベル×presence) を分離し、
    // 静かな帯の主音が誤って満額で光らないようにする。低域ほど強く、中高域は弱く掛けて既存の
    // (機能している) 挙動を保つ。旧 ±1 ピークゲートを置換。
    let boosted_peak = boosted.iter().copied().fold(0.0_f32, f32::max).max(1.0e-6);
    // 全体ラウドネスゲート: 静かな箇所で鍵盤が光りっぱなしにならないよう全体を抑える。Norm
    // (ラウドネス正規化) 適用中は presence 用の peak だけ gain 分持ち上げ、録音レベルが小さくても
    // 体感ラウドネスに合った明るさにする (パターンは boosted のまま = 不変)。
    let presence_peak = music_core::apply_display_gain_db(peak, norm_gain_db);
    let presence = ((presence_peak - KEY_HIGHLIGHT_MIN_PEAK) / 0.28)
        .clamp(0.0, 1.0)
        .powf(0.7);
    let center = (0..note_count)
        .map(|idx| {
            let lo = idx.saturating_sub(KEY_LOCAL_WINDOW);
            let hi = (idx + KEY_LOCAL_WINDOW).min(note_count - 1);
            let local_ref = kth_largest(&boosted[lo..=hi], KEY_LOCAL_TARGET);
            let ratio = boosted[idx] / (local_ref + 1.0e-4);
            let prominence = smoothstep(KEY_LOCAL_RATIO_LO, KEY_LOCAL_RATIO_HI, ratio);
            let local_factor = KEY_LOCAL_FLOOR + (1.0 - KEY_LOCAL_FLOOR) * prominence;
            let hz = midi_to_hz(SPECTRUM_NOTE_MIN_MIDI + idx as u8);
            let strength = local_declutter_strength(hz);
            let eff = 1.0 - strength * (1.0 - local_factor);
            ((boosted[idx] / boosted_peak) * presence * eff).clamp(0.0, 1.0)
        })
        .collect();
    // 縦グラデの伸び factor: 各鍵の倍音エネルギーを基音に対する相対量 (0..1) にする。
    let even = (0..note_count)
        .map(|idx| (even_energy[idx] / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0))
        .collect();
    let odd = (0..note_count)
        .map(|idx| (odd_energy[idx] / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0))
        .collect();
    KeyboardVisuals { center, even, odd }
}

/// `values` の k 番目 (1-indexed) に大きい値。要素数 < k なら最小値 (= しきい値が低く全通過)。
fn kth_largest(values: &[f32], k: usize) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut buf: Vec<f32> = values.to_vec();
    buf.sort_unstable_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    buf[k.clamp(1, buf.len()) - 1]
}

/// 適応的局所正規化の適用強度。低域 (~KEY_LOCAL_FULL_HZ 以下)=1.0、高域 (~KEY_LOCAL_FADE_HZ 以上)
/// =KEY_LOCAL_MIN_STRENGTH。log 周波数で補間。
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

/// note index `idx` の偶数 / 奇数倍音の重み付きエネルギーを、知覚補正済み `notes` から求める。
/// オクターブ倍音 (2/4/8倍) は整数半音に乗るが、奇数倍音や 6倍は平均律からズレるので近傍 bin の
/// max で拾う。高次ほど寄与は小さい (重みは 1/n)。
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

fn midi_to_hz(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let frame_ms: f64 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(16.0);
    let rate = 48_000u32;
    let frames = samples.len() / 2;
    let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
    let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BAND_COUNT);
    let mut sustain = vec![0.0f32; note_count];
    let mut trail = vec![0.0f32; note_count];
    let mut out = std::io::BufWriter::new(std::fs::File::create(&args[2]).unwrap());
    let radius = rate as usize; // +-1 s, same as SPECTRUM_SNAPSHOT_RADIUS_SECS
    let mut t = 0.0f64;
    while (t * rate as f64) < frames as f64 {
        let center = (t * rate as f64).round() as usize;
        let start = center.saturating_sub(radius);
        let end = (center + radius + 1).min(frames);
        let window = &samples[start * 2..end * 2];
        let local_center = (center - start) as f64 / rate as f64;
        let analysis = analyzer.analyze_moving_window(window, rate, local_center);
        let sustained = update_keyboard_sustain(&analysis.notes, &mut sustain);
        let visuals = compute_keyboard_visuals(&sustained, note_count, 0.0);
        for i in 0..note_count {
            trail[i] = (trail[i] * KEY_HIGHLIGHT_DECAY).max(visuals.center[i]);
        }
        write!(out, "{:.4}", t).unwrap();
        for v in &analysis.notes {
            write!(out, ",{:.4}", v).unwrap();
        }
        for v in &trail {
            write!(out, ",{:.4}", v).unwrap();
        }
        writeln!(out).unwrap();
        t += frame_ms / 1000.0;
    }
}
