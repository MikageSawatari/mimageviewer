use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use musicvideo_core::{
    Assets, Frame, ImageId, Project, RenderReport, ScaledImageCacheStats, Smoothing,
    SpectrumSource, UnresolvedAsset, evaluate, render_frame,
};

use crate::decode::AudioData;
use crate::project::{
    BACKGROUND_ID, FontData, create_assets, default_project, new_pitch_keyboard_layer,
    spectrum_band_count, synthetic_background,
};

const BENCHMARK_WARMUP_FRAMES: usize = 10;
const BENCHMARK_BACKGROUND_WIDTH: u32 = 3000;
const BENCHMARK_BACKGROUND_HEIGHT: u32 = 1687;
const BENCHMARK_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone, Copy, Default)]
pub struct TimingStats {
    pub mean_ms: f64,
    pub p95_ms: f64,
}

#[derive(Debug, Clone)]
pub struct RenderStats {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub analysis: TimingStats,
    pub composition: TimingStats,
    pub total: TimingStats,
    pub realtime_ratio: f64,
}

#[derive(Debug, Clone)]
pub enum ExportOutcome {
    Completed {
        output_dir: PathBuf,
        stats: RenderStats,
    },
    Cancelled {
        output_dir: PathBuf,
        completed_frames: usize,
    },
}

pub fn frame_count(project: &Project) -> Result<usize, String> {
    if !project.output.fps.is_finite() || project.output.fps <= 0.0 {
        return Err("fps は 0 より大きい有限値である必要があります".to_owned());
    }
    if !project.duration_secs.is_finite() || project.duration_secs <= 0.0 {
        return Err("書き出し時間が 0 秒です".to_owned());
    }
    let count = (project.duration_secs * project.output.fps).ceil();
    if count > usize::MAX as f64 {
        return Err("書き出しフレーム数が大きすぎます".to_owned());
    }
    Ok(count as usize)
}

#[allow(clippy::too_many_arguments)]
pub fn export_png_sequence(
    project: Project,
    assets: Arc<Assets>,
    audio: Arc<AudioData>,
    output_root: &Path,
    cancel: &AtomicBool,
    progress: &AtomicUsize,
    mut notify: impl FnMut(),
) -> Result<ExportOutcome, String> {
    let total_frames = frame_count(&project)?;
    let output_dir = create_output_dir(output_root)?;
    let mut spectrum = SpectrumSource::new(
        audio.sample_rate,
        spectrum_band_count(&project),
        Smoothing::Average { factor: 0.7 },
    )
    .map_err(|error| format!("スペクトラム解析を初期化できません: {error}"))?;
    let mut frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("出力フレームを確保できません: {error}"))?;
    let mut analysis_ms = Vec::with_capacity(total_frames);
    let mut composition_ms = Vec::with_capacity(total_frames);
    let mut total_ms = Vec::with_capacity(total_frames);
    let mut last_notify = Instant::now();

    for index in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            notify();
            return Ok(ExportOutcome::Cancelled {
                output_dir,
                completed_frames: index,
            });
        }
        let time = index as f64 / project.output.fps;
        let total_started = Instant::now();
        let analysis_started = Instant::now();
        let spectrum_frame = spectrum.full_frame_at(&audio.stereo_pcm, time, 0.0).clone();
        let analyzed_ms = analysis_started.elapsed().as_secs_f64() * 1000.0;
        let composition_started = Instant::now();
        let scene = evaluate(&project, time, &spectrum_frame);
        let report = render_frame(&scene, &assets, &mut frame)
            .map_err(|error| format!("フレーム {} の合成に失敗しました: {error}", index + 1))?;
        let composed_ms = composition_started.elapsed().as_secs_f64() * 1000.0;
        ensure_resolved(&report, &format!("フレーム {}", index + 1))?;
        let elapsed_ms = total_started.elapsed().as_secs_f64() * 1000.0;
        if cancel.load(Ordering::Relaxed) {
            notify();
            return Ok(ExportOutcome::Cancelled {
                output_dir,
                completed_frames: index,
            });
        }
        let path = output_dir.join(format!("frame_{index:06}.png"));
        image::save_buffer_with_format(
            &path,
            &frame.rgba,
            frame.width,
            frame.height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|error| format!("PNG を保存できません ({}): {error}", path.display()))?;
        analysis_ms.push(analyzed_ms);
        composition_ms.push(composed_ms);
        total_ms.push(elapsed_ms);
        progress.store(index + 1, Ordering::Relaxed);
        if last_notify.elapsed() >= Duration::from_millis(100) {
            notify();
            last_notify = Instant::now();
        }
    }

    let stats = summarize(
        &analysis_ms,
        &composition_ms,
        &total_ms,
        project.output.width,
        project.output.height,
        project.output.fps,
    );
    eprintln!(
        "musicvideo export: frames={} resolution={}x{} fps={:.3} analysis_mean_ms={:.3} analysis_p95_ms={:.3} composition_mean_ms={:.3} composition_p95_ms={:.3} total_mean_ms={:.3} total_p95_ms={:.3} realtime_ratio={:.3}",
        stats.frames,
        stats.width,
        stats.height,
        stats.fps,
        stats.analysis.mean_ms,
        stats.analysis.p95_ms,
        stats.composition.mean_ms,
        stats.composition.p95_ms,
        stats.total.mean_ms,
        stats.total.p95_ms,
        stats.realtime_ratio
    );
    notify();
    Ok(ExportOutcome::Completed { output_dir, stats })
}

fn create_output_dir(root: &Path) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            format!("miv_musicvideo_frames_{stamp}")
        } else {
            format!("miv_musicvideo_frames_{stamp}_{suffix}")
        };
        let path = root.join(name);
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "書き出しフォルダーを作成できません ({}): {error}",
                    path.display()
                ));
            }
        }
    }
    Err("重複しない書き出しフォルダー名を作成できません".to_owned())
}

fn summarize(
    analysis_ms: &[f64],
    composition_ms: &[f64],
    total_ms: &[f64],
    width: u32,
    height: u32,
    fps: f64,
) -> RenderStats {
    let total = summarize_timings(total_ms);
    RenderStats {
        frames: total_ms.len(),
        width,
        height,
        fps,
        analysis: summarize_timings(analysis_ms),
        composition: summarize_timings(composition_ms),
        total,
        realtime_ratio: if total.mean_ms > 0.0 {
            1000.0 / fps / total.mean_ms
        } else {
            0.0
        },
    }
}

fn summarize_timings(timings: &[f64]) -> TimingStats {
    if timings.is_empty() {
        return TimingStats::default();
    }
    let mean_ms = timings.iter().sum::<f64>() / timings.len() as f64;
    let mut sorted = timings.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p95_index = ((sorted.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    TimingStats {
        mean_ms,
        p95_ms: sorted[p95_index],
    }
}

fn ensure_resolved(report: &RenderReport, context: &str) -> Result<(), String> {
    if report.unresolved_assets.is_empty() {
        return Ok(());
    }
    let details = report
        .unresolved_assets
        .iter()
        .map(|asset| match asset {
            UnresolvedAsset::Image(id) => format!("画像 {}", id.0),
            UnresolvedAsset::Font(key) => format!("フォント {key}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("{context}で未解決の素材があります: {details}"))
}

pub fn render_sample_frame(output: &Path, font: &FontData) -> Result<(), String> {
    let assets = create_assets(Some(font))?;
    assets.insert_image(
        ImageId::new(BACKGROUND_ID),
        synthetic_background(BENCHMARK_BACKGROUND_WIDTH, BENCHMARK_BACKGROUND_HEIGHT),
    );
    let mut project = default_project(10.0);
    project.layers.push(new_pitch_keyboard_layer(&project));
    let pcm = synthetic_chirp_pcm(11.0);
    let mut spectrum = SpectrumSource::new(
        BENCHMARK_SAMPLE_RATE,
        spectrum_band_count(&project),
        Smoothing::Average { factor: 0.7 },
    )
    .map_err(|error| format!("サンプル用の解析を初期化できません: {error}"))?;
    let time = 5.0;
    let spectrum_frame = spectrum.full_frame_at(&pcm, time, 0.0).clone();
    let scene = evaluate(&project, time, &spectrum_frame);
    let mut frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("サンプル画像を確保できません: {error}"))?;
    let report = render_frame(&scene, &assets, &mut frame)
        .map_err(|error| format!("サンプル画像を合成できません: {error}"))?;
    ensure_resolved(&report, "サンプル画像")?;
    image::save_buffer_with_format(
        output,
        &frame.rgba,
        frame.width,
        frame.height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| {
        format!(
            "サンプル画像を保存できません ({}): {error}",
            output.display()
        )
    })
}

pub fn run_benchmark(frames: usize, font: &FontData) -> Result<(), String> {
    if frames == 0 {
        return Err("ベンチマークのフレーム数は 1 以上を指定してください".to_owned());
    }
    let without_glow = benchmark_once(frames, font, false)?;
    let with_glow = benchmark_once(frames, font, true)?;
    println!(
        "musicvideo benchmark glow_delta: composition_mean_ms={:.3} composition_p95_ms={:.3}",
        with_glow.composition.mean_ms - without_glow.composition.mean_ms,
        with_glow.composition.p95_ms - without_glow.composition.p95_ms,
    );
    Ok(())
}

fn benchmark_once(
    frames: usize,
    font: &FontData,
    glow_enabled: bool,
) -> Result<RenderStats, String> {
    let assets = create_assets(Some(font))?;
    assets.insert_image(
        ImageId::new(BACKGROUND_ID),
        synthetic_background(BENCHMARK_BACKGROUND_WIDTH, BENCHMARK_BACKGROUND_HEIGHT),
    );
    let mut project = default_project(frames as f64 / 60.0);
    if !glow_enabled {
        for layer in &mut project.layers {
            layer.glow = musicvideo_core::Animatable::Const(None);
        }
    }
    let pcm = synthetic_chirp_pcm(project.duration_secs + 1.0);
    let mut spectrum = SpectrumSource::new(
        BENCHMARK_SAMPLE_RATE,
        spectrum_band_count(&project),
        Smoothing::Average { factor: 0.7 },
    )
    .map_err(|error| format!("ベンチマーク解析を初期化できません: {error}"))?;
    let mut frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("ベンチマーク用フレームを確保できません: {error}"))?;

    for index in 0..BENCHMARK_WARMUP_FRAMES {
        let time = index as f64 / project.output.fps;
        let spectrum_frame = spectrum.full_frame_at(&pcm, time, 0.0).clone();
        let scene = evaluate(&project, time, &spectrum_frame);
        let report = render_frame(&scene, &assets, &mut frame)
            .map_err(|error| format!("ウォームアップ合成に失敗しました: {error}"))?;
        ensure_resolved(&report, "ウォームアップ")?;
    }
    let cache_after_warmup = assets.scaled_image_cache_stats();
    spectrum.reset();

    let mut analysis_ms = Vec::with_capacity(frames);
    let mut composition_ms = Vec::with_capacity(frames);
    let mut total_ms = Vec::with_capacity(frames);
    for index in 0..frames {
        let time = index as f64 / project.output.fps;
        let total_started = Instant::now();
        let analysis_started = Instant::now();
        let spectrum_frame = spectrum.full_frame_at(&pcm, time, 0.0).clone();
        let analyzed_ms = analysis_started.elapsed().as_secs_f64() * 1000.0;
        let composition_started = Instant::now();
        let scene = evaluate(&project, time, &spectrum_frame);
        let report = render_frame(&scene, &assets, &mut frame)
            .map_err(|error| format!("ベンチマーク合成に失敗しました: {error}"))?;
        let composed_ms = composition_started.elapsed().as_secs_f64() * 1000.0;
        ensure_resolved(&report, "ベンチマーク")?;
        analysis_ms.push(analyzed_ms);
        composition_ms.push(composed_ms);
        total_ms.push(total_started.elapsed().as_secs_f64() * 1000.0);
    }
    let stats = summarize(
        &analysis_ms,
        &composition_ms,
        &total_ms,
        project.output.width,
        project.output.height,
        project.output.fps,
    );
    let cache_after_measurement = assets.scaled_image_cache_stats();
    validate_benchmark_cache(cache_after_warmup, cache_after_measurement, frames as u64)?;
    println!(
        "musicvideo benchmark: glow={} frames={} resolution={}x{} fps={:.3} analysis_mean_ms={:.3} analysis_p95_ms={:.3} composition_mean_ms={:.3} composition_p95_ms={:.3} total_mean_ms={:.3} total_p95_ms={:.3} realtime_ratio={:.3}",
        if glow_enabled { "on" } else { "off" },
        stats.frames,
        stats.width,
        stats.height,
        stats.fps,
        stats.analysis.mean_ms,
        stats.analysis.p95_ms,
        stats.composition.mean_ms,
        stats.composition.p95_ms,
        stats.total.mean_ms,
        stats.total.p95_ms,
        stats.realtime_ratio
    );
    println!(
        "musicvideo benchmark cache: source={}x{} warmup_frames={} entries={} capacity={} misses_total={} scale_operations_total={} scale_operations_measured={} hits_measured={}",
        BENCHMARK_BACKGROUND_WIDTH,
        BENCHMARK_BACKGROUND_HEIGHT,
        BENCHMARK_WARMUP_FRAMES,
        cache_after_measurement.entries,
        cache_after_measurement.capacity,
        cache_after_measurement.misses,
        cache_after_measurement.scale_operations,
        cache_after_measurement
            .scale_operations
            .saturating_sub(cache_after_warmup.scale_operations),
        cache_after_measurement
            .hits
            .saturating_sub(cache_after_warmup.hits)
    );
    Ok(stats)
}

fn validate_benchmark_cache(
    after_warmup: ScaledImageCacheStats,
    after_measurement: ScaledImageCacheStats,
    measured_frames: u64,
) -> Result<(), String> {
    let measured_scales = after_measurement
        .scale_operations
        .saturating_sub(after_warmup.scale_operations);
    let measured_hits = after_measurement.hits.saturating_sub(after_warmup.hits);
    if after_warmup.scale_operations != 1
        || measured_scales != 0
        || measured_hits != measured_frames
    {
        return Err(format!(
            "背景画像キャッシュが想定どおり再利用されませんでした (事前拡縮 {}, 計測中拡縮 {}, 計測中ヒット {})",
            after_warmup.scale_operations, measured_scales, measured_hits
        ));
    }
    Ok(())
}

fn synthetic_chirp_pcm(duration_secs: f64) -> Vec<f32> {
    let sample_frames = (duration_secs.max(0.0) * BENCHMARK_SAMPLE_RATE as f64).ceil() as usize;
    let mut pcm = Vec::with_capacity(sample_frames * 2);
    let mut phase = 0.0_f64;
    let mut noise_state = 0x6d2b_79f5_u32;
    let min_frequency = 30.0_f64;
    let max_frequency = 18_000.0_f64;
    let sweep_seconds = 2.0_f64;
    for sample in 0..sample_frames {
        let time = sample as f64 / BENCHMARK_SAMPLE_RATE as f64;
        let sweep = (time % sweep_seconds) / sweep_seconds;
        let frequency = min_frequency * (max_frequency / min_frequency).powf(sweep);
        phase = (phase + std::f64::consts::TAU * frequency / BENCHMARK_SAMPLE_RATE as f64)
            % std::f64::consts::TAU;
        noise_state ^= noise_state << 13;
        noise_state ^= noise_state >> 17;
        noise_state ^= noise_state << 5;
        let noise = (noise_state as f64 / u32::MAX as f64 * 2.0 - 1.0) * 0.08;
        let value = (phase.sin() * 0.65 + noise).clamp(-1.0, 1.0) as f32;
        pcm.extend_from_slice(&[value, value]);
    }
    pcm
}
