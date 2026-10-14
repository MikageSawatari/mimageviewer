//! Shared snapshot construction and blocking export for the GUI worker and dev CLI.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mimageviewer::video::mvexport::{
    EncoderPreference, ExportError, ExportProgress, ExportSummary, MusicVideoExportRequest,
    export_music_video, verify_exported_mp4,
};
use musicvideo_core::{Assets, Project};
use musicvideo_ui::ResourceRevision;

use crate::decode::AudioData;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExportSettings {
    pub encoder: EncoderPreference,
    pub video_bitrate_bps: u32,
    pub compute_frame_hash: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            encoder: EncoderPreference::Auto,
            video_bitrate_bps: 12_000_000,
            compute_frame_hash: false,
        }
    }
}

pub(crate) fn snapshot(
    project: &Project,
    assets: &Assets,
    audio: Arc<AudioData>,
    pcm_id: ResourceRevision,
    output_path: PathBuf,
    settings: ExportSettings,
    replace_existing: bool,
) -> MusicVideoExportRequest<ResourceRevision> {
    let retained = project
        .images
        .iter()
        .map(|image| image.id.clone())
        .collect::<Vec<_>>();
    MusicVideoExportRequest {
        project: project.clone(),
        assets: assets.fork_retain_images(&retained),
        source_duration_secs: (audio.stereo_pcm.len() / 2) as f64 / f64::from(audio.sample_rate),
        audio,
        pcm_id,
        output_path,
        encoder: settings.encoder,
        video_bitrate_bps: settings.video_bitrate_bps,
        replace_existing,
        compute_frame_hash: settings.compute_frame_hash,
    }
}

pub(crate) fn export_snapshot(
    request: MusicVideoExportRequest<ResourceRevision>,
    progress: &dyn Fn(ExportProgress),
    cancel: &AtomicBool,
) -> Result<ExportSummary, ExportError> {
    export_music_video(request, progress, cancel)
}

struct DevExportArgs {
    project_path: PathBuf,
    output_path: PathBuf,
    settings: ExportSettings,
}

fn parse_dev_args(mut args: impl Iterator<Item = String>) -> Result<DevExportArgs, String> {
    let usage = "--export-mp4 <project.mivmv.json> <output.mp4> [--encoder X] [--video-bitrate N] [--frame-hash]";
    let project_path = args
        .next()
        .filter(|arg| !arg.starts_with("--"))
        .ok_or(usage)?;
    let output_path = args
        .next()
        .filter(|arg| !arg.starts_with("--"))
        .ok_or(usage)?;
    let mut settings = ExportSettings::default();
    while let Some(option) = args.next() {
        match option.as_str() {
            "--encoder" => {
                settings.encoder = args
                    .next()
                    .ok_or(usage)?
                    .parse()
                    .map_err(|error: ExportError| error.to_string())?;
            }
            "--video-bitrate" => {
                settings.video_bitrate_bps = args
                    .next()
                    .ok_or(usage)?
                    .parse()
                    .map_err(|_| "ビットレートは正の整数で指定してください")?;
                if settings.video_bitrate_bps == 0 {
                    return Err("ビットレートは正の整数で指定してください".to_owned());
                }
            }
            "--frame-hash" => settings.compute_frame_hash = true,
            _ => return Err(format!("不明な書き出しオプションです: {option}")),
        }
    }
    Ok(DevExportArgs {
        project_path: project_path.into(),
        output_path: output_path.into(),
        settings,
    })
}

fn load_snapshot(
    args: DevExportArgs,
    cancel: &AtomicBool,
) -> Result<MusicVideoExportRequest<ResourceRevision>, String> {
    // Use the same project/path resolution, image loading, decoder and immutable
    // resource leases as the editor's Project-open worker. No GUI/audio device.
    let store = crate::resource_blobs::ResourceBlobStore::new()?;
    let loaded = crate::app::load_project_bundle(args.project_path, &store, cancel)?;
    if let Some(error) = loaded.audio_error {
        return Err(error);
    }
    if !loaded.image_errors.is_empty() {
        return Err(format!(
            "画像素材を読み込めません: {:?}",
            loaded.image_errors
        ));
    }
    let audio = loaded.audio.ok_or("プロジェクトに音声がありません")?;
    let pcm_id = loaded
        .resources
        .audio()
        .ok_or("音声の識別がありません")?
        .revision();
    let font = crate::project::load_default_font();
    let assets = crate::project::create_assets(font.as_ref())?;
    for (id, image) in loaded.images {
        assets.insert_shared_image(id, image);
    }
    Ok(snapshot(
        &loaded.project,
        &assets,
        audio,
        pcm_id,
        args.output_path,
        args.settings,
        false,
    ))
}

pub(crate) fn run_dev_export(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args = parse_dev_args(args)?;
    let output_path = args.output_path.clone();
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let request = load_snapshot(args, &cancel)?;
    let load_secs = started.elapsed().as_secs_f64();
    let mut last_stage = None;
    let mut last_frames = 0;
    let progress_state = std::cell::RefCell::new((&mut last_stage, &mut last_frames));
    let summary = export_snapshot(
        request,
        &|progress| {
            let mut state = progress_state.borrow_mut();
            if *state.0 != Some(progress.stage)
                || progress.completed_frames == progress.total_frames
                || progress.completed_frames.saturating_sub(*state.1) >= 600
            {
                eprintln!(
                    "進捗: {:?} {}/{} フレーム",
                    progress.stage, progress.completed_frames, progress.total_frames
                );
                *state.0 = Some(progress.stage);
                *state.1 = progress.completed_frames;
            }
        },
        &cancel,
    )
    .map_err(|error| error.to_string())?;
    println!("MP4 export: {summary:?}");
    println!(
        "project_load_secs={load_secs:.6} total_wall_secs={:.6}",
        started.elapsed().as_secs_f64()
    );
    let verification = verify_exported_mp4(&output_path).map_err(|error| error.to_string())?;
    println!("MP4 verification: {verification:?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p4b3_dev_load_resolves_editor_project_assets_and_default_ffmpeg_audio() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/audio-tracks/multi-audio.m4a");
        let audio_path = dir.path().join("source.m4a");
        std::fs::copy(fixture, &audio_path).unwrap();
        let image_path = dir.path().join("background.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 8, 7, 255]))
            .save(&image_path)
            .unwrap();
        let mut project = musicvideo_core::default_project(0.5);
        project.audio = Some(musicvideo_core::AudioRef {
            path: audio_path.clone(),
        });
        let id = musicvideo_core::ImageId::new(crate::project::BACKGROUND_ID);
        project.images.push(musicvideo_core::ImageRef {
            id: id.clone(),
            path: image_path,
        });
        let project_path = dir.path().join("edited.mivmv.json");
        crate::project_file::save_project(&project_path, &project).unwrap();
        let cancel = AtomicBool::new(false);
        let expected = crate::decode::decode_audio(&audio_path, &cancel).unwrap();
        let request = load_snapshot(
            DevExportArgs {
                project_path,
                output_path: dir.path().join("output.mp4"),
                settings: ExportSettings::default(),
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(request.project, project);
        assert_eq!(request.audio.sample_rate, expected.sample_rate);
        assert_eq!(request.audio.stereo_pcm, expected.stereo_pcm);
        assert_eq!(
            request.source_duration_secs,
            expected.stereo_pcm.len() as f64 / 2.0 / f64::from(expected.sample_rate)
        );
        assert_eq!(
            request.assets.image(&id).unwrap().rgba,
            [9, 8, 7, 255].repeat(4)
        );
        assert!(!request.replace_existing);
    }
    #[test]
    fn p4b3_dev_export_parses_defaults_overrides_and_rejects_bad_options() {
        let parse = |args: &[&str]| parse_dev_args(args.iter().map(|arg| arg.to_string()));
        let default = parse(&["project.mivmv.json", "out.mp4"]).unwrap();
        assert_eq!(default.settings, ExportSettings::default());
        let custom = parse(&[
            "project.mivmv.json",
            "out.mp4",
            "--encoder",
            "nvenc",
            "--video-bitrate",
            "8000000",
            "--frame-hash",
        ])
        .unwrap();
        assert_eq!(custom.settings.encoder, EncoderPreference::Nvenc);
        assert_eq!(custom.settings.video_bitrate_bps, 8_000_000);
        assert!(custom.settings.compute_frame_hash);
        for invalid in [
            vec![],
            vec!["project"],
            vec!["project", "out", "--encoder"],
            vec!["project", "out", "--video-bitrate", "0"],
            vec!["project", "out", "--bad"],
        ] {
            assert!(parse(&invalid).is_err());
        }
    }
}
