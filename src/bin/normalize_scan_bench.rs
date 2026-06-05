//! Development CLI for measuring audio-normalize scan throughput on real folders.
//!
//! Example:
//!   cargo run --release --bin normalize_scan_bench -- D:\home\18\dms2 --sample 12 --jobs 1
//!   cargo run --release --bin normalize_scan_bench -- D:\home\18\dms2 --sample 12 --jobs 4 --csv bench.csv

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ffmpeg_the_third as ffmpeg;
use mimageviewer::video::normalize_scanner::{NormalizeScanProgress, scan_audio_loudness};

const DEFAULT_TARGET_MILLI: i32 = -14_000;
const DEFAULT_SAMPLE: usize = 8;
const DEFAULT_JOBS: usize = 1;
const DEFAULT_SEED: u64 = 0x6d49_565f_4e4f_524d;
const DEFAULT_EXTS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "wmv", "mpg", "mpeg", "m4v", "webm", "ts", "m2ts",
];

fn main() {
    let opts = match Options::parse(std::env::args_os().skip(1)) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("{err}");
            eprintln!();
            eprintln!("{}", Options::usage());
            std::process::exit(2);
        }
    };

    if let Err(e) = ffmpeg::init() {
        eprintln!("ffmpeg::init failed: {e}");
        std::process::exit(1);
    }
    ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Quiet);

    let started = Instant::now();
    println!(
        "enumerating videos under {} (sample={}, seed={}, jobs={})",
        opts.root.display(),
        opts.sample,
        opts.seed,
        opts.jobs
    );
    let sample = match sample_videos(&opts) {
        Ok(sample) => sample,
        Err(err) => {
            eprintln!("enumeration failed: {err}");
            std::process::exit(1);
        }
    };
    if sample.paths.is_empty() {
        eprintln!("no matching video files found");
        std::process::exit(1);
    }
    println!(
        "found {} matching files, selected {} in {:.2}s",
        sample.seen,
        sample.paths.len(),
        started.elapsed().as_secs_f64()
    );

    let scan_started = Instant::now();
    let results = run_scan_jobs(&opts, sample.paths);
    let scan_wall = scan_started.elapsed();
    print_summary(&results, scan_wall);

    if let Some(csv_path) = opts.csv.as_ref() {
        if let Err(err) = write_csv(csv_path, &results) {
            eprintln!("failed to write CSV {}: {err}", csv_path.display());
            std::process::exit(1);
        }
        println!("csv: {}", csv_path.display());
    }
}

#[derive(Debug)]
struct Options {
    root: PathBuf,
    sample: usize,
    jobs: usize,
    seed: u64,
    target_milli: i32,
    exts: Vec<String>,
    max_depth: usize,
    csv: Option<PathBuf>,
}

impl Options {
    fn parse<I>(mut args: I) -> Result<Self, String>
    where
        I: Iterator<Item = std::ffi::OsString>,
    {
        let root = args
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| "missing <root>".to_string())?;

        let mut opts = Self {
            root,
            sample: DEFAULT_SAMPLE,
            jobs: DEFAULT_JOBS,
            seed: DEFAULT_SEED,
            target_milli: DEFAULT_TARGET_MILLI,
            exts: DEFAULT_EXTS.iter().map(|s| s.to_string()).collect(),
            max_depth: usize::MAX,
            csv: None,
        };

        while let Some(arg) = args.next() {
            let arg_s = arg.to_string_lossy();
            match arg_s.as_ref() {
                "--sample" => {
                    opts.sample = parse_next_usize(&mut args, "--sample")?;
                    if opts.sample == 0 {
                        return Err("--sample must be > 0".to_string());
                    }
                }
                "--jobs" => {
                    opts.jobs = parse_next_usize(&mut args, "--jobs")?;
                    if opts.jobs == 0 {
                        return Err("--jobs must be > 0".to_string());
                    }
                }
                "--seed" => opts.seed = parse_next_u64(&mut args, "--seed")?,
                "--target-lufs-milli" => {
                    opts.target_milli = parse_next_i32(&mut args, "--target-lufs-milli")?;
                }
                "--exts" => {
                    let raw = args
                        .next()
                        .ok_or_else(|| "--exts requires a value".to_string())?;
                    opts.exts = raw
                        .to_string_lossy()
                        .split(',')
                        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if opts.exts.is_empty() {
                        return Err("--exts must contain at least one extension".to_string());
                    }
                }
                "--max-depth" => opts.max_depth = parse_next_usize(&mut args, "--max-depth")?,
                "--csv" => {
                    opts.csv = Some(PathBuf::from(
                        args.next()
                            .ok_or_else(|| "--csv requires a path".to_string())?,
                    ));
                }
                "--help" | "-h" => return Err(Self::usage().to_string()),
                other => return Err(format!("unknown option: {other}")),
            }
        }

        Ok(opts)
    }

    fn usage() -> &'static str {
        "Usage: normalize_scan_bench <root> [--sample N] [--jobs N] [--seed U64] \
         [--target-lufs-milli N] [--exts mp4,mkv,mov] [--max-depth N] [--csv out.csv]"
    }
}

fn parse_next_usize<I>(args: &mut I, name: &str) -> Result<usize, String>
where
    I: Iterator<Item = std::ffi::OsString>,
{
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))?
        .to_string_lossy()
        .parse::<usize>()
        .map_err(|e| format!("{name}: {e}"))
}

fn parse_next_u64<I>(args: &mut I, name: &str) -> Result<u64, String>
where
    I: Iterator<Item = std::ffi::OsString>,
{
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))?
        .to_string_lossy()
        .parse::<u64>()
        .map_err(|e| format!("{name}: {e}"))
}

fn parse_next_i32<I>(args: &mut I, name: &str) -> Result<i32, String>
where
    I: Iterator<Item = std::ffi::OsString>,
{
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))?
        .to_string_lossy()
        .parse::<i32>()
        .map_err(|e| format!("{name}: {e}"))
}

struct SampledVideos {
    seen: u64,
    paths: Vec<PathBuf>,
}

fn sample_videos(opts: &Options) -> Result<SampledVideos, String> {
    let mut rng = SplitMix64::new(opts.seed);
    let mut selected: Vec<PathBuf> = Vec::with_capacity(opts.sample);
    let mut seen = 0_u64;
    let mut stack = vec![(opts.root.clone(), 0_usize)];

    while let Some((dir, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("skip unreadable dir {}: {err}", dir.display());
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    eprintln!("skip unreadable entry in {}: {err}", dir.display());
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(err) => {
                    eprintln!("skip file_type {}: {err}", entry.path().display());
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                if depth < opts.max_depth {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !file_type.is_file() || !matches_ext(&path, &opts.exts) {
                continue;
            }

            seen = seen.saturating_add(1);
            if selected.len() < opts.sample {
                selected.push(path);
            } else {
                let replace = rng.gen_range(seen);
                if replace < opts.sample as u64 {
                    selected[replace as usize] = path;
                }
            }
        }
    }

    shuffle(&mut selected, &mut rng);
    Ok(SampledVideos {
        seen,
        paths: selected,
    })
}

fn matches_ext(path: &Path, exts: &[String]) -> bool {
    let Some(ext) = path.extension().and_then(OsStr::to_str) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    exts.iter().any(|candidate| candidate == &ext)
}

fn shuffle(paths: &mut [PathBuf], rng: &mut SplitMix64) {
    for i in (1..paths.len()).rev() {
        let j = rng.gen_range((i + 1) as u64) as usize;
        paths.swap(i, j);
    }
}

#[derive(Clone, Debug)]
struct BenchResult {
    idx: usize,
    worker_id: usize,
    path: PathBuf,
    file_size: u64,
    elapsed: Duration,
    duration_ms: u64,
    pts_processed_ms: u64,
    ok: bool,
    gain_db: Option<f32>,
    integrated_lufs: Option<f32>,
    true_peak_db: Option<f32>,
    error: Option<String>,
}

fn run_scan_jobs(opts: &Options, paths: Vec<PathBuf>) -> Vec<BenchResult> {
    let total = paths.len();
    let queue = Arc::new(Mutex::new(
        paths.into_iter().enumerate().collect::<VecDeque<_>>(),
    ));
    let (tx, rx) = mpsc::channel::<BenchResult>();

    for worker_id in 0..opts.jobs.min(total) {
        let queue = Arc::clone(&queue);
        let tx = tx.clone();
        let target_milli = opts.target_milli;
        std::thread::Builder::new()
            .name(format!("norm-bench-{worker_id}"))
            .spawn(move || {
                loop {
                    let job = {
                        let mut queue = queue.lock().unwrap();
                        queue.pop_front()
                    };
                    let Some((idx, path)) = job else {
                        break;
                    };
                    let result = scan_one(idx, worker_id, path, target_milli);
                    let _ = tx.send(result);
                }
            })
            .expect("spawn norm bench worker");
    }
    drop(tx);

    let mut results = Vec::with_capacity(total);
    while let Ok(result) = rx.recv() {
        print_result_line(results.len() + 1, total, &result);
        results.push(result);
    }
    results.sort_by_key(|r| r.idx);
    results
}

fn scan_one(idx: usize, worker_id: usize, path: PathBuf, target_milli: i32) -> BenchResult {
    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(NormalizeScanProgress::default());
    let t0 = Instant::now();
    let result = scan_audio_loudness(&path, target_milli, cancel, Arc::clone(&progress));
    let elapsed = t0.elapsed();
    let duration_ms = progress.duration_ms.load(Ordering::Acquire);
    let pts_processed_ms = progress.pts_processed_ms.load(Ordering::Acquire);

    match result {
        Ok(r) => BenchResult {
            idx,
            worker_id,
            path,
            file_size,
            elapsed,
            duration_ms,
            pts_processed_ms,
            ok: true,
            gain_db: Some(r.gain_db),
            integrated_lufs: Some(r.integrated_lufs),
            true_peak_db: Some(r.true_peak_db),
            error: None,
        },
        Err(e) => BenchResult {
            idx,
            worker_id,
            path,
            file_size,
            elapsed,
            duration_ms,
            pts_processed_ms,
            ok: false,
            gain_db: None,
            integrated_lufs: None,
            true_peak_db: None,
            error: Some(e.to_string()),
        },
    }
}

fn print_result_line(done: usize, total: usize, result: &BenchResult) {
    let speed = realtime_factor(result);
    let name = result
        .path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("<non-utf8>");
    if result.ok {
        println!(
            "[{done:>3}/{total:<3}] worker={} elapsed={:>7.2}s media={:>8.1}s speed={:>7.1}x size={:>8.1}MiB {}",
            result.worker_id,
            result.elapsed.as_secs_f64(),
            result.duration_ms as f64 / 1000.0,
            speed.unwrap_or(0.0),
            result.file_size as f64 / (1024.0 * 1024.0),
            name
        );
    } else {
        println!(
            "[{done:>3}/{total:<3}] worker={} elapsed={:>7.2}s ERROR {}: {}",
            result.worker_id,
            result.elapsed.as_secs_f64(),
            name,
            result.error.as_deref().unwrap_or("unknown error")
        );
    }
}

fn print_summary(results: &[BenchResult], scan_wall: Duration) {
    let ok: Vec<&BenchResult> = results.iter().filter(|r| r.ok).collect();
    let total_elapsed: f64 = results.iter().map(|r| r.elapsed.as_secs_f64()).sum();
    let wall_media_secs: f64 = ok.iter().map(|r| r.duration_ms as f64 / 1000.0).sum();
    let total_size_mib: f64 = results
        .iter()
        .map(|r| r.file_size as f64 / (1024.0 * 1024.0))
        .sum();
    let mut speeds: Vec<f64> = ok.iter().filter_map(|r| realtime_factor(r)).collect();
    speeds.sort_by(f64::total_cmp);

    println!();
    println!("=== summary ===");
    println!("files       : {} ok / {} total", ok.len(), results.len());
    println!("input size  : {:.1} MiB", total_size_mib);
    println!("media time  : {:.1}s", wall_media_secs);
    println!("batch wall  : {:.2}s", scan_wall.as_secs_f64());
    println!("sum elapsed : {:.2}s", total_elapsed);
    if scan_wall.as_secs_f64() > 0.0 {
        println!(
            "batch speed : {:.1}x",
            wall_media_secs / scan_wall.as_secs_f64()
        );
    }
    if !speeds.is_empty() {
        println!(
            "speed x     : min {:.1} / p50 {:.1} / p90 {:.1} / max {:.1}",
            speeds[0],
            percentile(&speeds, 0.50),
            percentile(&speeds, 0.90),
            speeds[speeds.len() - 1]
        );
    }
    let errors = results.iter().filter(|r| !r.ok).count();
    if errors > 0 {
        println!("errors      : {errors}");
    }
}

fn realtime_factor(result: &BenchResult) -> Option<f64> {
    let elapsed = result.elapsed.as_secs_f64();
    if elapsed <= 0.0 || result.duration_ms == 0 {
        return None;
    }
    Some((result.duration_ms as f64 / 1000.0) / elapsed)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn write_csv(path: &Path, results: &[BenchResult]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    writeln!(
        file,
        "idx,worker_id,ok,elapsed_secs,duration_ms,pts_processed_ms,realtime_factor,file_size,path,gain_db,integrated_lufs,true_peak_db,error"
    )?;
    for r in results {
        writeln!(
            file,
            "{},{},{},{:.6},{},{},{:.6},{},\"{}\",{},{},{},\"{}\"",
            r.idx,
            r.worker_id,
            r.ok,
            r.elapsed.as_secs_f64(),
            r.duration_ms,
            r.pts_processed_ms,
            realtime_factor(r).unwrap_or(0.0),
            r.file_size,
            csv_escape(&r.path.to_string_lossy()),
            opt_f32(r.gain_db),
            opt_f32(r.integrated_lufs),
            opt_f32(r.true_peak_db),
            csv_escape(r.error.as_deref().unwrap_or(""))
        )?;
    }
    Ok(())
}

fn opt_f32(v: Option<f32>) -> String {
    v.map(|v| format!("{v:.6}")).unwrap_or_default()
}

fn csv_escape(s: &str) -> String {
    s.replace('"', "\"\"")
}

#[derive(Clone, Copy)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn gen_range(&mut self, upper_exclusive: u64) -> u64 {
        if upper_exclusive <= 1 {
            return 0;
        }
        self.next_u64() % upper_exclusive
    }
}
