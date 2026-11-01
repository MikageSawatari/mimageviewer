//! H3b: observe the real preview worker and adoption, and export the same snapshot.
//! No window, playback device, source-file decode, or alternate renderer is used.
use super::*;
use crate::mp4::{ExportSettings, export_snapshot, snapshot};
use mimageviewer::video::mvexport::{EncoderPreference, ExportSummary};
use musicvideo_core::{
    Animatable, BandCalibration, BandRange, BandWeighting, Fill, ImageRef, Interp, Keyframe,
    ModulationBinding, ModulationCurve, ModulationTarget, ModulatorSource, NormRect, Rgba8,
    SpectrumShape, Track, new_band_energy_modulator, new_linear_spectrum_layer,
    new_pitch_keyboard_layer, new_radial_spectrum_layer, new_text_layer,
};
use sha2::{Digest, Sha256};

const SOURCE_DURATION: f64 = 2.4;

fn fixture(duration: f64) -> MusicVideoLabApp {
    let font = crate::project::load_default_font().expect("H3b requires a Japanese font");
    let assets = crate::project::create_assets(Some(&font)).unwrap();
    // Translucent source pixels are composited onto the rasterizer's opaque black canvas.
    assets.insert_image(
        ImageId::new(BACKGROUND_ID),
        ImageAsset::new(2, 2, vec![27, 63, 191, 173].repeat(4)).unwrap(),
    );
    let mut app = MusicVideoLabApp::new(assets);
    app.project = default_project(duration);
    app.project.output.width = 160;
    app.project.output.height = 90;
    app.project.output.fps = 60.0;
    app.project.audio = Some(AudioRef {
        path: "synthetic.wav".into(),
    });
    app.project.images.push(ImageRef {
        id: ImageId::new(BACKGROUND_ID),
        path: "synthetic.png".into(),
    });
    app.project
        .layers
        .retain(|layer| !matches!(layer.kind, LayerKind::Spectrum(_)));
    for layer in &mut app.project.layers {
        layer.glow = Animatable::Const(None);
        layer.corner_radius = Animatable::Const(1.0);
        match &mut layer.kind {
            LayerKind::TimeText(text) => {
                layer.rect = Animatable::Const(NormRect::new(0.5, 0.55, 0.49, 0.2));
                text.size_px = 11.0;
                text.fill = Animatable::Const(Fill::Solid(Rgba8::WHITE));
            }
            LayerKind::ProgressBar(bar) => bar.border_thickness_px = 1.0,
            _ => {}
        }
    }
    // A moving image makes using transport time instead of time(n) observable,
    // even at adjacent floating-point values around a frame boundary.
    app.project.layers[0].rect = Animatable::Track(
        Track::new(vec![
            Keyframe {
                t: 0.0,
                value: NormRect::FULL,
                interp: Interp::Linear,
            },
            Keyframe {
                t: SOURCE_DURATION,
                value: NormRect::new(0.1, 0.0, 0.9, 1.0),
                interp: Interp::Linear,
            },
        ])
        .unwrap(),
    );
    let mut spectrum = new_linear_spectrum_layer(&app.project);
    spectrum.rect = Animatable::Const(NormRect::new(0.03, 0.05, 0.94, 0.3));
    if let LayerKind::Spectrum(s) = &mut spectrum.kind {
        s.band_count = 32;
    }
    app.project.layers.push(spectrum);
    app.project
        .layers
        .push(new_pitch_keyboard_layer(&app.project));
    let mut text = new_text_layer(&app.project);
    text.rect = Animatable::Const(NormRect::new(0.02, 0.4, 0.6, 0.2));
    if let LayerKind::Text(t) = &mut text.kind {
        t.text = "画素 H3b".into();
        t.size_px = 12.0;
    }
    app.project.layers.push(text);

    let onset = new_band_energy_modulator(&app.project);
    app.project.modulators.push(onset);
    let mut energy = new_band_energy_modulator(&app.project);
    energy.source = ModulatorSource::BandEnergy {
        range: BandRange::Hz {
            low_hz: 400.0,
            high_hz: 520.0,
        },
        smoothing: Smoothing::Average { factor: 0.65 },
        weighting: BandWeighting::Raw,
        gain_db: 0.0,
        floor: 0.0,
        ceiling: 1.0,
        calibration: BandCalibration::Auto {
            low_percentile: 5.0,
            high_percentile: 95.0,
        },
        threshold: 0.0,
    };
    app.project.modulators.push(energy);
    for (layer_index, modulator_index, target, amount) in [
        (0, 1, ModulationTarget::Opacity, -0.8),
        (3, 0, ModulationTarget::Scale, 0.4),
        (5, 0, ModulationTarget::Rotation, 15.0),
    ] {
        app.project.layers[layer_index]
            .modulations
            .push(ModulationBinding {
                target,
                modulator: app.project.modulators[modulator_index].id,
                amount,
                curve: ModulationCurve::Linear,
            });
    }
    let rate = 48_000;
    let audio = Arc::new(AudioData {
        stereo_pcm: (0..(SOURCE_DURATION * f64::from(rate)) as usize)
            .flat_map(|n| {
                let t = n as f32 / rate as f32;
                let pulse = (-18.0 * (t % 0.31)).exp();
                // The late loud section detects calibration and PCM-view truncation.
                let gain = if t > 1.45 { 0.65 } else { 0.015 + 0.09 * pulse };
                let v = gain
                    * ((t * std::f32::consts::TAU * 440.0).sin()
                        + pulse * (t * std::f32::consts::TAU * 73.0).sin());
                [v, v * 0.7]
            })
            .collect(),
        sample_rate: rate,
        duration_secs: SOURCE_DURATION,
    });
    let mut manifest = ResourceManifest::default();
    manifest.set_audio(Some(ResourceLease::new(Arc::clone(&audio))));
    app.editor_state
        .reset_for_project_with_resources(&app.project, manifest.clone());
    app.runtime_resources = RuntimeResourceState::Ready(manifest);
    app.audio = Some(audio);
    app
}

fn calibrate(app: &mut MusicVideoLabApp, ctx: &egui::Context) {
    app.start_calibration_if_needed(ctx);
    if let Some(pending) = app.calibration_pending.take() {
        let result = pending
            .rx
            .recv_timeout(Duration::from_secs(30))
            .expect("calibration worker");
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        app.calibration_pending = Some(CalibrationPending { rx, ..pending });
        app.poll_calibration();
    }
    assert!(app.calibration_ready(), "{}", app.status);
    assert_eq!(app.modulator_calibrations.len(), 2);
}

fn export(app: &MusicVideoLabApp, settings: ExportSettings) -> ExportSummary {
    let directory = tempfile::tempdir().unwrap();
    let audio = app.audio.clone().unwrap();
    let revision = app
        .editor_state
        .resource_manifest()
        .audio()
        .unwrap()
        .revision();
    let request = snapshot(
        &app.project,
        &app.assets,
        audio.clone(),
        revision,
        directory.path().join("parity.mp4"),
        settings,
        false,
    );
    assert!(Arc::ptr_eq(&request.audio, &audio));
    assert_eq!(request.pcm_id, revision);
    export_snapshot(request, &|_| {}, &AtomicBool::new(false)).unwrap()
}

fn hashes(app: &MusicVideoLabApp) -> Vec<[u8; 32]> {
    let summary = export(
        app,
        ExportSettings {
            encoder: EncoderPreference::OpenH264,
            video_bitrate_bps: 1_000_000,
            compute_frame_hash: false,
            compute_per_frame_hashes: true,
        },
    );
    assert!(
        summary.rgba_sha256.is_none(),
        "the two hash flags are independent"
    );
    let hashes = summary.rgba_frame_sha256.unwrap();
    assert_eq!(hashes.len() as u64, summary.frames);
    hashes
}

fn preview(app: &mut MusicVideoLabApp, ctx: &egui::Context, position: f64, seek: bool) -> [u8; 32] {
    if seek {
        app.seek_to(position);
    } else {
        app.position_secs = position;
        app.schedule_playback_preview();
    }
    calibrate(app, ctx);
    app.start_preview_if_needed(ctx);
    let pending = app.preview_pending.take().expect("preview admitted");
    let result = pending
        .rx
        .recv_timeout(Duration::from_secs(30))
        .expect("preview worker")
        .unwrap();
    assert!(result.report.unresolved_assets.is_empty());
    let grid = FrameGrid::new(app.project.output.fps, app.project.duration_secs).unwrap();
    assert_eq!(result.frame_index, grid.index_at(position));
    assert_eq!(
        result.time_secs.to_bits(),
        grid.time(result.frame_index).to_bits()
    );
    let expected_pixels = result
        .rgba
        .chunks_exact(4)
        .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
        .collect::<Vec<_>>();
    assert_eq!(result.image.size, [160, 90]);
    assert_eq!(result.image.pixels, expected_pixels);
    let hash = Sha256::digest(&result.rgba).into();
    let expected_time = result.time_secs;
    // Feed the worker result through the real admission/adoption path.
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(result)).unwrap();
    app.preview_pending = Some(PreviewPending { rx, ..pending });
    app.poll_preview(ctx);
    let displayed = app.displayed_preview.as_ref().expect("result adopted");
    assert_eq!(displayed.time_secs.to_bits(), expected_time.to_bits());
    assert_eq!(displayed.project, app.project);
    let mut delta = ctx.tex_manager().write().take_delta();
    let uploads = delta.set.get(&displayed.texture.id()).cloned();
    delta.clear();
    let uploads = uploads.expect("texture upload");
    assert_eq!(uploads.len(), 1);
    let egui::ImageData::Color(image) = &uploads[0].image;
    assert_eq!(
        image.pixels, expected_pixels,
        "adoption only uploads the worker image"
    );
    hash
}

fn compare(
    app: &mut MusicVideoLabApp,
    ctx: &egui::Context,
    hashes: &[[u8; 32]],
    requests: &[(f64, bool)],
) {
    let grid = FrameGrid::new(app.project.output.fps, app.project.duration_secs).unwrap();
    for &(position, seek) in requests {
        let n = grid.index_at(position);
        assert_eq!(
            preview(app, ctx, position, seek),
            hashes[n as usize],
            "preview/export mismatch: position={position:.17}, frame={n}, seek={seek}"
        );
    }
}

#[test]
fn output_fps_30_preview_matches_export_after_fps_change() {
    let mut app = fixture(SOURCE_DURATION);
    let ctx = egui::Context::default();
    // Prime the same timeline at 60 fps before changing the project.
    preview(&mut app, &ctx, 1.37, true);
    app.project.output.fps = 30.0;
    app.handle_editor_response(EditorResponse {
        project_changed: true,
        ..Default::default()
    });
    let reference = hashes(&app);
    let grid = FrameGrid::new(30.0, SOURCE_DURATION).unwrap();
    assert_eq!(reference.len(), 72);
    let mut requests = (0..grid.frames())
        .map(|n| (grid.time(n), false))
        .collect::<Vec<_>>();
    requests.extend([71, 2, 29, 30, 0, 59, 59].map(|n| (grid.time(n), true)));
    compare(&mut app, &ctx, &reference, &requests);
}

#[test]
fn h3b_forward_every_frame_matches_export() {
    let mut app = fixture(SOURCE_DURATION);
    let ctx = egui::Context::default();
    let reference = hashes(&app);
    let grid = FrameGrid::new(60.0, SOURCE_DURATION).unwrap();
    let requests = (0..grid.frames())
        .map(|n| (grid.time(n), false))
        .collect::<Vec<_>>();
    compare(&mut app, &ctx, &reference, &requests);
}

#[test]
fn p5a_h3b_scaled_radial_spectrum_preview_matches_export() {
    let mut app = fixture(0.4);
    let ctx = egui::Context::default();
    let mut radial = new_radial_spectrum_layer(&app.project);
    radial.rect = Animatable::Const(NormRect::new(0.3, 0.15, 0.4, 0.7));
    if let LayerKind::Spectrum(spectrum) = &mut radial.kind {
        spectrum.band_count = 32;
        if let SpectrumShape::Radial {
            inner_ratio,
            thickness_px,
            ..
        } = &mut spectrum.shape
        {
            *inner_ratio = 0.45;
            *thickness_px = 2.5;
        }
    }
    radial.modulations.push(ModulationBinding {
        target: ModulationTarget::Scale,
        modulator: app.project.modulators[1].id,
        amount: 0.7,
        curve: ModulationCurve::Linear,
    });
    app.project.layers = vec![radial];
    let reference = hashes(&app);

    // The assigned Scale must affect visible pixels, so identical unmodulated
    // paths in preview and export cannot make this parity check vacuous.
    let bindings = std::mem::take(&mut app.project.layers[0].modulations);
    let unscaled = hashes(&app);
    app.project.layers[0].modulations = bindings;
    assert_ne!(reference, unscaled, "radial Scale must change export RGBA");
    let changed_frame = reference
        .iter()
        .zip(&unscaled)
        .position(|(scaled, unscaled)| scaled != unscaled)
        .unwrap() as u64;
    let grid = FrameGrid::new(app.project.output.fps, app.project.duration_secs).unwrap();
    let mut requests = (0..grid.frames())
        .map(|n| (grid.time(n), false))
        .collect::<Vec<_>>();
    requests
        .extend([grid.frames() - 1, changed_frame, 0, changed_frame].map(|n| (grid.time(n), true)));
    compare(&mut app, &ctx, &reference, &requests);
}

#[test]
fn h3b_dropped_frames_and_boundary_positions_match_export() {
    let mut app = fixture(SOURCE_DURATION);
    let ctx = egui::Context::default();
    let reference = hashes(&app);
    let grid = FrameGrid::new(60.0, SOURCE_DURATION).unwrap();
    let mut requests = vec![(0.0, false)];
    let mut n = 0;
    for step in [2, 5, 3, 4].into_iter().cycle() {
        n += step;
        if n >= grid.frames() {
            break;
        }
        let time = grid.time(n);
        requests.extend([
            (time.next_down(), false),
            (time, false),
            (time.next_up(), false),
        ]);
    }
    // n=123 at 60fps is the documented floating-point correction case.
    requests.extend([
        (grid.time(123).next_down(), false),
        (grid.time(123), false),
        (grid.time(123).next_up(), false),
    ]);
    requests.sort_by(|left, right| left.0.total_cmp(&right.0));
    compare(&mut app, &ctx, &reference, &requests);
}

#[test]
fn h3b_backward_far_direct_checkpoint_and_repeat_match_export() {
    let mut app = fixture(SOURCE_DURATION);
    let ctx = egui::Context::default();
    let reference = hashes(&app);
    let grid = FrameGrid::new(60.0, SOURCE_DURATION).unwrap();
    // D3 takes a checkpoint after each ceil(fps) frames, including frame zero.
    let checkpoint = app.project.output.fps.ceil() as u64 - 1;
    let requests = [
        143, 2, 120, checkpoint, checkpoint, 0, 119, 118, 119, 1, 143,
    ]
    .map(|n| (grid.time(n), true));
    compare(&mut app, &ctx, &reference, &requests);
}

#[test]
fn h3b_settings_change_and_restore_match_each_snapshot_export() {
    let mut app = fixture(SOURCE_DURATION);
    let ctx = egui::Context::default();
    let original = app.project.clone();
    let reference = hashes(&app);
    compare(&mut app, &ctx, &reference, &[(1.37, true), (0.73, true)]);
    if let ModulatorSource::BandEnergy {
        gain_db, smoothing, ..
    } = &mut app.project.modulators[1].source
    {
        *gain_db = -8.0;
        *smoothing = Smoothing::Envelope {
            attack_secs: 0.04,
            release_secs: 0.3,
        };
    }
    app.handle_editor_response(EditorResponse {
        project_changed: true,
        ..Default::default()
    });
    let changed = hashes(&app);
    assert_ne!(
        reference, changed,
        "Average modulator alone must affect pixels"
    );
    let modulator_frame = reference
        .iter()
        .zip(&changed)
        .position(|(before, after)| before != after)
        .unwrap() as u64;
    let grid = FrameGrid::new(app.project.output.fps, app.project.duration_secs).unwrap();
    compare(
        &mut app,
        &ctx,
        &changed,
        &[(1.37, true), (0.73, true), (2.2, true)],
    );
    compare(
        &mut app,
        &ctx,
        &changed,
        &[(grid.time(modulator_frame), true)],
    );
    app.project = original.clone();
    app.project.display_gain_db = 12.0;
    app.handle_editor_response(EditorResponse {
        project_changed: true,
        ..Default::default()
    });
    let gain_changed = hashes(&app);
    assert_ne!(
        reference, gain_changed,
        "display gain alone must affect pixels"
    );
    // Gain may saturate at the fixed seek positions. Also request a frame whose
    // pixels demonstrably changed, so ignoring gain in preview cannot pass.
    let gain_frame = reference
        .iter()
        .zip(&gain_changed)
        .position(|(before, after)| before != after)
        .unwrap() as u64;
    compare(
        &mut app,
        &ctx,
        &gain_changed,
        &[(1.37, true), (0.73, true), (2.2, true)],
    );
    compare(
        &mut app,
        &ctx,
        &gain_changed,
        &[(grid.time(gain_frame), true)],
    );
    app.project = original;
    app.handle_editor_response(EditorResponse {
        project_changed: true,
        ..Default::default()
    });
    let restored = hashes(&app);
    assert_eq!(reference, restored);
    compare(
        &mut app,
        &ctx,
        &restored,
        &[(2.2, true), (0.73, true), (1.37, true)],
    );
    compare(&mut app, &ctx, &restored, &[(grid.time(gain_frame), true)]);
    compare(
        &mut app,
        &ctx,
        &restored,
        &[(grid.time(modulator_frame), true)],
    );
}

fn duration_parity(duration: f64) {
    let mut app = fixture(duration);
    let ctx = egui::Context::default();
    let reference = hashes(&app);
    let grid = FrameGrid::new(60.0, duration).unwrap();
    // Include windows near both ends and around the start of silent extension.
    let requests = [
        grid.frames() - 1,
        0,
        1,
        grid.frames() / 2,
        grid.frames() - 2,
        grid.index_at(SOURCE_DURATION),
        grid.index_at(SOURCE_DURATION - 0.02),
    ]
    .map(|n| (grid.time(n), true));
    compare(&mut app, &ctx, &reference, &requests);
}

#[test]
fn h3b_output_shorter_than_audio_matches_export() {
    duration_parity(1.4);
}
#[test]
fn h3b_output_longer_than_audio_matches_export_with_silence() {
    duration_parity(3.2);
}
#[test]
fn h3b_output_equal_to_audio_matches_export() {
    duration_parity(SOURCE_DURATION);
}

#[test]
fn h3b_hash_flags_are_opt_in_and_independent() {
    let app = fixture(0.05);
    let settings = ExportSettings {
        encoder: EncoderPreference::OpenH264,
        ..Default::default()
    };
    let normal = export(&app, settings);
    assert!(normal.rgba_sha256.is_none() && normal.rgba_frame_sha256.is_none());
    assert_eq!(normal.frame_hash_secs, 0.0);
    let sequence = export(
        &app,
        ExportSettings {
            compute_frame_hash: true,
            ..settings
        },
    );
    assert!(sequence.rgba_sha256.is_some() && sequence.rgba_frame_sha256.is_none());
    let both = export(
        &app,
        ExportSettings {
            compute_frame_hash: true,
            compute_per_frame_hashes: true,
            ..settings
        },
    );
    assert_eq!(both.rgba_sha256, sequence.rgba_sha256);
    assert_eq!(both.rgba_frame_sha256.unwrap().len() as u64, both.frames);
}

#[test]
fn h3b_fixture_features_each_affect_visible_pixels() {
    let mut app = fixture(SOURCE_DURATION);
    let ctx = egui::Context::default();
    let original = app.project.clone();
    let reference = hashes(&app);
    let positions = [0.73, 1.37, 2.2];
    let grid = FrameGrid::new(60.0, SOURCE_DURATION).unwrap();
    // These are deliberately different projects, not expected parity comparisons.
    // Prove that merely including a feature does not give vacuous coverage.
    for index in 0..original.layers.len() {
        app.project = original.clone();
        let removed = app.project.layers.remove(index);
        assert!(
            positions
                .iter()
                .any(|&position| preview(&mut app, &ctx, position, true)
                    != reference[grid.index_at(position) as usize]),
            "layer {} must affect visible pixels",
            removed.name
        );
    }
    for modulator in &original.modulators {
        app.project = original.clone();
        for layer in &mut app.project.layers {
            layer
                .modulations
                .retain(|binding| binding.modulator != modulator.id);
        }
        assert!(
            positions
                .iter()
                .any(|&position| preview(&mut app, &ctx, position, true)
                    != reference[grid.index_at(position) as usize]),
            "modulator {:?} must affect visible pixels",
            modulator.id
        );
    }
    for index in 0..original.modulators.len() {
        app.project = original.clone();
        match &mut app.project.modulators[index].source {
            ModulatorSource::BandEnergy { smoothing, .. }
            | ModulatorSource::BandOnset { smoothing, .. } => *smoothing = Smoothing::None,
        }
        assert!(
            positions
                .iter()
                .any(|&position| preview(&mut app, &ctx, position, true)
                    != reference[grid.index_at(position) as usize]),
            "smoothing of modulator {index} must affect visible pixels"
        );
    }
}

#[test]
fn h3b_preview_texture_only_converts_rgba_and_downscales() {
    let mut app = fixture(0.05);
    let ctx = egui::Context::default();
    let reference = hashes(&app);
    compare(&mut app, &ctx, &reference, &[(0.02, true)]);
    app.editor_state
        .select_layer(&app.project, app.project.layers.len());
    let texture_id = app.displayed_preview.as_ref().unwrap().texture.id();
    let output = crate::project::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(80.0, 60.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw_preview(ui));
        },
    ));
    let meshes = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture_id => Some(mesh),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(meshes.len(), 1);
    let mesh = meshes[0];
    assert_eq!(mesh.vertices.len(), 4);
    assert!(
        mesh.vertices
            .iter()
            .all(|v| v.color == egui::Color32::WHITE)
    );
    let bounds = mesh.calc_bounds();
    assert!(bounds.width() < 160.0 && bounds.height() < 90.0);
    assert!((bounds.width() / bounds.height() - 160.0 / 90.0).abs() < 0.00001);
    let uv_bounds = mesh
        .vertices
        .iter()
        .fold(egui::Rect::NOTHING, |mut rect, v| {
            rect.extend_with(v.uv);
            rect
        });
    assert_eq!(
        uv_bounds,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0))
    );
    for vertex in &mesh.vertices {
        let relative = (vertex.pos - bounds.min) / bounds.size();
        assert!((relative.x - vertex.uv.x).abs() < 0.000001);
        assert!((relative.y - vertex.uv.y).abs() < 0.000001);
    }
    assert!(
        !output
            .textures_delta
            .set
            .iter()
            .any(|(id, _)| *id == texture_id),
        "drawing does not rewrite pixels"
    );
}
