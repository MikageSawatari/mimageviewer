//! Headless H3a cold seek and downstream-edit measurement, 5 min / 48 kHz / 60 fps.
use musicvideo_core::{
    AnalysisKey, AnalysisTimeline, BandCalibration, FrameGrid, ModulatorSource, Smoothing,
    default_project, new_band_energy_modulator, pcm_view,
};
use std::time::Instant;
fn main() {
    let rate = 48_000;
    let pcm: Vec<f32> = (0..300 * rate)
        .flat_map(|n| {
            let t = n as f32 / rate as f32;
            let v = (std::f32::consts::TAU * (55.0 * t + 2.0 * t * t)).sin() * 0.5;
            [v, v]
        })
        .collect();
    let grid = FrameGrid::new(60.0, 300.0).unwrap();
    let view = pcm_view(&pcm, rate, 300.0).unwrap();
    let mut timeline = AnalysisTimeline::default();
    for gain in [0.0, 3.0] {
        let key = AnalysisKey::new(
            1u64,
            rate,
            view.logical_frames,
            grid,
            118,
            Smoothing::default(),
            &[],
            &[],
            gain,
        )
        .unwrap();
        let started = Instant::now();
        std::hint::black_box(timeline.frame(&key, view, 14_400, &|| false).unwrap());
        println!(
            "gain={gain} cold_or_edit_secs={:.6}",
            started.elapsed().as_secs_f64()
        );
    }

    // The same PCM and output lattice, with manual ranges: no calibration scan
    // participates in these timings. Only the downstream definition changes.
    let project = default_project(300.0);
    let mut modulator = new_band_energy_modulator(&project);
    match &mut modulator.source {
        ModulatorSource::BandEnergy { calibration, .. }
        | ModulatorSource::BandOnset { calibration, .. } => *calibration = BandCalibration::Manual,
    }
    let mut timeline = AnalysisTimeline::default();
    for floor_value in [0.03, 0.08] {
        match &mut modulator.source {
            ModulatorSource::BandEnergy { floor, .. }
            | ModulatorSource::BandOnset { floor, .. } => *floor = floor_value,
        }
        let key = AnalysisKey::new(
            1u64,
            rate,
            view.logical_frames,
            grid,
            118,
            Smoothing::default(),
            std::slice::from_ref(&modulator),
            &[],
            0.0,
        )
        .unwrap();
        let started = Instant::now();
        std::hint::black_box(timeline.frame(&key, view, 14_400, &|| false).unwrap());
        println!(
            "manual_floor={floor_value} cold_or_edit_secs={:.6}",
            started.elapsed().as_secs_f64()
        );
    }
}
