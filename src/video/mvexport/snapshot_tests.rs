//! Regression tests exercise the actual export worker with disposable sources.
use std::sync::{Arc, Mutex};

use comic_core::FontSet;
use musicvideo_core::{
    Animatable, Assets, AudioRef, BandCalibration, BandRange, BandWeighting,
    DEFAULT_BACKGROUND_IMAGE_ID, ImageAsset, ImageId, ImageRef, ModulationBinding, ModulationCurve,
    ModulationTarget, ModulatorSource, Project, default_project, new_band_energy_modulator,
};

use super::*;
use crate::video::mvexport::ExportPcm;

fn pcm() -> Arc<ExportPcm> {
    let rate = 48_000;
    let frames = 17_760;
    let samples = (0..frames)
        .flat_map(|n| {
            let gain = if n < 6000 { 0.003 } else { 0.11 };
            let signal = gain * (n as f32 * std::f32::consts::TAU * 440.0 / rate as f32).sin();
            [signal, -signal * 0.4]
        })
        .collect();
    Arc::new(ExportPcm {
        stereo_pcm: samples,
        sample_rate: rate,
        // Container metadata deliberately differs from actual decoded duration.
        duration_secs: 99.0,
    })
}

fn project(duration: f64) -> Project {
    let mut project = default_project(duration);
    project.output.width = 128;
    project.output.height = 72;
    project.output.fps = 20.0;
    project
        .layers
        .retain(|layer| matches!(layer.kind, LayerKind::Image { .. } | LayerKind::Spectrum(_)));
    let mut modulator = new_band_energy_modulator(&project);
    modulator.source = ModulatorSource::BandEnergy {
        range: BandRange::Hz {
            low_hz: 400.0,
            high_hz: 480.0,
        },
        smoothing: Smoothing::None,
        weighting: BandWeighting::Raw,
        gain_db: 0.0,
        floor: 0.0,
        ceiling: 1.0,
        calibration: BandCalibration::Auto {
            low_percentile: 0.1,
            high_percentile: 0.9,
        },
        threshold: 0.0,
    };
    let id = modulator.id;
    project.modulators.push(modulator);
    for layer in &mut project.layers {
        if matches!(layer.kind, LayerKind::Image { .. }) {
            layer.opacity = Animatable::Const(0.2);
            layer.modulations.push(ModulationBinding {
                target: ModulationTarget::Opacity,
                modulator: id,
                amount: 0.8,
                curve: ModulationCurve::Squared,
            });
        }
    }
    project
}

fn assets() -> Assets {
    let assets = Assets::new(FontSet::new());
    assets.insert_image(
        ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID),
        ImageAsset::new(2, 2, vec![27, 63, 191, 255].repeat(4)).unwrap(),
    );
    assets
}

fn request<I>(
    project: Project,
    assets: Assets,
    audio: Arc<ExportPcm>,
    pcm_id: I,
    output_path: &Path,
) -> MusicVideoExportRequest<I> {
    MusicVideoExportRequest {
        project,
        assets,
        source_duration_secs: audio.stereo_pcm.len() as f64 / 2.0 / audio.sample_rate as f64,
        audio,
        pcm_id,
        output_path: output_path.to_owned(),
        encoder: EncoderPreference::OpenH264,
        video_bitrate_bps: 1_000_000,
        replace_existing: false,
        compute_frame_hash: true,
        compute_per_frame_hashes: false,
    }
}

fn legacy_chunks(audio: &ExportPcm, duration: f64) -> Vec<ProcessedChunk> {
    let mut fitted = audio.stereo_pcm.clone();
    fitted.resize(
        (duration * audio.sample_rate as f64).round() as usize * 2,
        0.0,
    );
    fitted
        .chunks(AUDIO_CHUNK_SAMPLES_PER_CHANNEL * 2)
        .enumerate()
        .map(|(n, samples)| ProcessedChunk {
            samples: samples.to_vec(),
            audible_pts_secs: (n * AUDIO_CHUNK_SAMPLES_PER_CHANNEL) as f64
                / audio.sample_rate as f64,
            duration_secs: (samples.len() / 2) as f64 / audio.sample_rate as f64,
            source_secs_per_output_sec: 1.0,
            seek_serial: 0,
            pdc_latency_secs_at_process: 0.0,
            effetune_generation: None,
        })
        .collect()
}

fn legacy_audio_packets(audio: &ExportPcm, duration: f64) -> Vec<Vec<u8>> {
    let mut encoder = open_aac_encoder(
        audio.sample_rate,
        AUDIO_BITRATE_BPS,
        0,
        StreamTimeline::new(0.0).unwrap(),
    )
    .unwrap();
    let mut packets = Vec::new();
    for chunk in legacy_chunks(audio, duration) {
        packets.extend(encoder.push_chunk(chunk).unwrap());
    }
    packets.extend(encoder.finish().unwrap());
    packets
        .into_iter()
        .map(|p| p.data().unwrap().to_vec())
        .collect()
}

fn mp4_audio_packets(path: &Path) -> Vec<Vec<u8>> {
    let mut input = ffmpeg::format::input(path).unwrap();
    let audio_index = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .unwrap()
        .index();
    input
        .packets()
        .filter_map(|packet| {
            let (stream, packet) = packet.unwrap();
            (stream.index() == audio_index).then(|| packet.data().unwrap().to_vec())
        })
        .collect()
}

fn reference_hash(project: &Project, assets: &Assets, audio: &ExportPcm) -> String {
    let grid = FrameGrid::new(project.output.fps, project.duration_secs).unwrap();
    let view = pcm_view(&audio.stereo_pcm, audio.sample_rate, project.duration_secs).unwrap();
    let calibrations = calibrate_modulators_view(
        view,
        audio.sample_rate,
        project.duration_secs,
        audio.stereo_pcm.len() as f64 / 2.0 / audio.sample_rate as f64,
        &project.modulators,
        &|| false,
    )
    .unwrap();
    let bands = project
        .layers
        .iter()
        .filter_map(|l| match &l.kind {
            LayerKind::Spectrum(s) => Some(s.band_count),
            _ => None,
        })
        .max()
        .unwrap_or(1);
    let key = AnalysisKey::new(
        123_u64,
        audio.sample_rate,
        view.logical_frames,
        grid,
        bands,
        Smoothing::default(),
        &project.modulators,
        &calibrations,
        project.display_gain_db,
    )
    .unwrap();
    let mut timeline = AnalysisTimeline::default();
    let mut frame = Frame::new(project.output.width, project.output.height).unwrap();
    let mut hash = Sha256::new();
    for n in 0..grid.frames() {
        let spectrum = timeline.frame(&key, view, n, &|| false).unwrap();
        let scene = evaluate(project, grid.time(n), spectrum);
        assert!(
            render_frame(&scene, assets, &mut frame)
                .unwrap()
                .unresolved_assets
                .is_empty()
        );
        hash.update(&frame.rgba);
    }
    format!("{:x}", hash.finalize())
}

#[test]
fn view_chunks_match_legacy_fitting_bits_pts_and_bounded_size() {
    let mut audio = pcm();
    // Include signed zero, infinities and NaN payloads to test copying by bits.
    Arc::get_mut(&mut audio).unwrap().stereo_pcm[..6].copy_from_slice(&[
        -0.0,
        f32::from_bits(0x7fc01234),
        f32::INFINITY,
        -0.7,
        0.4,
        -f32::INFINITY,
    ]);
    for duration in [0.19, 0.37, 0.61] {
        let view = pcm_view(&audio.stereo_pcm, audio.sample_rate, duration).unwrap();
        let expected = legacy_chunks(&audio, duration);
        let actual: Vec<_> = (0..view.logical_frames)
            .step_by(AUDIO_CHUNK_SAMPLES_PER_CHANNEL)
            .map(|n| materialize_audio_chunk(view, audio.sample_rate, n))
            .collect();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert!(actual.samples.len() <= AUDIO_CHUNK_SAMPLES_PER_CHANNEL * 2);
            assert_eq!(
                actual
                    .samples
                    .iter()
                    .map(|s| s.to_bits())
                    .collect::<Vec<_>>(),
                expected
                    .samples
                    .iter()
                    .map(|s| s.to_bits())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                actual.audible_pts_secs.to_bits(),
                expected.audible_pts_secs.to_bits()
            );
            assert_eq!(
                actual.duration_secs.to_bits(),
                expected.duration_secs.to_bits()
            );
        }
    }
    assert_eq!(audio.stereo_pcm.len(), 17_760 * 2);
}

#[test]
fn snapshot_export_survives_source_deletion_and_matches_rgba_and_aac_reference() {
    let dir = tempfile::tempdir().unwrap();
    let audio_path = dir.path().join("source.wav");
    let image_path = dir.path().join("source.png");
    std::fs::write(&audio_path, b"source audio is already decoded").unwrap();
    std::fs::write(&image_path, b"source image is already decoded").unwrap();
    let mut project = project(0.61);
    project.audio = Some(AudioRef {
        path: audio_path.clone(),
    });
    let image_id = ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID);
    project.images.push(ImageRef {
        id: image_id.clone(),
        path: image_path.clone(),
    });
    let live_assets = assets();
    let snapshot_assets = live_assets.fork_retain_images(std::slice::from_ref(&image_id));
    assert!(Arc::ptr_eq(
        &snapshot_assets.image(&image_id).unwrap(),
        &live_assets.image(&image_id).unwrap()
    ));
    let audio = pcm();
    let expected_hash = reference_hash(&project, &snapshot_assets, &audio);
    let expected_audio = legacy_audio_packets(&audio, project.duration_secs);
    let output = dir.path().join("export.mp4");
    let events = Mutex::new(Vec::new());
    let summary = export(
        request(
            project,
            snapshot_assets,
            Arc::clone(&audio),
            42_u64,
            &output,
        ),
        &|p| {
            events.lock().unwrap().push(p);
            if p.stage == ExportStage::Calibration {
                std::fs::remove_file(&audio_path).unwrap();
                std::fs::remove_file(&image_path).unwrap();
                live_assets.insert_image(
                    image_id.clone(),
                    ImageAsset::new(2, 2, vec![255; 16]).unwrap(),
                );
            }
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(summary.rgba_sha256.unwrap(), expected_hash);
    assert_eq!(mp4_audio_packets(&output), expected_audio);
    let events = events.into_inner().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|p| p.stage)
            .fold(Vec::new(), |mut stages, stage| {
                if stages.last() != Some(&stage) {
                    stages.push(stage);
                }
                stages
            }),
        vec![
            ExportStage::Calibration,
            ExportStage::AudioEncoding,
            ExportStage::Frames,
            ExportStage::Finalizing
        ]
    );
    assert_eq!(events.last().unwrap().completed_frames, summary.frames);
    assert!(events.iter().all(|p| p.total_frames == summary.frames));
    assert_eq!(Arc::strong_count(&audio), 1);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[derive(Clone)]
struct Revision {
    value: u64,
    observed: Arc<Mutex<Vec<(u64, u64)>>>,
}
impl PartialEq for Revision {
    fn eq(&self, other: &Self) -> bool {
        self.observed
            .lock()
            .unwrap()
            .push((self.value, other.value));
        self.value == other.value
    }
}
impl Eq for Revision {}

#[test]
fn export_analysis_uses_supplied_revision_and_recomputes_calibration_after_pcm_restore() {
    let dir = tempfile::tempdir().unwrap();
    let original = pcm();
    // Undo can restore a different Arc with the old resource revision.
    let restored = Arc::new(ExportPcm {
        stereo_pcm: original.stereo_pcm.clone(),
        sample_rate: original.sample_rate,
        duration_secs: original.duration_secs,
    });
    assert!(!Arc::ptr_eq(&original, &restored));
    let project = project(0.19);
    let expected = reference_hash(&project, &assets(), &original);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let revision = Revision {
        value: 739,
        observed: Arc::clone(&observed),
    };
    let output = dir.path().join("restored.mp4");
    let summary = export(
        request(project.clone(), assets(), restored, revision, &output),
        &|_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(summary.rgba_sha256.unwrap(), expected);
    assert_eq!(
        mp4_audio_packets(&output),
        legacy_audio_packets(&original, project.duration_secs)
    );
    let observed = observed.lock().unwrap();
    assert!(
        !observed.is_empty(),
        "revision must reach the analysis timeline's key comparisons"
    );
    assert!(observed.iter().all(|pair| *pair == (739, 739)));
}

#[test]
fn atomic_publication_refuses_a_destination_created_after_admission() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("output.mp4");
    let partial = PartialOutput::new(&destination).unwrap();
    let path = partial.path().to_owned();
    assert!(
        path.is_file(),
        "temporary name is reserved, not merely guessed"
    );
    std::fs::write(&path, b"new mp4").unwrap();
    std::fs::write(&destination, b"racing destination").unwrap();
    assert!(
        partial
            .commit(&destination, false, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"racing destination");
    assert!(!path.exists());
}

#[test]
fn atomic_replacing_publication_preserves_original_until_commit() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("output.mp4");
    std::fs::write(&destination, b"original").unwrap();
    let partial = PartialOutput::new(&destination).unwrap();
    let path = partial.path().to_owned();
    std::fs::write(&path, b"replacement").unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    partial
        .commit(&destination, true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"replacement");
    assert!(!path.exists());
}

#[test]
fn publication_cancel_and_failure_leave_existing_destination_intact() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("output.mp4");
    std::fs::write(&destination, b"original").unwrap();
    let partial = PartialOutput::new(&destination).unwrap();
    let path = partial.path().to_owned();
    std::fs::write(&path, b"cancelled mp4").unwrap();
    assert_eq!(
        partial.commit(&destination, true, &AtomicBool::new(true)),
        Err(ExportError::Cancelled)
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert!(!path.exists());
    let partial = PartialOutput::new(&destination).unwrap();
    let path = partial.path().to_owned();
    // Deterministic publish failure, without manipulating actual user data.
    std::fs::remove_file(&path).unwrap();
    assert!(
        partial
            .commit(&destination, true, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
}

#[test]
fn finalizing_cancellation_and_render_failure_preserve_approved_existing_output() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("output.mp4");
    std::fs::write(&destination, b"original").unwrap();
    let cancel = AtomicBool::new(false);
    let mut req = request(project(0.19), assets(), pcm(), 11, &destination);
    req.replace_existing = true;
    let result = export(
        req,
        &|p| {
            if p.stage == ExportStage::Finalizing {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        &cancel,
    );
    assert_eq!(result, Err(ExportError::Cancelled));
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    let mut req = request(
        project(0.19),
        Assets::new(FontSet::new()),
        pcm(),
        12,
        &destination,
    );
    req.replace_existing = true;
    assert!(matches!(
        export(req, &|_| {}, &AtomicBool::new(false)),
        Err(ExportError::Render(_))
    ));
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn request_validation_rejects_invalid_pcm_and_requires_overwrite_approval() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("output.mp4");
    for invalid in 0..5 {
        let mut req = request(project(0.19), assets(), pcm(), 3, &destination);
        match invalid {
            0 => Arc::get_mut(&mut req.audio).unwrap().sample_rate = 0,
            1 => Arc::get_mut(&mut req.audio).unwrap().stereo_pcm.clear(),
            2 => {
                Arc::get_mut(&mut req.audio).unwrap().stereo_pcm.pop();
            }
            3 => req.source_duration_secs = f64::NAN,
            4 => req.source_duration_secs = 0.0,
            _ => unreachable!(),
        }
        assert!(
            matches!(validate_request(&req), Err(ExportError::InvalidRequest(_))),
            "case {invalid}"
        );
    }
    let mut req = request(project(0.19), assets(), pcm(), 3, &destination);
    assert!(validate_request(&req).is_ok());
    std::fs::write(&destination, b"existing").unwrap();
    assert!(matches!(
        validate_request(&req),
        Err(ExportError::InvalidRequest(_))
    ));
    req.replace_existing = true;
    assert!(validate_request(&req).is_ok());
    assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
}
