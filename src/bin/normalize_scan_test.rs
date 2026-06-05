//! 開発用 CLI — 実 scanner を動画ファイルで動かし結果を表示。
//!
//! cargo run --release --bin normalize_scan_test -- <video> [target_lufs_milli]
//! cargo run --release --bin normalize_scan_test -- <video> --provisional-after 5

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use mimageviewer::video::normalize_scanner::{
    NormalizeScanProgress, scan_audio_loudness, scan_audio_loudness_with_provisional,
};

struct Options {
    path: PathBuf,
    target_milli: i32,
    provisional_after_secs: Option<f64>,
}

fn main() {
    let opts = match parse_options() {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{e}");
            eprintln!(
                "Usage: normalize_scan_test <path/to/video> [target_lufs_milli=-14000] [--provisional-after SECS]"
            );
            std::process::exit(2);
        }
    };
    let path = Path::new(&opts.path);

    if let Err(e) = ffmpeg_the_third::init() {
        eprintln!("ffmpeg::init failed: {e}");
        std::process::exit(1);
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(NormalizeScanProgress::default());

    println!(
        "scanning {} (target = {} mLUFS, provisional_after={:?})",
        path.display(),
        opts.target_milli,
        opts.provisional_after_secs
    );
    let t0 = Instant::now();
    let mut provisional_count = 0_u32;
    let result = if let Some(after_secs) = opts.provisional_after_secs {
        let mut on_provisional = |r: mimageviewer::video::normalize_types::NormalizeResult| {
            provisional_count += 1;
            println!(
                "\n=== provisional #{} at {:.2}s wall ===",
                provisional_count,
                t0.elapsed().as_secs_f64()
            );
            print_result(&r);
        };
        scan_audio_loudness_with_provisional(
            path,
            opts.target_milli,
            cancel,
            progress.clone(),
            after_secs,
            &mut on_provisional,
        )
    } else {
        scan_audio_loudness(path, opts.target_milli, cancel, progress.clone())
    };
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
            println!("\n=== result ===");
            print_result(&r);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_options() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let mut path: Option<PathBuf> = None;
    let mut target_milli = -14000;
    let mut provisional_after_secs = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--provisional-after" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--provisional-after requires SECS".to_string())?;
                let secs: f64 = value
                    .parse()
                    .map_err(|_| format!("invalid --provisional-after value: {value}"))?;
                provisional_after_secs = Some(secs);
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => {
                target_milli = arg
                    .parse()
                    .map_err(|_| format!("invalid target_lufs_milli: {arg}"))?;
            }
        }
    }
    let path = path.ok_or_else(|| "missing video path".to_string())?;
    Ok(Options {
        path,
        target_milli,
        provisional_after_secs,
    })
}

fn print_result(r: &mimageviewer::video::normalize_types::NormalizeResult) {
    let linear = 10.0_f64.powf(r.gain_db as f64 / 20.0);
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
