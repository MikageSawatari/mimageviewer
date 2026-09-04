use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use musicvideo_core::{
    Assets, Frame, ImageId, Project, Smoothing, SpectrumFrame, SpectrumSource, evaluate,
    render_frame,
};

use crate::decode::AudioData;
use crate::project::{
    BACKGROUND_ID, FontData, SPECTRUM_BANDS, create_assets, default_project, synthetic_background,
};

#[derive(Debug, Clone)]
pub struct RenderStats {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub mean_ms: f64,
    pub p95_ms: f64,
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
        SPECTRUM_BANDS,
        Smoothing::Average { factor: 0.7 },
    )
    .map_err(|error| format!("スペクトラム解析を初期化できません: {error}"))?;
    let mut frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("出力フレームを確保できません: {error}"))?;
    let mut render_ms = Vec::with_capacity(total_frames);
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
        let bands = spectrum.frame_at(&audio.stereo_pcm, time).to_vec();
        let scene = evaluate(&project, time, &SpectrumFrame::new(bands));
        let started = Instant::now();
        render_frame(&scene, &assets, &mut frame)
            .map_err(|error| format!("フレーム {} の合成に失敗しました: {error}", index + 1))?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
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
        render_ms.push(elapsed_ms);
        progress.store(index + 1, Ordering::Relaxed);
        if last_notify.elapsed() >= Duration::from_millis(100) {
            notify();
            last_notify = Instant::now();
        }
    }

    let stats = summarize(
        &render_ms,
        project.output.width,
        project.output.height,
        project.output.fps,
    );
    eprintln!(
        "musicvideo export: frames={} resolution={}x{} fps={:.3} mean_ms={:.3} p95_ms={:.3} realtime_ratio={:.3}",
        stats.frames,
        stats.width,
        stats.height,
        stats.fps,
        stats.mean_ms,
        stats.p95_ms,
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

fn summarize(render_ms: &[f64], width: u32, height: u32, fps: f64) -> RenderStats {
    if render_ms.is_empty() {
        return RenderStats {
            frames: 0,
            width,
            height,
            fps,
            mean_ms: 0.0,
            p95_ms: 0.0,
            realtime_ratio: 0.0,
        };
    }
    let mean_ms = render_ms.iter().sum::<f64>() / render_ms.len() as f64;
    let mut sorted = render_ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p95_index = ((sorted.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    RenderStats {
        frames: render_ms.len(),
        width,
        height,
        fps,
        mean_ms,
        p95_ms: sorted[p95_index],
        realtime_ratio: 1000.0 / fps / mean_ms,
    }
}

pub fn run_benchmark(frames: usize, font: &FontData) -> Result<RenderStats, String> {
    if frames == 0 {
        return Err("ベンチマークのフレーム数は 1 以上を指定してください".to_owned());
    }
    let assets = create_assets(Some(font))?;
    assets.insert_image(ImageId::new(BACKGROUND_ID), synthetic_background());
    let project = default_project(frames as f64 / 60.0);
    let mut frame = Frame::new(project.output.width, project.output.height)
        .map_err(|error| format!("ベンチマーク用フレームを確保できません: {error}"))?;

    for index in 0..10 {
        let time = index as f64 / project.output.fps;
        let scene = evaluate(
            &project,
            time,
            &SpectrumFrame::new(synthetic_spectrum(index, SPECTRUM_BANDS)),
        );
        render_frame(&scene, &assets, &mut frame)
            .map_err(|error| format!("ウォームアップ合成に失敗しました: {error}"))?;
    }

    let mut timings = Vec::with_capacity(frames);
    for index in 0..frames {
        let time = index as f64 / project.output.fps;
        let scene = evaluate(
            &project,
            time,
            &SpectrumFrame::new(synthetic_spectrum(index, SPECTRUM_BANDS)),
        );
        let started = Instant::now();
        render_frame(&scene, &assets, &mut frame)
            .map_err(|error| format!("ベンチマーク合成に失敗しました: {error}"))?;
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let stats = summarize(
        &timings,
        project.output.width,
        project.output.height,
        project.output.fps,
    );
    println!(
        "musicvideo benchmark: frames={} resolution={}x{} fps={:.3} mean_ms={:.3} p95_ms={:.3} realtime_ratio={:.3}",
        stats.frames,
        stats.width,
        stats.height,
        stats.fps,
        stats.mean_ms,
        stats.p95_ms,
        stats.realtime_ratio
    );
    Ok(stats)
}

fn synthetic_spectrum(frame: usize, band_count: usize) -> Vec<f32> {
    (0..band_count)
        .map(|band| {
            let phase = frame as f32 * 0.071 + band as f32 * 0.193;
            (phase.sin() * 0.5 + 0.5).powf(1.35)
        })
        .collect()
}
