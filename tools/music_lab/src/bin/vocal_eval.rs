use std::env;
use std::fs::File;
use std::path::{Path, PathBuf};

use music_core::{
    AnalysisConfig, AudioStreamInfo, DecodedAudio, TimelineAnalysis, analyze_stereo_timeline,
};
use serde::Deserialize;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::conv::IntoSample;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const DEFAULT_THRESHOLDS: &[f32] = &[0.10, 0.15, 0.20, 0.25, 0.30, 0.35, 0.40, 0.50, 0.60];

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage(&args);
        return Ok(());
    }
    let labels_path = PathBuf::from(&args[1]);
    let options = parse_options(&args[2..])?;
    let labels = load_label_set(&labels_path)?;
    if labels.tracks.is_empty() {
        return Err("label file has no tracks".to_string());
    }

    let base_dir = labels_path.parent().unwrap_or_else(|| Path::new("."));
    let mut aggregate = vec![EvalAccum::default(); options.thresholds.len()];
    println!("file\tthreshold\tprecision\trecall\tf1\ttp_s\tfp_s\tfn_s\ttn_s");
    for track in &labels.tracks {
        let path = resolve_track_path(base_dir, &track.path);
        eprintln!("analyzing {}", path.display());
        let decoded = decode_audio_file(&path)?;
        let analysis = analyze_stereo_timeline(
            &decoded.stereo_samples,
            decoded.info.sample_rate,
            AnalysisConfig::default(),
        );
        let truth = build_truth_mask(&analysis, &track.vocal, &track.ignore);
        if let Some(threshold) = options.dump_segments {
            dump_segments(&analysis, &truth, threshold);
        }
        for (idx, threshold) in options.thresholds.iter().copied().enumerate() {
            let metrics = evaluate_threshold(&analysis, &truth, threshold);
            aggregate[idx].add(metrics);
            println!(
                "{}\t{threshold:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.2}\t{:.2}\t{:.2}\t{:.2}",
                path.display(),
                metrics.precision(),
                metrics.recall(),
                metrics.f1(),
                metrics.tp,
                metrics.fp,
                metrics.fn_,
                metrics.tn
            );
        }
    }

    if labels.tracks.len() > 1 {
        println!("-- aggregate --");
        for (idx, threshold) in options.thresholds.iter().copied().enumerate() {
            let metrics = aggregate[idx].metrics();
            println!(
                "ALL\t{threshold:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.2}\t{:.2}\t{:.2}\t{:.2}",
                metrics.precision(),
                metrics.recall(),
                metrics.f1(),
                metrics.tp,
                metrics.fp,
                metrics.fn_,
                metrics.tn
            );
        }
    }
    Ok(())
}

fn print_usage(args: &[String]) {
    let exe = args.first().map(String::as_str).unwrap_or("vocal_eval");
    println!("Usage: {exe} <labels.json> [--thresholds 0.2,0.3,0.4]");
    println!("       {exe} <labels.json> [--dump-segments 0.1]");
    println!(
        "Labels JSON: {{ \"tracks\": [{{ \"path\": \"song.mp4\", \"vocal\": [{{ \"start\": 12.3, \"end\": 42.0 }}] }}] }}"
    );
}

#[derive(Debug)]
struct EvalOptions {
    thresholds: Vec<f32>,
    dump_segments: Option<f32>,
}

fn parse_options(args: &[String]) -> Result<EvalOptions, String> {
    let mut thresholds = DEFAULT_THRESHOLDS.to_vec();
    let mut dump_segments = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--thresholds" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("--thresholds requires a comma separated value".to_string());
                };
                thresholds = value
                    .split(',')
                    .map(|part| {
                        part.trim()
                            .parse::<f32>()
                            .map(|value| value.clamp(0.0, 1.0))
                            .map_err(|e| format!("invalid threshold '{part}': {e}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if thresholds.is_empty() {
                    return Err("--thresholds produced no values".to_string());
                }
                i += 2;
            }
            "--dump-segments" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("--dump-segments requires a threshold".to_string());
                };
                dump_segments = Some(
                    value
                        .parse::<f32>()
                        .map(|value| value.clamp(0.0, 1.0))
                        .map_err(|e| format!("invalid dump threshold '{value}': {e}"))?,
                );
                i += 2;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    thresholds.sort_by(f32::total_cmp);
    thresholds.dedup_by(|a, b| (*a - *b).abs() < f32::EPSILON);
    Ok(EvalOptions {
        thresholds,
        dump_segments,
    })
}

fn load_label_set(path: &Path) -> Result<LabelSet, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    serde_json::from_reader(file).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn resolve_track_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn build_truth_mask(
    analysis: &TimelineAnalysis,
    vocal: &[TimeSpan],
    ignore: &[TimeSpan],
) -> Vec<Option<bool>> {
    analysis
        .bins
        .iter()
        .map(|bin| {
            let start = bin.start_secs;
            let end = bin.start_secs + bin.duration_secs;
            if ignore.iter().any(|span| span.overlaps(start, end)) {
                None
            } else {
                Some(vocal.iter().any(|span| span.overlaps(start, end)))
            }
        })
        .collect()
}

fn evaluate_threshold(
    analysis: &TimelineAnalysis,
    truth: &[Option<bool>],
    threshold: f32,
) -> Metrics {
    let mut metrics = Metrics::default();
    for (bin, truth) in analysis.bins.iter().zip(truth) {
        let Some(expected) = truth else {
            continue;
        };
        let predicted = bin.vocal_score >= threshold;
        let secs = bin.duration_secs as f32;
        match (*expected, predicted) {
            (true, true) => metrics.tp += secs,
            (false, true) => metrics.fp += secs,
            (true, false) => metrics.fn_ += secs,
            (false, false) => metrics.tn += secs,
        }
    }
    metrics
}

fn dump_segments(analysis: &TimelineAnalysis, truth: &[Option<bool>], threshold: f32) {
    eprintln!("-- predicted segments @ {threshold:.3} --");
    for segment in collect_segments(analysis, |idx, bin| {
        truth.get(idx).and_then(|v| *v).is_some() && bin.vocal_score >= threshold
    }) {
        eprintln!(
            "pred\t{:.3}\t{:.3}\t{:.3}\tpeak={:.3}\tavg={:.3}",
            segment.start,
            segment.end,
            segment.end - segment.start,
            segment.peak,
            segment.avg
        );
    }
    eprintln!("-- truth segments --");
    for segment in collect_segments(analysis, |idx, _bin| truth.get(idx) == Some(&Some(true))) {
        eprintln!(
            "truth\t{:.3}\t{:.3}\t{:.3}",
            segment.start,
            segment.end,
            segment.end - segment.start
        );
    }
}

struct DumpSegment {
    start: f64,
    end: f64,
    peak: f32,
    avg: f32,
}

fn collect_segments(
    analysis: &TimelineAnalysis,
    mut is_active: impl FnMut(usize, &music_core::WaveformBin) -> bool,
) -> Vec<DumpSegment> {
    let mut segments = Vec::new();
    let mut start = None;
    let mut last_end = 0.0;
    let mut peak = 0.0_f32;
    let mut sum = 0.0_f32;
    let mut count = 0usize;
    for (idx, bin) in analysis.bins.iter().enumerate() {
        let active = is_active(idx, bin);
        if active {
            if start.is_none() {
                start = Some(bin.start_secs);
                peak = 0.0;
                sum = 0.0;
                count = 0;
            }
            last_end = bin.start_secs + bin.duration_secs;
            peak = peak.max(bin.vocal_score);
            sum += bin.vocal_score;
            count += 1;
        } else if let Some(segment_start) = start.take() {
            segments.push(DumpSegment {
                start: segment_start,
                end: last_end,
                peak,
                avg: sum / count.max(1) as f32,
            });
        }
    }
    if let Some(segment_start) = start {
        segments.push(DumpSegment {
            start: segment_start,
            end: last_end,
            peak,
            avg: sum / count.max(1) as f32,
        });
    }
    segments
}

fn decode_audio_file(path: &Path) -> Result<DecodedAudio, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("probe {}: {e}", path.display()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track.codec_params.codec != CODEC_TYPE_NULL
                && track.codec_params.sample_rate.is_some()
                && track.codec_params.channels.is_some()
        })
        .or_else(|| {
            format.tracks().iter().find(|track| {
                track.codec_params.codec != CODEC_TYPE_NULL
                    && (track.codec_params.sample_rate.is_some()
                        || track.codec_params.channels.is_some())
            })
        })
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| format!("no supported audio track: {}", path.display()))?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder {}: {e}", path.display()))?;

    let mut stereo_samples = Vec::new();
    let mut stream_info = AudioStreamInfo::default();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => {
                return Err("decoder reset required; not handled in vocal_eval".to_string());
            }
            Err(err) => return Err(format!("packet {}: {err}", path.display())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(format!("decode {}: {err}", path.display())),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();
        stream_info.sample_rate = spec.rate;
        stream_info.channels = channels as u16;
        for frame in samples.chunks(channels) {
            let left: f32 = frame.first().copied().unwrap_or(0.0).into_sample();
            let right: f32 = frame.get(1).copied().unwrap_or(left).into_sample();
            stereo_samples.push(left.clamp(-1.0, 1.0));
            stereo_samples.push(right.clamp(-1.0, 1.0));
        }
    }
    stream_info.duration_secs =
        stereo_samples.len() as f64 / 2.0 / stream_info.sample_rate.max(1) as f64;
    if stereo_samples.is_empty() {
        return Err(format!("no decoded samples: {}", path.display()));
    }
    Ok(DecodedAudio {
        info: stream_info,
        stereo_samples,
    })
}

#[derive(Debug, Deserialize)]
struct LabelSet {
    tracks: Vec<TrackLabels>,
}

#[derive(Debug, Deserialize)]
struct TrackLabels {
    path: PathBuf,
    #[serde(default)]
    vocal: Vec<TimeSpan>,
    #[serde(default)]
    ignore: Vec<TimeSpan>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct TimeSpan {
    start: f64,
    end: f64,
}

impl TimeSpan {
    fn overlaps(self, start: f64, end: f64) -> bool {
        self.start < end && self.end > start
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct EvalAccum {
    metrics: Metrics,
}

impl EvalAccum {
    fn add(&mut self, metrics: Metrics) {
        self.metrics.tp += metrics.tp;
        self.metrics.fp += metrics.fp;
        self.metrics.fn_ += metrics.fn_;
        self.metrics.tn += metrics.tn;
    }

    fn metrics(self) -> Metrics {
        self.metrics
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Metrics {
    tp: f32,
    fp: f32,
    fn_: f32,
    tn: f32,
}

impl Metrics {
    fn precision(self) -> f32 {
        self.tp / (self.tp + self.fp).max(1.0e-6)
    }

    fn recall(self) -> f32 {
        self.tp / (self.tp + self.fn_).max(1.0e-6)
    }

    fn f1(self) -> f32 {
        let precision = self.precision();
        let recall = self.recall();
        2.0 * precision * recall / (precision + recall).max(1.0e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_span_overlap_treats_touching_edges_as_non_overlap() {
        let span = TimeSpan {
            start: 10.0,
            end: 20.0,
        };

        assert!(span.overlaps(12.0, 14.0));
        assert!(span.overlaps(5.0, 11.0));
        assert!(!span.overlaps(20.0, 21.0));
        assert!(!span.overlaps(1.0, 10.0));
    }

    #[test]
    fn metrics_are_duration_weighted() {
        let metrics = Metrics {
            tp: 8.0,
            fp: 2.0,
            fn_: 4.0,
            tn: 16.0,
        };

        assert!((metrics.precision() - 0.8).abs() < 1.0e-6);
        assert!((metrics.recall() - (8.0 / 12.0)).abs() < 1.0e-6);
        assert!((metrics.f1() - (2.0 * 0.8 * (8.0 / 12.0) / (0.8 + 8.0 / 12.0))).abs() < 1.0e-6);
    }

    #[test]
    fn parse_thresholds_sorts_and_deduplicates() {
        let args = vec!["--thresholds".to_string(), "0.4,0.2,0.2".to_string()];
        let options = parse_options(&args).expect("thresholds should parse");

        assert_eq!(options.thresholds, vec![0.2, 0.4]);
    }

    #[test]
    fn parse_options_accepts_dump_segments() {
        let args = vec!["--dump-segments".to_string(), "0.123".to_string()];
        let options = parse_options(&args).expect("options should parse");

        assert_eq!(options.dump_segments, Some(0.123));
    }
}
