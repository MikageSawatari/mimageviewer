//! Release benchmark for the production HarmNet frontend/inference.
//! cargo run -p music-core --release --example harmnet_dump
//! PCM copy and complete spectrum-worker request are benchmarked by the application test.
use music_core::pitch::{HARMNET_LONG_WINDOW_SAMPLES, HarmNetTracker};
use std::hint::black_box;
use std::time::Instant;

fn main() {
    if cfg!(debug_assertions) {
        eprintln!("Use --release for performance measurements.");
    }
    let first = Instant::now();
    let mut tracker = HarmNetTracker::new(48_000);
    println!(
        "cold initialization (weights + FFT plans): {:.3} ms",
        first.elapsed().as_secs_f64() * 1e3
    );
    let started = Instant::now();
    for _ in 0..10 {
        black_box(HarmNetTracker::new(48_000));
    }
    println!(
        "warm initialization: {:.3} ms",
        started.elapsed().as_secs_f64() * 100.0
    );
    let signal: Vec<f32> = (0..HARMNET_LONG_WINDOW_SAMPLES)
        .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 48_000.0).sin() * 0.1)
        .collect();
    for _ in 0..10 {
        tracker.push_frame(black_box(&signal));
    }
    let frames = 500;
    let started = Instant::now();
    for _ in 0..frames {
        tracker.push_frame(black_box(&signal));
        black_box(tracker.probabilities(3));
    }
    println!(
        "frontend + inference frame: {:.3} ms",
        started.elapsed().as_secs_f64() * 1e3 / frames as f64
    );
    let runs = 10;
    let started = Instant::now();
    for _ in 0..runs {
        tracker.reset();
        for _ in 0..58 {
            tracker.push_frame(black_box(&signal));
        }
        black_box(tracker.probabilities(3));
    }
    println!(
        "58-frame warmup (inference only): {:.3} ms",
        started.elapsed().as_secs_f64() * 1e3 / runs as f64
    );
}
