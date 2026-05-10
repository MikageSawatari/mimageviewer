//! 開発用 CLI — 実 scanner を動画ファイルで動かし結果を表示。
//!
//! cargo run --release --bin normalize_scan_test -- <video> [target_lufs_milli]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use mimageviewer::video::normalize_scanner::{NormalizeScanProgress, scan_audio_loudness};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: normalize_scan_test <path/to/video> [target_lufs_milli=-14000]");
        std::process::exit(2);
    }
    let path = Path::new(&args[1]);
    let target_milli: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(-14000);

    if let Err(e) = ffmpeg_the_third::init() {
        eprintln!("ffmpeg::init failed: {e}");
        std::process::exit(1);
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(NormalizeScanProgress::default());

    println!(
        "scanning {} (target = {} mLUFS)",
        path.display(),
        target_milli
    );
    let t0 = Instant::now();
    let result = scan_audio_loudness(path, target_milli, cancel, progress.clone());
    let elapsed = t0.elapsed();

    println!(
        "elapsed: {:.2}s  duration_ms (publish): {}  pts_processed_ms (last): {}  indeterminate: {}",
        elapsed.as_secs_f64(),
        progress.duration_ms.load(Ordering::Acquire),
        progress.pts_processed_ms.load(Ordering::Acquire),
        progress.indeterminate.load(Ordering::Acquire),
    );

    match result {
        Ok(r) => {
            let linear = 10.0_f64.powf(r.gain_db as f64 / 20.0);
            println!("\n=== result ===");
            println!("integrated_lufs : {:.2} LUFS", r.integrated_lufs);
            println!("true_peak_db    : {:.2} dBFS", r.true_peak_db);
            println!(
                "target          : {} mLUFS (= {:.3} LUFS)",
                r.target_lufs_milli,
                r.target_lufs_milli as f32 / 1000.0
            );
            println!("gain_db         : {:+.2} dB", r.gain_db);
            println!("gain (linear)   : {:.4}x", linear);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
