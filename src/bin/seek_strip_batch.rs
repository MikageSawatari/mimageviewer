//! Batch verifier for the production video seek-strip worker.
//!
//! Usage:
//!   cargo run --release --features dev-tools --bin seek_strip_batch -- <folder> [folder...]
//!   cargo run --release --features dev-tools --bin seek_strip_batch -- --limit 20 --json <folder>

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mimageviewer::video::seek_strip_batch::{
    AxisReport, BatchOptions, BatchReport, CellState, FileReport, discover_video_files, verify_file,
};

struct Cli {
    roots: Vec<PathBuf>,
    limit: Option<usize>,
    json: bool,
    options: BatchOptions,
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(Some(cli)) => cli,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!();
            print_usage();
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let discovery = discover_video_files(&cli.roots, cli.limit);
    let paths = discovery.files.clone();
    if !cli.json {
        println!(
            "discovered {} supported video file(s){}",
            paths.len(),
            if discovery.limit_reached {
                " (limit reached)"
            } else {
                ""
            }
        );
        for issue in &discovery.issues {
            println!("SCAN-ERROR {}: {}", issue.path, issue.reason);
        }
    }

    let mut files = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        if cli.json {
            eprintln!("[{}/{}] {}", index + 1, paths.len(), path.display());
        }
        let report = verify_file(path, &cli.options);
        if !cli.json {
            print_file_summary(&report);
        }
        files.push(report);
    }

    let report =
        BatchReport::from_files(&cli.roots, cli.options, discovery, files, started.elapsed());
    if cli.json {
        if let Err(error) = serde_json::to_writer_pretty(std::io::stdout().lock(), &report) {
            eprintln!("error: failed to serialize JSON: {error}");
            return ExitCode::from(2);
        }
        println!();
    } else {
        print_failure_details(&report);
        println!(
            "SUMMARY total={} pass={} fail={} skip={} failed_cells={} elapsed={:.1}s",
            report.files.len(),
            report.passed_files,
            report.failed_files,
            report.skipped_files,
            report.failed_cells,
            report.elapsed_ms / 1000.0,
        );
    }

    if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_args() -> Result<Option<Cli>, String> {
    let mut roots = Vec::new();
    let mut limit = None;
    let mut json = false;
    let mut options = BatchOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(None);
            }
            "--json" => json = true,
            "--software" => options.hardware_decode = false,
            "--limit" => {
                limit = Some(parse_value(
                    "--limit",
                    args.next().ok_or("--limit requires a value")?,
                )?);
                if limit == Some(0) {
                    return Err("--limit must be positive".to_string());
                }
            }
            "--visible-count" => {
                options.visible_count = parse_value(
                    "--visible-count",
                    args.next().ok_or("--visible-count requires a value")?,
                )?;
                if options.visible_count == 0 {
                    return Err("--visible-count must be positive".to_string());
                }
            }
            "--min-gap" => {
                options.minimum_gap_secs = parse_value(
                    "--min-gap",
                    args.next().ok_or("--min-gap requires a value")?,
                )?;
                if !options.minimum_gap_secs.is_finite() || options.minimum_gap_secs < 0.0 {
                    return Err("--min-gap must be a finite non-negative number".to_string());
                }
            }
            "--axis-timeout" => {
                options.axis_timeout_secs = parse_value(
                    "--axis-timeout",
                    args.next().ok_or("--axis-timeout requires a value")?,
                )?;
            }
            "--window-timeout" => {
                options.window_timeout_secs = parse_value(
                    "--window-timeout",
                    args.next().ok_or("--window-timeout requires a value")?,
                )?;
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unknown option: {argument}"));
            }
            _ => roots.push(PathBuf::from(argument)),
        }
    }
    if roots.is_empty() {
        return Err("provide at least one file or folder".to_string());
    }
    Ok(Some(Cli {
        roots,
        limit,
        json,
        options,
    }))
}

fn parse_value<T>(name: &str, value: String) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| format!("invalid {name} value {value:?}: {error}"))
}

fn print_usage() {
    println!(
        "Usage: seek_strip_batch [OPTIONS] <FILE-OR-FOLDER>...
         
         Recursively verifies mImageViewer video files with the app's own
         SeekStripThumbnailWorker and axis resolver.
         
         Options:
           --limit N           Verify at most N files
           --json              Emit one JSON report to stdout
           --software          Disable the initial hardware decode attempt
           --visible-count N   Cells per window (default: 11)
           --min-gap SECONDS   Keyframe adoption gap (default: 2.0)
           --axis-timeout N    Axis timeout in seconds (default: 15)
           --window-timeout N  Per-window timeout in seconds (default: 30)
           -h, --help          Show this help"
    );
}

fn print_file_summary(file: &FileReport) {
    if let Some(reason) = &file.skipped_reason {
        println!("SKIP {} reason={reason}", file.path);
        return;
    }
    if let Some(error) = &file.file_error {
        println!("FAIL {} axis=unresolved error={error}", file.path);
        return;
    }
    let Some(axis) = &file.axis else {
        println!("FAIL {} axis=missing", file.path);
        return;
    };
    let state = if file.passed() { "PASS" } else { "FAIL" };
    let windows = file
        .windows
        .iter()
        .map(|window| {
            format!(
                "{}:{}/{}",
                window.name, window.ready_count, window.failed_count
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "{state} {} axis={} reason={} index={} coverage={} monotonic={} adopted={} spacing={} decode={} windows=[{}]",
        file.path,
        axis.kind,
        axis.reason_code,
        axis.keyframe_count,
        format_optional(axis.index_coverage_percent, 1),
        format_monotonic(axis),
        axis.adopted_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_string()),
        format_spacing(axis),
        format_decode(file),
        windows,
    );
}

fn print_failure_details(report: &BatchReport) {
    let failures: Vec<_> = report
        .files
        .iter()
        .filter(|file| file.verification_failed())
        .collect();
    if failures.is_empty() {
        return;
    }
    println!();
    println!("FAILURE DETAILS");
    for file in failures {
        println!("{}", file.path);
        if let Some(axis) = &file.axis {
            println!(
                "  axis={} reason={} ({}) index={} coverage={} monotonic={} inversions={} adopted={} spacing={}",
                axis.kind,
                axis.reason_code,
                axis.reason,
                axis.keyframe_count,
                format_optional(axis.index_coverage_percent, 2),
                format_monotonic(axis),
                axis.index_inversion_count,
                axis.adopted_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                format_spacing(axis),
            );
        }
        println!("  decode={}", format_decode(file));
        if let Some(error) = &file.file_error {
            println!("  file_error={error}");
        }
        for window in file.windows.iter().filter(|window| window.failed_count > 0) {
            println!(
                "  window={} range={}..={} ready={} failed={}",
                window.name,
                window.actual_start_index,
                window.actual_end_index,
                window.ready_count,
                window.failed_count,
            );
            for cell in &window.cells {
                println!(
                    "    cell={} time={:.6}s state={}{}",
                    cell.index,
                    cell.time_secs,
                    if cell.state == CellState::Ready {
                        "ready"
                    } else {
                        "failed"
                    },
                    cell.failure
                        .as_deref()
                        .map(|failure| format!(" reason={failure}"))
                        .unwrap_or_default(),
                );
            }
        }
    }
}

fn format_monotonic(axis: &AxisReport) -> &'static str {
    match axis.index_timestamps_monotonic {
        Some(true) => "yes",
        Some(false) => "no",
        None => "n/a",
    }
}

fn format_spacing(axis: &AxisReport) -> String {
    axis.adopted_spacing
        .as_ref()
        .map(|spacing| {
            format!(
                "min={:.2}/p50={:.2}/p90={:.2}/max={:.2}/mean={:.2}s",
                spacing.minimum_secs,
                spacing.p50_secs,
                spacing.p90_secs,
                spacing.maximum_secs,
                spacing.mean_secs,
            )
        })
        .unwrap_or_else(|| "-".to_string())
}

fn format_decode(file: &FileReport) -> String {
    let Some(decode) = &file.decode else {
        return "n/a".to_string();
    };
    let initial = decode.initial_path.as_deref().unwrap_or("unopened");
    let final_path = decode.final_path.as_deref().unwrap_or("unopened");
    if let Some(reason) = &decode.full_frame_fallback_trigger {
        format!("{initial}->{final_path}/full-frame({reason})")
    } else if let Some(reason) = &decode.software_retry_failure {
        format!("{initial}->{final_path}/software-retry({reason})")
    } else {
        final_path.to_string()
    }
}

fn format_optional(value: Option<f64>, precision: usize) -> String {
    value
        .map(|value| format!("{value:.precision$}%"))
        .unwrap_or_else(|| "n/a".to_string())
}
