//! Standalone clock-free remote transcode benchmark.

use std::path::PathBuf;

use mimageviewer::video::clockless_transcode::{
    ClocklessQuality, ClocklessTranscodeControl, ClocklessTranscodeOptions, run_clockless_transcode,
};

fn main() {
    let options = match parse_options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            std::process::exit(2);
        }
    };
    ffmpeg_the_third::util::log::set_level(ffmpeg_the_third::util::log::Level::Warning);
    let control = ClocklessTranscodeControl::auto_releasing(options.segment_capacity)
        .expect("validated non-zero capacity");
    let cpu_before = process_cpu_seconds();
    let report = match run_clockless_transcode(&options, &control) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("clockless transcode failed: {error}");
            std::process::exit(1);
        }
    };
    let cpu_seconds = process_cpu_seconds()
        .zip(cpu_before)
        .map(|(after, before)| (after - before).max(0.0));
    let logical_cpus = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    let cpu_cores = cpu_seconds.map(|cpu| cpu / report.wall_secs.max(f64::EPSILON));
    let cpu_percent = cpu_cores.map(|cores| cores / logical_cpus as f64 * 100.0);

    println!(
        "result codec={} source={}x{} fps={}/{} decoder={} hw_decode={} encoder={} output={}x{} audio={} audio_codec={} source_secs={:.3} wall_secs={:.3} realtime_x={:.3} cpu_seconds={} cpu_cores={} cpu_percent={} logical_cpus={} packets={} video_frames={} audio_frames={} segments={} scale_profile_samples={}",
        report.source_codec,
        report.source_width,
        report.source_height,
        report.frame_rate_num,
        report.frame_rate_den,
        report.decoder_name,
        report.hardware_decode_active,
        report.encoder,
        report.output_width,
        report.output_height,
        report.include_audio,
        report.audio_codec.as_deref().unwrap_or("none"),
        report.source_secs_processed,
        report.wall_secs,
        report.realtime_multiple,
        display_optional(cpu_seconds),
        display_optional(cpu_cores),
        display_optional(cpu_percent),
        logical_cpus,
        report.input_packets,
        report.video_frames,
        report.audio_frames,
        report.completed_segments,
        report.scale_profile_samples,
    );
    println!(
        "stages demux={:.6} video_decode={:.6} video_download={:.6} scale_encode={:.6} profiled_swscale={:.6} audio_decode_resample={:.6} audio_encode={:.6} mux={:.6}",
        report.times.demux_secs,
        report.times.video_decode_secs,
        report.times.video_download_secs,
        report.times.video_scale_encode_secs,
        report.times.profiled_swscale_secs,
        report.times.audio_decode_resample_secs,
        report.times.audio_encode_secs,
        report.times.mux_secs,
    );
    println!("path={}", report.source_path.display());
}

fn parse_options() -> Result<ClocklessTranscodeOptions, String> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "missing <video>".to_owned())?;
    let mut options = ClocklessTranscodeOptions::benchmark(path);
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--seconds" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--seconds requires a value".to_owned())?;
                let seconds = value
                    .to_string_lossy()
                    .parse::<f64>()
                    .map_err(|error| format!("--seconds: {error}"))?;
                options.max_source_secs = if seconds == 0.0 { None } else { Some(seconds) };
            }
            "--no-audio" => options.include_audio = false,
            "--audio" => options.include_audio = true,
            "--sw-decode" => options.hw_decode = false,
            "--hw-decode" => options.hw_decode = true,
            "--profile-swscale" => options.profile_swscale = true,
            "--quality" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--quality requires minimum|low|standard|high".to_owned())?;
                options.quality = match value.to_string_lossy().as_ref() {
                    "minimum" => ClocklessQuality::Minimum,
                    "low" => ClocklessQuality::Low,
                    "standard" => ClocklessQuality::Standard,
                    "high" => ClocklessQuality::High,
                    other => return Err(format!("unknown quality: {other}")),
                };
            }
            "--segments" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--segments requires a value".to_owned())?;
                options.segment_capacity = value
                    .to_string_lossy()
                    .parse::<usize>()
                    .map_err(|error| format!("--segments: {error}"))?;
                if options.segment_capacity == 0 {
                    return Err("--segments must be greater than zero".to_owned());
                }
            }
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(options)
}

fn display_optional(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "unavailable".to_owned())
}

#[cfg(windows)]
fn process_cpu_seconds() -> Option<f64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }
    Some((filetime_ticks(kernel) + filetime_ticks(user)) as f64 / 10_000_000.0)
}

#[cfg(windows)]
fn filetime_ticks(value: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(not(windows))]
fn process_cpu_seconds() -> Option<f64> {
    None
}

fn print_usage() {
    eprintln!(
        "Usage: clockless_transcode_bench <video> [--seconds N|0] [--audio|--no-audio] \
         [--hw-decode|--sw-decode] [--quality minimum|low|standard|high] \
         [--segments N] [--profile-swscale]"
    );
}
