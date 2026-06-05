use std::path::PathBuf;

use mimageviewer::video::decoder::{
    D3d11vaDecodeProbeMode, D3d11vaDecodeProbeReport, run_d3d11va_decode_probe,
};

fn main() {
    let mut path: Option<PathBuf> = None;
    let mut mode_arg = "all".to_string();
    let mut max_frames = Some(600_u64);
    let mut max_packets: Option<u64> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                return;
            }
            "--mode" => {
                mode_arg = args.next().unwrap_or_else(|| {
                    eprintln!("--mode requires shared, owned, sw, or all");
                    std::process::exit(2);
                });
            }
            "--frames" => {
                max_frames = Some(parse_u64_arg("--frames", args.next()));
                if max_frames == Some(0) {
                    max_frames = None;
                }
            }
            "--packets" => {
                max_packets = Some(parse_u64_arg("--packets", args.next()));
                if max_packets == Some(0) {
                    max_packets = None;
                }
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => {
                eprintln!("unexpected argument: {arg}");
                print_usage();
                std::process::exit(2);
            }
        }
    }

    let Some(path) = path else {
        print_usage();
        std::process::exit(2);
    };
    if !path.is_file() {
        eprintln!("path is not a file: {}", path.display());
        std::process::exit(1);
    }

    let modes = match parse_modes(&mode_arg) {
        Ok(modes) => modes,
        Err(e) => {
            eprintln!("{e}");
            print_usage();
            std::process::exit(2);
        }
    };

    let mut exit_code = 0;
    for mode in modes {
        println!("== mode={} ==", mode.label());
        match run_d3d11va_decode_probe(&path, mode, max_frames, max_packets) {
            Ok(report) => {
                print_report(&report);
                if !report.ok() {
                    exit_code = 1;
                }
            }
            Err(e) => {
                eprintln!("probe failed: {e}");
                exit_code = 1;
            }
        }
    }
    std::process::exit(exit_code);
}

fn parse_u64_arg(name: &str, value: Option<String>) -> u64 {
    let Some(value) = value else {
        eprintln!("{name} requires a number");
        std::process::exit(2);
    };
    value.parse::<u64>().unwrap_or_else(|_| {
        eprintln!("{name} requires a number, got {value}");
        std::process::exit(2);
    })
}

fn parse_modes(value: &str) -> Result<Vec<D3d11vaDecodeProbeMode>, String> {
    match value {
        "all" => Ok(vec![
            D3d11vaDecodeProbeMode::Shared,
            D3d11vaDecodeProbeMode::Owned,
            D3d11vaDecodeProbeMode::Software,
        ]),
        "shared" => Ok(vec![D3d11vaDecodeProbeMode::Shared]),
        "owned" => Ok(vec![D3d11vaDecodeProbeMode::Owned]),
        "sw" | "software" => Ok(vec![D3d11vaDecodeProbeMode::Software]),
        _ => Err(format!("unknown mode: {value}")),
    }
}

fn print_report(report: &D3d11vaDecodeProbeReport) {
    println!(
        "ok={} exit={} elapsed_ms={:.1} codec={} decoder={} hw_active={} \
         d3d11va_supported={} size={}x{} field_order={} stream_interlaced={} \
         packets={} frames={} hw_frames={} sw_frames={} send_errors={} \
         readback_failures={} packet_read_errors={}",
        report.ok(),
        report.exit_reason,
        report.elapsed_ms,
        report.codec_name,
        report.decoder_name,
        report.hw_decode_active,
        report.d3d11va_supported,
        report.width,
        report.height,
        report.field_order,
        report.stream_interlaced,
        report.packets,
        report.frames,
        report.hw_frames,
        report.sw_frames,
        report.send_packet_errors,
        report.readback_failures,
        report.packet_read_errors
    );
    if let Some(err) = report.first_send_packet_error.as_ref() {
        println!(
            "first_send_packet_error=\"{}\" at_packet={} at_frame={} elapsed_ms={:.1}",
            err,
            report.first_send_packet_error_at_packet.unwrap_or(0),
            report.first_send_packet_error_at_frame.unwrap_or(0),
            report.first_send_packet_error_elapsed_ms.unwrap_or(0.0)
        );
    }
    println!("d3d11va_config={}", report.d3d11va_config);
}

fn print_usage() {
    eprintln!(
        "Usage: probe_d3d11va_decode <video> [--mode shared|owned|sw|all] \
         [--frames N] [--packets N]\n\
         Defaults: --mode all --frames 600. Use --frames 0 for full decode."
    );
}
