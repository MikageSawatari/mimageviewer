use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use comic_core::{FontSet, LoadedFont};
use mimageviewer::video::mvexport::{
    EncoderPreference, MusicVideoExportRequest, export_music_video, verify_exported_mp4,
};
use musicvideo_core::{
    Assets, DEFAULT_BACKGROUND_IMAGE_ID, DEFAULT_FONT_KEY, ImageAsset, ImageId, default_project,
};

const DEFAULT_VIDEO_BITRATE_BPS: u32 = 12_000_000;

struct Args {
    audio: PathBuf,
    image: PathBuf,
    output: PathBuf,
    seconds: Option<f64>,
    encoder: EncoderPreference,
    video_bitrate_bps: u32,
    frame_hash: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("mvexport_cli: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let probe = mimageviewer::audio_decode::probe_audio_file(&args.audio)
        .map_err(|error| format!("音声情報を取得できません: {error}"))?;
    let duration_secs = args.seconds.unwrap_or(probe.duration_secs);
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        return Err("書き出し秒数は 0 より大きい有限値にしてください".to_owned());
    }
    let assets = load_assets(&args.image)?;
    let project = default_project(duration_secs);
    let output = project.output;
    let cancel = AtomicBool::new(false);
    let last_reported = AtomicU64::new(0);
    let progress = |completed: u64, total: u64| {
        let last = last_reported.load(Ordering::Relaxed);
        if completed == total || completed.saturating_sub(last) >= 600 {
            last_reported.store(completed, Ordering::Relaxed);
            eprintln!("進捗: {completed}/{total} フレーム");
        }
    };
    let summary = export_music_video(
        MusicVideoExportRequest {
            project,
            assets,
            audio_path: args.audio,
            output_path: args.output.clone(),
            encoder: args.encoder,
            video_bitrate_bps: args.video_bitrate_bps,
            compute_frame_hash: args.frame_hash,
        },
        &progress,
        &cancel,
    )
    .map_err(|error| error.to_string())?;

    println!(
        "書き出し: frames={} resolution={}x{} fps={:.3} encoder={} render_mean_ms={:.3} render_p95_ms={:.3} convert_mean_ms={:.3} encode_mean_ms={:.3} wall_secs={:.3} realtime_ratio={:.3} rgba_sha256={}",
        summary.frames,
        output.width,
        output.height,
        output.fps,
        summary.encoder,
        summary.render_mean_ms,
        summary.render_p95_ms,
        summary.convert_mean_ms,
        summary.encode_mean_ms,
        summary.wall_secs,
        summary.realtime_ratio,
        summary.rgba_sha256.as_deref().unwrap_or("未計算")
    );
    println!(
        "区間外内訳: audio_decode_secs={:.3} aac_encode_secs={:.3} mux_secs={:.3} faststart_secs={:.3} rename_secs={:.6} frame_hash_secs={:.3} other_secs={:.3}",
        summary.audio_decode_secs,
        summary.aac_encode_secs,
        summary.mux_secs,
        summary.faststart_secs,
        summary.rename_secs,
        summary.frame_hash_secs,
        summary.other_secs,
    );

    let verification = verify_exported_mp4(&args.output).map_err(|error| error.to_string())?;
    println!(
        "再検証: streams={} video_codec={} resolution={}x{} fps={:.3} video_frames={} video_duration_secs={:.6} audio_codec={} sample_rate={} audio_samples={} audio_duration_secs={:.6} av_difference_secs={:.6}",
        verification.streams,
        verification.video.codec,
        verification.video.width,
        verification.video.height,
        verification.video.fps,
        verification.video.frames,
        verification.video.duration_secs,
        verification.audio.codec,
        verification.audio.sample_rate,
        verification.audio.samples_per_channel,
        verification.audio.duration_secs,
        verification.duration_difference_secs
    );
    if verification.video.frames != summary.frames {
        return Err(format!(
            "映像フレーム数が不一致です: 予定 {}, 実測 {}",
            summary.frames, verification.video.frames
        ));
    }
    if verification.duration_difference_secs > 0.100 {
        return Err(format!(
            "映像と音声の長さが 100ms を超えてずれています: {:.3}ms",
            verification.duration_difference_secs * 1000.0
        ));
    }
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut audio = None;
    let mut image = None;
    let mut output = None;
    let mut seconds = None;
    let mut encoder = EncoderPreference::Auto;
    let mut video_bitrate_bps = DEFAULT_VIDEO_BITRATE_BPS;
    let mut frame_hash = false;
    let mut values = std::env::args().skip(1);
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--audio" => audio = Some(PathBuf::from(next_value(&mut values, "--audio")?)),
            "--image" => image = Some(PathBuf::from(next_value(&mut values, "--image")?)),
            "--out" => output = Some(PathBuf::from(next_value(&mut values, "--out")?)),
            "--seconds" => {
                let value = next_value(&mut values, "--seconds")?;
                seconds = Some(
                    value
                        .parse::<f64>()
                        .map_err(|_| format!("--seconds の値が不正です: {value}"))?,
                );
            }
            "--encoder" => {
                let value = next_value(&mut values, "--encoder")?;
                encoder = EncoderPreference::from_str(&value).map_err(|error| error.to_string())?;
            }
            "--video-bitrate" => {
                let value = next_value(&mut values, "--video-bitrate")?;
                video_bitrate_bps = value
                    .parse::<u32>()
                    .map_err(|_| format!("--video-bitrate の値が不正です: {value}"))?;
            }
            "--frame-hash" => frame_hash = true,
            "--help" | "-h" => return Err(usage().to_owned()),
            _ => return Err(format!("不明な引数です: {argument}\n{}", usage())),
        }
    }
    Ok(Args {
        audio: audio.ok_or_else(|| format!("--audio が必要です\n{}", usage()))?,
        image: image.ok_or_else(|| format!("--image が必要です\n{}", usage()))?,
        output: output.ok_or_else(|| format!("--out が必要です\n{}", usage()))?,
        seconds,
        encoder,
        video_bitrate_bps,
        frame_hash,
    })
}

fn next_value(values: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    values
        .next()
        .ok_or_else(|| format!("{option} の値がありません"))
}

fn usage() -> &'static str {
    "使用法: mvexport_cli --audio <音声> --image <画像> --out <出力.mp4> [--seconds N] [--encoder auto|nvenc|qsv|amf|mf|openh264] [--video-bitrate bps] [--frame-hash]"
}

fn load_assets(image_path: &Path) -> Result<Assets, String> {
    let bytes =
        load_default_font().ok_or_else(|| "Windows の日本語フォントを読み込めません".to_owned())?;
    let mut fonts = FontSet::new();
    fonts.insert(
        LoadedFont::from_bytes(DEFAULT_FONT_KEY, bytes)
            .map_err(|error| format!("フォントを読み込めません: {error}"))?,
    );
    let assets = Assets::new(fonts);
    let decoded = image::open(image_path)
        .map_err(|error| format!("背景画像を開けません ({}): {error}", image_path.display()))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let image = ImageAsset::new(width, height, decoded.into_raw())
        .map_err(|error| format!("背景画像が不正です: {error}"))?;
    assets.insert_image(ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID), image);
    Ok(assets)
}

fn load_default_font() -> Option<Vec<u8>> {
    [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
        r"C:\Windows\Fonts\YuGothR.ttc",
    ]
    .into_iter()
    .find_map(|path| std::fs::read(path).ok())
}
