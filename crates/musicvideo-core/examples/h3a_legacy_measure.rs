//! Headless pre-H3a baseline (legacy adapter is intentionally retained).
use musicvideo_core::{Smoothing, SpectrumSource};
use std::time::Instant;

fn main() {
    let rate = 48_000;
    let pcm: Vec<f32> = (0..300 * rate)
        .flat_map(|n| {
            let t = n as f32 / rate as f32;
            let v = (std::f32::consts::TAU * (55.0 * t + 2.0 * t * t)).sin() * 0.5;
            [v, v]
        })
        .collect();
    let mut source = SpectrumSource::new(rate, 118, Smoothing::default()).unwrap();
    let started = Instant::now();
    for n in 0..=14_400 {
        std::hint::black_box(source.full_frame_at(&pcm, n as f64 / 60.0, 0.0));
    }
    println!(
        "legacy_reference_0_to_240_secs={:.6}",
        started.elapsed().as_secs_f64()
    );
    let mut fresh = SpectrumSource::new(rate, 118, Smoothing::default()).unwrap();
    let started = Instant::now();
    std::hint::black_box(fresh.full_frame_at(&pcm, 240.0, 0.0));
    println!(
        "legacy_direct_seek_secs={:.6} (different semantics)",
        started.elapsed().as_secs_f64()
    );
}
