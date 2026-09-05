use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use music_core::{
    AnalysisConfig, KEYBOARD_NOTE_COUNT, KeyboardAnimationState, KeyboardFrame,
    SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalyzer,
    analyze_stereo_timeline_cancellable, prepare_keyboard_frame_into,
};
use musicvideo_core::{
    Animatable, Assets, BAND_ENERGY_CALIBRATION_HIGH_PERCENTILE,
    BAND_ENERGY_CALIBRATION_LOW_PERCENTILE, BandCalibration, Frame, ImageId, LayerKind,
    MODULATOR_CALIBRATION_HZ, ModulationBinding, ModulationCurve, ModulationTarget, ModulatorId,
    ModulatorSource, Project, RenderReport, ScaledImageCacheStats, Scene, SceneItemKind, Smoothing,
    SpectrumFrame, SpectrumSource, UnresolvedAsset, calibrate_modulators, evaluate,
    new_band_energy_modulator, new_miv_spectrum_keyboard_layers, norm_rect_to_pixels, render_frame,
};

use crate::decode::AudioData;
use crate::decode::decode_audio;
use crate::project::{
    BACKGROUND_ID, FontData, create_assets, default_project, new_pitch_keyboard_layer,
    spectrum_band_count, synthetic_background,
};
use crate::project_file::load_project;

const BENCHMARK_WARMUP_FRAMES: usize = 10;
const BENCHMARK_BACKGROUND_WIDTH: u32 = 3000;
const BENCHMARK_BACKGROUND_HEIGHT: u32 = 1687;
const BENCHMARK_SAMPLE_RATE: u32 = 48_000;
const DISTRIBUTION_DURATION_SECS: f64 = 18.0;
const DISTRIBUTION_FPS: f64 = 60.0;
const DISTRIBUTION_SIGNAL_AMPLITUDE: f64 = 0.0035;
const DISTRIBUTION_GAINS_DB: [f32; 3] = [0.0, 6.0, 12.0];
const STATIONARY_TONE_DURATION_SECS: f64 = 2.0;
const STATIONARY_TONE_CENTER_SECS: f64 = 1.0;
const KICK_RESPONSE_FPS: f64 = 60.0;
const KICK_RESPONSE_DURATION_SECS: f64 = 2.0;
const KICK_ONSET_SECS: f64 = 0.8;

pub fn run_project_modulator_response(project_path: &Path) -> Result<(), String> {
    let project = load_project(project_path)?;
    let audio_path = project
        .audio
        .as_ref()
        .ok_or_else(|| "プロジェクトに音声素材がありません".to_owned())?
        .path
        .clone();
    let cancel = AtomicBool::new(false);
    let audio = decode_audio(&audio_path, &cancel)?;

    let onset = new_band_energy_modulator(&project);
    let mut level = onset.clone();
    level.id = ModulatorId(onset.id.0.saturating_add(1));
    level.name = "低域の音量".to_owned();
    level.source = match onset.source.clone() {
        ModulatorSource::BandOnset {
            range,
            smoothing,
            weighting,
            gain_db,
            floor,
            ceiling,
            threshold,
            ..
        } => ModulatorSource::BandEnergy {
            range,
            smoothing,
            weighting,
            gain_db,
            floor,
            ceiling,
            calibration: BandCalibration::Auto {
                low_percentile: BAND_ENERGY_CALIBRATION_LOW_PERCENTILE,
                high_percentile: BAND_ENERGY_CALIBRATION_HIGH_PERCENTILE,
            },
            threshold,
        },
        ModulatorSource::BandEnergy { .. } => {
            return Err("キック既定が立ち上がり検出になっていません".to_owned());
        }
    };
    let modulators = [level, onset];
    let calibrations =
        calibrate_modulators(&audio.stereo_pcm, audio.sample_rate, &modulators, &|| false)
            .map_err(|error| format!("実素材の自動調整に失敗しました: {error}"))?;
    for calibration in &calibrations {
        println!(
            "calibration id={} feature={:?} percentiles={:.1}-{:.1} range={:.6}-{:.6} samples={}",
            calibration.request.id.0,
            calibration.request.feature,
            calibration.request.low_percentile,
            calibration.request.high_percentile,
            calibration.floor,
            calibration.ceiling,
            calibration.sample_count,
        );
    }

    let mut source = SpectrumSource::new(
        audio.sample_rate,
        spectrum_band_count(&project),
        Smoothing::Average { factor: 0.7 },
    )
    .map_err(|error| format!("実素材のスペクトラム解析を初期化できません: {error}"))?;
    source
        .configure_modulators_with_calibrations(&modulators, &calibrations)
        .map_err(|error| format!("実素材のモジュレータを初期化できません: {error}"))?;
    let sample_count = (audio.duration_secs * MODULATOR_CALIBRATION_HZ).ceil() as usize;
    let mut values = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let time = index as f64 / MODULATOR_CALIBRATION_HZ;
        let frame = source.full_frame_at(&audio.stereo_pcm, time, project.display_gain_db);
        let level_value = frame
            .modulators
            .iter()
            .find(|value| value.id == modulators[0].id)
            .map_or(0.0, |value| value.value);
        let onset_value = frame
            .modulators
            .iter()
            .find(|value| value.id == modulators[1].id)
            .map_or(0.0, |value| value.value);
        values.push((level_value, onset_value));
    }
    let level_saturated = values.iter().filter(|value| value.0 >= 0.95).count();
    let onset_saturated = values.iter().filter(|value| value.1 >= 0.95).count();
    let onset_quiet = values.iter().filter(|value| value.1 <= 0.1).count();
    println!(
        "distribution frames={} level_ge_0.95={:.3}% onset_ge_0.95={:.3}% onset_le_0.10={:.3}%",
        values.len(),
        level_saturated as f64 * 100.0 / values.len().max(1) as f64,
        onset_saturated as f64 * 100.0 / values.len().max(1) as f64,
        onset_quiet as f64 * 100.0 / values.len().max(1) as f64,
    );

    let timeline = analyze_stereo_timeline_cancellable(
        &audio.stereo_pcm,
        audio.sample_rate,
        AnalysisConfig::default(),
        &|| false,
    )
    .ok_or_else(|| "実素材のビート位置を解析できません".to_owned())?;
    println!(
        "independent beat grid: status={:?} bpm={:?} confidence={:.3}",
        timeline.beat_grid.status, timeline.beat_grid.bpm, timeline.beat_grid.confidence
    );
    println!("time_secs level_on_beat onset_on_beat level_between onset_between");
    let rows = timeline
        .beat_grid
        .beats
        .windows(2)
        .filter(|beats| beats[0].time_secs >= 20.0)
        .take(16)
        .map(|beats| {
            let beat = beats[0].time_secs;
            let between = (beats[0].time_secs + beats[1].time_secs) * 0.5;
            let on = response_at(&values, beat);
            let off = response_at(&values, between);
            println!(
                "{beat:.3} {:.6} {:.6} {:.6} {:.6}",
                on.0, on.1, off.0, off.1
            );
            (on, off)
        })
        .collect::<Vec<_>>();
    if !rows.is_empty() {
        let mean = |select: fn(&((f32, f32), (f32, f32))) -> f32| {
            rows.iter().map(select).sum::<f32>() / rows.len() as f32
        };
        println!(
            "beat means: level_on={:.6} onset_on={:.6} level_between={:.6} onset_between={:.6}",
            mean(|row| row.0.0),
            mean(|row| row.0.1),
            mean(|row| row.1.0),
            mean(|row| row.1.1),
        );
    }
    println!("dense time series: time_secs level onset");
    for step in 0..=40 {
        let time = 25.0 + step as f64 * 0.05;
        let value = response_at(&values, time);
        println!("{time:.3} {:.6} {:.6}", value.0, value.1);
    }
    Ok(())
}

fn response_at(values: &[(f32, f32)], time_secs: f64) -> (f32, f32) {
    let index = (time_secs * MODULATOR_CALIBRATION_HZ).round().max(0.0) as usize;
    values
        .get(index.min(values.len().saturating_sub(1)))
        .copied()
        .unwrap_or_default()
}

/// Renders the first image layer from a saved project at 1.0x and 1.2x, then
/// verifies sampled output pixels against the center-zoom coordinate mapping.
pub fn render_project_image_zoom_sample(
    project_path: &Path,
    output_directory: &Path,
) -> Result<(), String> {
    let mut project = load_project(project_path)?;
    let image_layer = project
        .layers
        .iter()
        .find(|layer| matches!(layer.kind, LayerKind::Image { .. }))
        .cloned()
        .ok_or_else(|| "プロジェクトに画像レイヤーがありません".to_owned())?;
    let image_id = match &image_layer.kind {
        LayerKind::Image { image, .. } => image.clone(),
        _ => unreachable!(),
    };
    let reference = project
        .images
        .iter()
        .find(|reference| reference.id == image_id)
        .ok_or_else(|| format!("画像素材 {} のパスがありません", image_id.0))?;
    let decoded = image::open(&reference.path)
        .map_err(|error| format!("画像を開けません ({}): {error}", reference.path.display()))?
        .to_rgba8();
    let assets = Assets::default();
    assets.insert_image(
        image_id,
        musicvideo_core::ImageAsset::new(decoded.width(), decoded.height(), decoded.into_raw())
            .map_err(|error| format!("画像を保持できません: {error}"))?,
    );
    project.layers = vec![image_layer];
    let spectrum = SpectrumFrame::default();
    let mut rendered = Vec::new();
    std::fs::create_dir_all(output_directory).map_err(|error| {
        format!(
            "サンプル出力フォルダーを作成できません ({}): {error}",
            output_directory.display()
        )
    })?;
    for (zoom_value, file_name) in [(1.0, "zoom-1.0.png"), (1.2, "zoom-1.2.png")] {
        let LayerKind::Image { zoom, .. } = &mut project.layers[0].kind else {
            unreachable!()
        };
        *zoom = Animatable::Const(zoom_value);
        let scene = evaluate(&project, 0.0, &spectrum);
        let mut frame = Frame::new(project.output.width, project.output.height)
            .map_err(|error| format!("サンプルフレームを確保できません: {error}"))?;
        let report = render_frame(&scene, &assets, &mut frame)
            .map_err(|error| format!("画像拡大サンプルを描画できません: {error}"))?;
        ensure_resolved(&report, "画像拡大サンプル")?;
        let output = output_directory.join(file_name);
        image::save_buffer_with_format(
            &output,
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
        })?;
        rendered.push((scene, frame, output));
    }

    let rect = norm_rect_to_pixels(
        rendered[0].0.items[0].rect,
        project.output.width,
        project.output.height,
    );
    let base = &rendered[0].1;
    let zoomed = &rendered[1].1;
    let mut error_sum = 0_u64;
    let mut samples = 0_u64;
    for grid_y in 1..=5 {
        for grid_x in 1..=9 {
            let x = rect.x0 + rect.width() * grid_x / 10;
            let y = rect.y0 + rect.height() * grid_y / 6;
            let local_x = x - rect.x0;
            let local_y = y - rect.y0;
            let source_x = rect.x0 as f32
                + (local_x as f32 + 0.5 - rect.width() as f32 * 0.5) / 1.2
                + rect.width() as f32 * 0.5
                - 0.5;
            let source_y = rect.y0 as f32
                + (local_y as f32 + 0.5 - rect.height() as f32 * 0.5) / 1.2
                + rect.height() as f32 * 0.5
                - 0.5;
            for channel in 0..3 {
                let expected = sample_frame_channel(base, source_x, source_y, channel);
                let actual = zoomed.rgba[(y * zoomed.width as usize + x) * 4 + channel];
                error_sum += u64::from(expected.abs_diff(actual));
                samples += 1;
            }
        }
    }
    let changed = base
        .rgba
        .chunks_exact(4)
        .zip(zoomed.rgba.chunks_exact(4))
        .filter(|(left, right)| left != right)
        .count();
    println!("zoom 1.0: {}", rendered[0].2.display());
    println!("zoom 1.2: {}", rendered[1].2.display());
    println!(
        "center mapping: samples={} mean_abs_error={:.6} changed_pixels={}",
        samples,
        error_sum as f64 / samples.max(1) as f64,
        changed
    );
    Ok(())
}

fn sample_frame_channel(frame: &Frame, x: f32, y: f32, channel: usize) -> u8 {
    let x = x.clamp(0.0, frame.width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, frame.height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(frame.width as usize - 1);
    let y1 = (y0 + 1).min(frame.height as usize - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let get =
        |px: usize, py: usize| frame.rgba[(py * frame.width as usize + px) * 4 + channel] as f32;
    let top = get(x0, y0) * (1.0 - tx) + get(x1, y0) * tx;
    let bottom = get(x0, y1) * (1.0 - tx) + get(x1, y1) * tx;
    (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8
}

/// Replays a continuous property drag against the real project and observes
/// the completed CPU frames directly. This separates missing glyph output
/// from preview scheduling: text rasterization is synchronous, so a completed
/// frame must either contain pixels or report its font as unresolved.
pub fn run_project_text_drag_diagnostic(project_path: &Path) -> Result<(), String> {
    const FRAMES: usize = 360;
    const POSITION_SECS: f64 = 26.0;

    let mut project = load_project(project_path)?;
    let font = crate::project::load_default_font()
        .ok_or_else(|| "テキスト診断に必要な Windows 日本語フォントが見つかりません".to_owned())?;
    let assets = crate::project::create_assets(Some(&font))?;
    for reference in &project.images {
        let image = image::open(&reference.path)
            .map_err(|error| {
                format!(
                    "実素材の画像を読み込めません ({}): {error}",
                    reference.path.display()
                )
            })?
            .to_rgba8();
        let image =
            musicvideo_core::ImageAsset::new(image.width(), image.height(), image.into_raw())
                .map_err(|error| format!("実素材の画像を保持できません: {error}"))?;
        assets.insert_image(reference.id.clone(), image);
    }
    let text_layer = project
        .layers
        .iter()
        .position(|layer| matches!(layer.kind, musicvideo_core::LayerKind::TimeText(_)))
        .ok_or_else(|| "プロジェクトに時間表示レイヤーがありません".to_owned())?;
    let spectrum = SpectrumFrame::new(vec![0.0; spectrum_band_count(&project)]);
    let mut isolated_frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("テキスト診断フレームを確保できません: {error}"))?;
    let mut full_frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("実素材診断フレームを確保できません: {error}"))?;
    let mut missing_pixels = Vec::new();
    let mut unresolved_fonts = Vec::new();
    let mut render_ms = Vec::with_capacity(FRAMES);

    for frame_index in 0..FRAMES {
        // A mouse drag supplies many distinct f32 values and therefore
        // deliberately exercises the text-cache miss path as well as hits.
        let phase = frame_index % 120;
        let size_px = if phase < 60 {
            35.0 + phase as f32 * 0.75
        } else {
            35.0 + (119 - phase) as f32 * 0.75
        };
        let musicvideo_core::LayerKind::TimeText(text) = &mut project.layers[text_layer].kind
        else {
            unreachable!("time-text layer index changed")
        };
        text.size_px = size_px;

        let scene = evaluate(&project, POSITION_SECS, &spectrum);
        let text_items = scene
            .items
            .iter()
            .filter(|item| matches!(item.kind, SceneItemKind::TimeText { .. }))
            .cloned()
            .collect::<Vec<_>>();
        let isolated = Scene {
            width: scene.width,
            height: scene.height,
            items: text_items,
        };
        let started = Instant::now();
        let report = render_frame(&isolated, &assets, &mut isolated_frame)
            .map_err(|error| format!("テキスト診断の合成に失敗しました: {error}"))?;
        render_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        if isolated_frame
            .rgba
            .chunks_exact(4)
            .all(|pixel| pixel[..3] == [0, 0, 0])
        {
            missing_pixels.push(frame_index);
        }
        if report
            .unresolved_assets
            .iter()
            .any(|asset| matches!(asset, UnresolvedAsset::Font(_)))
        {
            unresolved_fonts.push(frame_index);
        }

        // Also exercise the supplied project's full layer stack and assets.
        let report = render_frame(&scene, &assets, &mut full_frame)
            .map_err(|error| format!("実素材の全レイヤー合成に失敗しました: {error}"))?;
        ensure_resolved(&report, "実素材の全レイヤー合成")?;
    }

    render_ms.sort_by(f64::total_cmp);
    let mean = render_ms.iter().sum::<f64>() / render_ms.len() as f64;
    let p95 = render_ms[((render_ms.len() - 1) as f64 * 0.95).round() as usize];
    println!(
        "text drag diagnostic: completed={} text_missing={} unresolved_font={} mean_ms={mean:.3} p95_ms={p95:.3}",
        FRAMES,
        missing_pixels.len(),
        unresolved_fonts.len(),
    );
    if !missing_pixels.is_empty() {
        println!("text-missing frame indices: {missing_pixels:?}");
    }
    if !unresolved_fonts.is_empty() {
        println!("unresolved-font frame indices: {unresolved_fonts:?}");
    }
    Ok(())
}

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
    if !project.display_gain_db.is_finite() {
        return Err("表示ゲインは有限値にしてください".to_owned());
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
    let calibrations = calibrate_modulators(
        &audio.stereo_pcm,
        audio.sample_rate,
        &project.modulators,
        &|| cancel.load(Ordering::Relaxed),
    )
    .map_err(|error| format!("モジュレータの反応範囲を自動調整できません: {error}"))?;
    spectrum
        .configure_modulators_with_calibrations(&project.modulators, &calibrations)
        .map_err(|error| format!("モジュレータを初期化できません: {error}"))?;
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
        let spectrum_frame =
            spectrum.full_frame_at(&audio.stereo_pcm, time, project.display_gain_db);
        let analyzed_ms = analysis_started.elapsed().as_secs_f64() * 1000.0;
        let composition_started = Instant::now();
        let scene = evaluate(&project, time, spectrum_frame);
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
    let spectrum_frame = spectrum.full_frame_at(&pcm, time, project.display_gain_db);
    let scene = evaluate(&project, time, spectrum_frame);
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
    let without_glow = benchmark_once(frames, font, false, false, false)?;
    let with_glow = benchmark_once(frames, font, true, false, false)?;
    let with_modulation = benchmark_once(frames, font, true, true, false)?;
    let with_miv_spectrum = benchmark_once(frames, font, true, true, true)?;
    println!(
        "musicvideo benchmark glow_delta: composition_mean_ms={:.3} composition_p95_ms={:.3}",
        with_glow.composition.mean_ms - without_glow.composition.mean_ms,
        with_glow.composition.p95_ms - without_glow.composition.p95_ms,
    );
    println!(
        "musicvideo benchmark modulation_delta: analysis_mean_ms={:.3} composition_mean_ms={:.3} total_mean_ms={:.3}",
        with_modulation.analysis.mean_ms - with_glow.analysis.mean_ms,
        with_modulation.composition.mean_ms - with_glow.composition.mean_ms,
        with_modulation.total.mean_ms - with_glow.total.mean_ms,
    );
    println!(
        "musicvideo benchmark miv_horizontal_delta: analysis_mean_ms={:.3} composition_mean_ms={:.3} total_mean_ms={:.3}",
        with_miv_spectrum.analysis.mean_ms - with_modulation.analysis.mean_ms,
        with_miv_spectrum.composition.mean_ms - with_modulation.composition.mean_ms,
        with_miv_spectrum.total.mean_ms - with_modulation.total.mean_ms,
    );
    Ok(())
}

fn benchmark_once(
    frames: usize,
    font: &FontData,
    glow_enabled: bool,
    modulation_enabled: bool,
    miv_spectrum_enabled: bool,
) -> Result<RenderStats, String> {
    let assets = create_assets(Some(font))?;
    assets.insert_image(
        ImageId::new(BACKGROUND_ID),
        synthetic_background(BENCHMARK_BACKGROUND_WIDTH, BENCHMARK_BACKGROUND_HEIGHT),
    );
    let project = benchmark_project(
        frames as f64 / 60.0,
        glow_enabled,
        modulation_enabled,
        miv_spectrum_enabled,
    );
    let pcm = synthetic_chirp_pcm(project.duration_secs + 1.0);
    let mut spectrum = SpectrumSource::new(
        BENCHMARK_SAMPLE_RATE,
        spectrum_band_count(&project),
        Smoothing::Average { factor: 0.7 },
    )
    .map_err(|error| format!("ベンチマーク解析を初期化できません: {error}"))?;
    let calibrations =
        calibrate_modulators(&pcm, BENCHMARK_SAMPLE_RATE, &project.modulators, &|| false)
            .map_err(|error| format!("ベンチマークの自動調整に失敗しました: {error}"))?;
    spectrum
        .configure_modulators_with_calibrations(&project.modulators, &calibrations)
        .map_err(|error| format!("ベンチマークのモジュレータを初期化できません: {error}"))?;
    let mut frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("ベンチマーク用フレームを確保できません: {error}"))?;

    for index in 0..BENCHMARK_WARMUP_FRAMES {
        let time = index as f64 / project.output.fps;
        let spectrum_frame = spectrum.full_frame_at(&pcm, time, project.display_gain_db);
        let scene = evaluate(&project, time, spectrum_frame);
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
        let spectrum_frame = spectrum.full_frame_at(&pcm, time, project.display_gain_db);
        let analyzed_ms = analysis_started.elapsed().as_secs_f64() * 1000.0;
        let composition_started = Instant::now();
        let scene = evaluate(&project, time, spectrum_frame);
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
        "musicvideo benchmark: glow={} modulation={} keyboard=on miv_horizontal={} frames={} resolution={}x{} fps={:.3} analysis_mean_ms={:.3} analysis_p95_ms={:.3} composition_mean_ms={:.3} composition_p95_ms={:.3} total_mean_ms={:.3} total_p95_ms={:.3} realtime_ratio={:.3}",
        if glow_enabled { "on" } else { "off" },
        if modulation_enabled { "on" } else { "off" },
        if miv_spectrum_enabled { "on" } else { "off" },
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

fn benchmark_project(
    duration_secs: f64,
    glow_enabled: bool,
    modulation_enabled: bool,
    miv_spectrum_enabled: bool,
) -> Project {
    let mut project = default_project(duration_secs);
    if miv_spectrum_enabled {
        let layers = new_miv_spectrum_keyboard_layers(&project);
        project.layers.extend(layers);
    } else {
        project.layers.push(new_pitch_keyboard_layer(&project));
    }
    if !glow_enabled {
        for layer in &mut project.layers {
            layer.glow = musicvideo_core::Animatable::Const(None);
        }
    }
    if modulation_enabled {
        let modulator = new_band_energy_modulator(&project);
        project.layers[1].modulations.push(ModulationBinding {
            target: ModulationTarget::SpectrumRadius,
            modulator: modulator.id,
            amount: 80.0,
            curve: ModulationCurve::Squared,
        });
        project.modulators.push(modulator);
    }
    project
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

struct KeyboardDistribution {
    raw_peaks: Vec<f32>,
    keyboard_peaks: Vec<Vec<f32>>,
}

struct StationaryNoteResponse {
    values: Vec<f32>,
}

#[derive(Debug, Clone, Copy)]
struct KickResponseSample {
    time_secs: f64,
    raw: f32,
    average: f32,
    envelope: f32,
}

pub fn run_keyboard_distribution() -> Result<(), String> {
    let distribution = measure_keyboard_distribution(DISTRIBUTION_DURATION_SECS, DISTRIBUTION_FPS)?;
    println!(
        "keyboard distribution: signal=constant_amplitude_log_sweep sample_rate={} duration_secs={:.1} analysis_fps={:.1} min_hz=27.5 max_hz=4186.0 amplitude={:.4} gains_db=0,6,12 metric=mean_of_per_note_peak lit_threshold=0.20",
        BENCHMARK_SAMPLE_RATE,
        DISTRIBUTION_DURATION_SECS,
        DISTRIBUTION_FPS,
        DISTRIBUTION_SIGNAL_AMPLITUDE,
    );
    for (label, first_midi, last_midi) in keyboard_distribution_ranges() {
        let raw = summarize_note_range(&distribution.raw_peaks, first_midi, last_midi);
        let gain_0 = summarize_note_range(&distribution.keyboard_peaks[0], first_midi, last_midi);
        let gain_6 = summarize_note_range(&distribution.keyboard_peaks[1], first_midi, last_midi);
        let gain_12 = summarize_note_range(&distribution.keyboard_peaks[2], first_midi, last_midi);
        println!(
            "keyboard distribution range={label} midi={first_midi}-{last_midi} notes={} raw_mean={:.4} raw_min={:.4} raw_max={:.4} gain_0_mean={:.4} gain_0_min={:.4} gain_0_max={:.4} gain_6_mean={:.4} gain_12_mean={:.4} lit_0={}/{} lit_6={}/{} lit_12={}/{}",
            raw.count,
            raw.mean,
            raw.min,
            raw.max,
            gain_0.mean,
            gain_0.min,
            gain_0.max,
            gain_6.mean,
            gain_12.mean,
            gain_0.lit,
            gain_0.count,
            gain_6.lit,
            gain_6.count,
            gain_12.lit,
            gain_12.count,
        );
    }
    Ok(())
}

pub fn run_stationary_note_response() -> Result<(), String> {
    let response = measure_stationary_note_response(
        STATIONARY_TONE_DURATION_SECS,
        STATIONARY_TONE_CENTER_SECS,
        DISTRIBUTION_SIGNAL_AMPLITUDE,
    )?;
    println!(
        "stationary note response: signal=independent_constant_amplitude_sine sample_rate={} tone_duration_secs={:.1} center_secs={:.1} amplitude={:.4} longest_window_secs={:.6} metric=target_note_display_value",
        BENCHMARK_SAMPLE_RATE,
        STATIONARY_TONE_DURATION_SECS,
        STATIONARY_TONE_CENTER_SECS,
        DISTRIBUTION_SIGNAL_AMPLITUDE,
        32_768.0 / BENCHMARK_SAMPLE_RATE as f64,
    );
    for (offset, value) in response.values.iter().enumerate() {
        let midi = SPECTRUM_NOTE_MIN_MIDI + offset as u8;
        println!(
            "stationary note midi={midi} note={} hz={:.3} value={value:.6}",
            midi_note_label(midi),
            midi_frequency(midi),
        );
    }
    for (label, first_midi, last_midi) in keyboard_distribution_ranges() {
        let stats = summarize_note_range(&response.values, first_midi, last_midi);
        println!(
            "stationary range={label} midi={first_midi}-{last_midi} notes={} mean={:.6} min={:.6} max={:.6}",
            stats.count, stats.mean, stats.min, stats.max,
        );
    }
    Ok(())
}

pub fn run_kick_envelope_response() -> Result<(), String> {
    let samples = measure_kick_envelope_response()?;
    println!(
        "kick envelope response: signal=decaying_100_to_50_hz_burst onset_secs={KICK_ONSET_SECS:.3} sample_rate={BENCHMARK_SAMPLE_RATE} analysis_fps={KICK_RESPONSE_FPS:.1} range_hz=40-120 preprocessing=gain_3db_floor_0.03_ceiling_0.8_threshold_0.08 average_factor=0.7 envelope_attack_secs={:.3} envelope_release_secs={:.3}",
        musicvideo_core::KICK_ATTACK_SECS,
        musicvideo_core::KICK_RELEASE_SECS,
    );
    println!("time_secs offset_secs raw average_ema envelope");
    for sample in samples.iter().filter(|sample| {
        sample.time_secs >= KICK_ONSET_SECS - 0.4
            && sample.time_secs <= KICK_ONSET_SECS + 0.8
            && ((sample.time_secs * 20.0).round() - sample.time_secs * 20.0).abs() < 1.0e-6
    }) {
        println!(
            "{:.3} {:+.3} {:.6} {:.6} {:.6}",
            sample.time_secs,
            sample.time_secs - KICK_ONSET_SECS,
            sample.raw,
            sample.average,
            sample.envelope,
        );
    }
    Ok(())
}

fn measure_kick_envelope_response() -> Result<Vec<KickResponseSample>, String> {
    let project = default_project(KICK_RESPONSE_DURATION_SECS);
    let mut raw = new_band_energy_modulator(&project);
    raw.id = ModulatorId(1);
    raw.name = "raw".to_owned();
    // This older diagnostic isolates temporal smoothing itself. Keep its
    // source level-based even though the user-facing kick preset now uses the
    // separate onset feature.
    raw.source = match raw.source {
        ModulatorSource::BandOnset {
            range,
            smoothing,
            weighting,
            gain_db,
            floor,
            ceiling,
            calibration,
            threshold,
        } => ModulatorSource::BandEnergy {
            range,
            smoothing,
            weighting,
            gain_db,
            floor,
            ceiling,
            calibration,
            threshold,
        },
        source @ ModulatorSource::BandEnergy { .. } => source,
    };
    let smoothing = match &mut raw.source {
        ModulatorSource::BandEnergy { smoothing, .. }
        | ModulatorSource::BandOnset { smoothing, .. } => smoothing,
    };
    *smoothing = Smoothing::None;

    let mut average = raw.clone();
    average.id = ModulatorId(2);
    average.name = "average".to_owned();
    let smoothing = match &mut average.source {
        ModulatorSource::BandEnergy { smoothing, .. }
        | ModulatorSource::BandOnset { smoothing, .. } => smoothing,
    };
    *smoothing = Smoothing::Average { factor: 0.7 };

    let mut envelope = raw.clone();
    envelope.id = ModulatorId(3);
    envelope.name = "envelope".to_owned();
    let smoothing = match &mut envelope.source {
        ModulatorSource::BandEnergy { smoothing, .. }
        | ModulatorSource::BandOnset { smoothing, .. } => smoothing,
    };
    *smoothing = Smoothing::Envelope {
        attack_secs: musicvideo_core::KICK_ATTACK_SECS,
        release_secs: musicvideo_core::KICK_RELEASE_SECS,
    };

    let pcm = synthetic_kick_pcm(KICK_RESPONSE_DURATION_SECS, KICK_ONSET_SECS);
    let mut source = SpectrumSource::new(
        BENCHMARK_SAMPLE_RATE,
        musicvideo_core::DEFAULT_SPECTRUM_BANDS,
        Smoothing::None,
    )
    .map_err(|error| format!("キック応答の解析を初期化できません: {error}"))?;
    source
        .configure_modulators(&[raw, average, envelope])
        .map_err(|error| format!("キック応答の変調を初期化できません: {error}"))?;
    let frame_count = (KICK_RESPONSE_DURATION_SECS * KICK_RESPONSE_FPS).round() as usize;
    let mut samples = Vec::with_capacity(frame_count + 1);
    for index in 0..=frame_count {
        let time_secs = index as f64 / KICK_RESPONSE_FPS;
        let values = &source.full_frame_at(&pcm, time_secs, 0.0).modulators;
        if values.len() != 3 {
            return Err("キック応答の変調値が 3 系統そろいませんでした".to_owned());
        }
        samples.push(KickResponseSample {
            time_secs,
            raw: values[0].value,
            average: values[1].value,
            envelope: values[2].value,
        });
    }
    Ok(samples)
}

fn synthetic_kick_pcm(duration_secs: f64, onset_secs: f64) -> Vec<f32> {
    let frame_count = (duration_secs * BENCHMARK_SAMPLE_RATE as f64).ceil() as usize;
    let mut pcm = Vec::with_capacity(frame_count * 2);
    for frame in 0..frame_count {
        let time = frame as f64 / BENCHMARK_SAMPLE_RATE as f64;
        let elapsed = time - onset_secs;
        let value = if (0.0..0.25).contains(&elapsed) {
            let attack = 1.0 - (-elapsed / 0.002).exp();
            let decay = (-elapsed / 0.055).exp();
            let phase = std::f64::consts::TAU * (100.0 * elapsed - 125.0 * elapsed * elapsed);
            (phase.sin() * attack * decay * 0.9) as f32
        } else {
            0.0
        };
        pcm.extend_from_slice(&[value, value]);
    }
    pcm
}

fn measure_stationary_note_response(
    tone_duration_secs: f64,
    center_secs: f64,
    amplitude: f64,
) -> Result<StationaryNoteResponse, String> {
    let longest_window_secs = 32_768.0 / BENCHMARK_SAMPLE_RATE as f64;
    let half_window_secs = longest_window_secs * 0.5;
    if !tone_duration_secs.is_finite() || tone_duration_secs < 1.0 {
        return Err("定常正弦波は 1 秒以上にしてください".to_owned());
    }
    if !center_secs.is_finite()
        || center_secs < half_window_secs
        || center_secs + half_window_secs > tone_duration_secs
    {
        return Err("測定位置の前後に最長 FFT 窓の半分以上の余裕が必要です".to_owned());
    }
    if !amplitude.is_finite() || !(0.0..=1.0).contains(&amplitude) {
        return Err("正弦波の振幅は 0 以上 1 以下の有限値にしてください".to_owned());
    }

    let mut analyzer = SpectrumAnalyzer::new(musicvideo_core::DEFAULT_SPECTRUM_BANDS);
    let mut values = Vec::with_capacity(KEYBOARD_NOTE_COUNT);
    for midi in SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI {
        let pcm = synthetic_stationary_sine_pcm(midi, tone_duration_secs, amplitude);
        let analysis = analyzer.analyze_moving_window(&pcm, BENCHMARK_SAMPLE_RATE, center_secs);
        let index = usize::from(midi - analysis.note_min_midi);
        values.push(analysis.notes.get(index).copied().unwrap_or(0.0));
    }
    Ok(StationaryNoteResponse { values })
}

fn synthetic_stationary_sine_pcm(midi: u8, duration_secs: f64, amplitude: f64) -> Vec<f32> {
    let sample_frames = (duration_secs * BENCHMARK_SAMPLE_RATE as f64).ceil() as usize;
    let frequency = midi_frequency(midi);
    let mut pcm = Vec::with_capacity(sample_frames * 2);
    for sample in 0..sample_frames {
        let phase =
            std::f64::consts::TAU * frequency * sample as f64 / BENCHMARK_SAMPLE_RATE as f64;
        let value = (phase.sin() * amplitude) as f32;
        pcm.extend_from_slice(&[value, value]);
    }
    pcm
}

fn midi_frequency(midi: u8) -> f64 {
    440.0 * 2.0_f64.powf((midi as f64 - 69.0) / 12.0)
}

fn midi_note_label(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(midi % 12)],
        i16::from(midi) / 12 - 1
    )
}

fn measure_keyboard_distribution(
    duration_secs: f64,
    analysis_fps: f64,
) -> Result<KeyboardDistribution, String> {
    if !duration_secs.is_finite() || duration_secs <= 2.0 {
        return Err("対数スイープは 2 秒より長くしてください".to_owned());
    }
    if !analysis_fps.is_finite() || analysis_fps <= 0.0 {
        return Err("解析 fps は 0 より大きい有限値にしてください".to_owned());
    }
    let pcm = synthetic_log_sweep_pcm(duration_secs);
    let mut analyzer = SpectrumAnalyzer::new(musicvideo_core::DEFAULT_SPECTRUM_BANDS);
    let mut states = DISTRIBUTION_GAINS_DB
        .iter()
        .map(|_| KeyboardAnimationState::default())
        .collect::<Vec<_>>();
    let mut frames = DISTRIBUTION_GAINS_DB
        .iter()
        .map(|_| KeyboardFrame::default())
        .collect::<Vec<_>>();
    let mut raw_peaks = vec![0.0_f32; KEYBOARD_NOTE_COUNT];
    let mut keyboard_peaks = DISTRIBUTION_GAINS_DB
        .iter()
        .map(|_| vec![0.0_f32; KEYBOARD_NOTE_COUNT])
        .collect::<Vec<_>>();
    let frame_count = (duration_secs * analysis_fps).ceil() as usize;
    for index in 0..=frame_count {
        let time = (index as f64 / analysis_fps).min(duration_secs);
        let analysis = analyzer.analyze(&pcm, BENCHMARK_SAMPLE_RATE, time);
        for (peak, value) in raw_peaks.iter_mut().zip(&analysis.notes) {
            *peak = peak.max(*value);
        }
        for (((gain_db, state), frame), peaks) in DISTRIBUTION_GAINS_DB
            .iter()
            .zip(&mut states)
            .zip(&mut frames)
            .zip(&mut keyboard_peaks)
        {
            prepare_keyboard_frame_into(
                &analysis.notes,
                analysis.note_min_midi,
                *gain_db,
                state,
                frame,
            );
            for (peak, levels) in peaks.iter_mut().zip(&frame.levels) {
                *peak = peak.max(levels.center);
            }
        }
    }
    Ok(KeyboardDistribution {
        raw_peaks,
        keyboard_peaks,
    })
}

fn synthetic_log_sweep_pcm(duration_secs: f64) -> Vec<f32> {
    let sample_frames = (duration_secs * BENCHMARK_SAMPLE_RATE as f64).ceil() as usize;
    let mut pcm = Vec::with_capacity(sample_frames * 2);
    let mut phase = 0.0_f64;
    let lead_secs = 1.0;
    let sweep_secs = (duration_secs - 2.0).max(0.001);
    let min_frequency = 27.5_f64;
    let max_frequency = 4_186.009_f64;
    for sample in 0..sample_frames {
        let time = sample as f64 / BENCHMARK_SAMPLE_RATE as f64;
        let position = ((time - lead_secs) / sweep_secs).clamp(0.0, 1.0);
        let frequency = min_frequency * (max_frequency / min_frequency).powf(position);
        phase = (phase + std::f64::consts::TAU * frequency / BENCHMARK_SAMPLE_RATE as f64)
            % std::f64::consts::TAU;
        let value = (phase.sin() * DISTRIBUTION_SIGNAL_AMPLITUDE) as f32;
        pcm.extend_from_slice(&[value, value]);
    }
    pcm
}

#[derive(Clone, Copy)]
struct NoteRangeStats {
    count: usize,
    mean: f32,
    min: f32,
    max: f32,
    lit: usize,
}

fn summarize_note_range(values: &[f32], first_midi: u8, last_midi: u8) -> NoteRangeStats {
    let first = first_midi.saturating_sub(SPECTRUM_NOTE_MIN_MIDI) as usize;
    let last = last_midi.saturating_sub(SPECTRUM_NOTE_MIN_MIDI) as usize;
    let values = &values[first..=last.min(values.len().saturating_sub(1))];
    let count = values.len();
    let sum = values.iter().sum::<f32>();
    NoteRangeStats {
        count,
        mean: sum / count.max(1) as f32,
        min: values.iter().copied().fold(f32::INFINITY, f32::min),
        max: values.iter().copied().fold(0.0, f32::max),
        lit: values.iter().filter(|value| **value >= 0.20).count(),
    }
}

fn keyboard_distribution_ranges() -> Vec<(&'static str, u8, u8)> {
    let mut ranges = vec![
        ("low_A0-B2", SPECTRUM_NOTE_MIN_MIDI, 47),
        ("mid_C3-B5", 48, 83),
        ("high_C6-C8", 84, SPECTRUM_NOTE_MAX_MIDI),
        ("A0-B0", SPECTRUM_NOTE_MIN_MIDI, 23),
    ];
    for octave in 1..=7 {
        let first = 12 * (octave + 1);
        ranges.push((
            match octave {
                1 => "C1-B1",
                2 => "C2-B2",
                3 => "C3-B3",
                4 => "C4-B4",
                5 => "C5-B5",
                6 => "C6-B6",
                _ => "C7-B7",
            },
            first,
            first + 11,
        ));
    }
    ranges.push(("C8", SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MAX_MIDI));
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_core::LayerKind;

    #[test]
    fn benchmark_scene_includes_the_pitch_keyboard_path() {
        let project = benchmark_project(10.0, true, true, false);
        assert!(
            project
                .layers
                .iter()
                .any(|layer| matches!(&layer.kind, LayerKind::PitchKeyboard(_)))
        );
        assert_eq!(project.modulators.len(), 1);
        assert_eq!(project.layers[1].modulations.len(), 1);
    }

    #[test]
    fn benchmark_can_cover_the_shared_miv_horizontal_spectrum_path() {
        let project = benchmark_project(10.0, true, true, true);
        assert!(project.layers.iter().any(|layer| matches!(
            &layer.kind,
            LayerKind::Spectrum(spectrum)
                if spectrum.shape == musicvideo_core::SpectrumShape::MivHorizontal
        )));
        assert!(
            project
                .layers
                .iter()
                .any(|layer| matches!(&layer.kind, LayerKind::PitchKeyboard(_)))
        );
    }

    #[test]
    fn log_sweep_distribution_reaches_the_high_keyboard_range_and_gain_increases_it() {
        let distribution = measure_keyboard_distribution(8.0, 30.0).unwrap();
        let high_without_gain = summarize_note_range(&distribution.keyboard_peaks[0], 84, 108);
        let high_with_gain = summarize_note_range(&distribution.keyboard_peaks[2], 84, 108);
        assert_eq!(distribution.raw_peaks.len(), KEYBOARD_NOTE_COUNT);
        assert!(high_without_gain.max > 0.05);
        assert!(high_with_gain.mean > high_without_gain.mean);
        assert!(high_with_gain.lit >= high_without_gain.lit);
    }

    #[test]
    fn stationary_note_response_uses_fully_filled_windows_and_covers_the_keyboard() {
        let response = measure_stationary_note_response(1.2, 0.6, 0.0035).unwrap();
        let c4 = summarize_note_range(&response.values, 60, 71);
        let c5 = summarize_note_range(&response.values, 72, 83);
        let c6 = summarize_note_range(&response.values, 84, 95);
        assert_eq!(response.values.len(), KEYBOARD_NOTE_COUNT);
        assert!(response.values.iter().all(|value| value.is_finite()));
        assert!(response.values.iter().all(|value| *value > 0.01));
        assert!(c5.mean >= c4.mean * 0.9, "C5-B5 should not dip below C4-B4");
        assert!(
            c5.mean >= c6.mean * 0.85,
            "C5-B5 should not dip below C6-B6"
        );
    }

    #[test]
    fn kick_envelope_attacks_faster_and_releases_slower_than_average() {
        let samples = measure_kick_envelope_response().unwrap();
        let at = |time: f64| {
            samples
                .iter()
                .find(|sample| (sample.time_secs - time).abs() < 1.0e-9)
                .copied()
                .unwrap()
        };
        let before = at(0.7);
        let onset = at(0.8);
        let tail = at(1.15);

        assert_eq!(before.raw, 0.0);
        assert_eq!(before.envelope, 0.0);
        assert!(onset.envelope > onset.average + 0.3);
        assert!(tail.envelope > tail.average + 0.3);
    }
}
