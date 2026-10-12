//! Pitch-lab harness: run mIV's keyboard pipeline (SpectrumAnalyzer + ui_music_spectrum
//! keyboard visuals, copied verbatim) over a raw f32 interleaved stereo 48 kHz file.
//! Usage: keyboard_dump <in.f32> <out.csv> [frame_ms]
#![allow(dead_code, clippy::all)]
use music_core::{SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalyzer, SPECTRUM_BAND_COUNT};
use std::io::Write;

//@@EXTRACTED@@

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let samples: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
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
        for v in &analysis.notes { write!(out, ",{:.4}", v).unwrap(); }
        for v in &trail { write!(out, ",{:.4}", v).unwrap(); }
        writeln!(out).unwrap();
        t += frame_ms / 1000.0;
    }
}
