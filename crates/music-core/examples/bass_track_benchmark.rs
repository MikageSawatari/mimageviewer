//! Release-only measurement of four-worker timeline bass and marker agreement.
//! cargo run --release -p music-core --example bass_track_benchmark
//! --seconds 180 3600 selects durations; --compare-only skips performance runs.
//! --pcm <48kHz-stereo-f32le-file> measures an independently decoded real song.
use music_core::pitch::{HARMNET_HOP_SAMPLES, HARMNET_LONG_WINDOW_SAMPLES, HarmNetTracker};
use music_core::{
    BassEstimate, TimelineBassTrack, calculate_timeline_bass_segment, pitch_frame_at_secs,
    select_bass,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const SR: usize = 48_000;

fn add_tones(stereo: &mut [f32], first: f64, last: f64, tones: &[(u8, usize, f64)], gain: f64) {
    let start = (first * SR as f64).round() as usize;
    let end = (last * SR as f64).round() as usize;
    for (frame, pair) in stereo.chunks_exact_mut(2).enumerate().take(end).skip(start) {
        let fade = 1.0_f64.min((frame - start).min(end - frame) as f64 / 240.0);
        let mut value = 0.0;
        for &(midi, harmonics, exponent) in tones {
            let hz = 440.0 * ((midi as f64 - 69.0) / 12.0).exp2();
            for h in 1..=harmonics {
                value += gain
                    * (h as f64).powf(-exponent)
                    * (std::f64::consts::TAU * hz * h as f64 * frame as f64 / SR as f64).sin()
                    * fade;
            }
        }
        pair[0] += value as f32;
        pair[1] += value as f32;
    }
}

fn long_pcm(seconds: usize) -> Vec<f32> {
    let mut template = vec![0.0; SR * 20 * 2];
    add_tones(&mut template, 0.0, 9.0, &[(40, 10, 1.0)], 0.1);
    add_tones(
        &mut template,
        10.0,
        20.0,
        &[(43, 8, 1.0), (60, 6, 2.0), (64, 6, 2.0)],
        0.1,
    );
    let mut stereo = vec![0.0; SR * seconds * 2];
    for block in stereo.chunks_mut(template.len()) {
        block.copy_from_slice(&template[..block.len()]);
    }
    stereo
}

fn parallel_track(stereo: &[f32], label: &str) -> TimelineBassTrack {
    let mut track = TimelineBassTrack::new(stereo.len() / 2, SR as u32);
    let segments = track.segment_count();
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    let next = AtomicUsize::new(0);
    let start = Instant::now();
    let mut first_result = None;
    let mut publications = 0;
    let mut clone_time = Duration::ZERO;
    let mut last_publish = start;
    let mut last_snapshot = None;
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        for _ in 0..threads {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || {
                lower_thread_priority();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= segments {
                        break;
                    }
                    let result =
                        calculate_timeline_bass_segment(stereo, SR as u32, index, &|| false)
                            .unwrap();
                    if tx.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (completed, segment) in rx.into_iter().enumerate() {
            first_result.get_or_insert_with(|| start.elapsed());
            assert!(track.merge_segment(segment));
            if last_publish.elapsed() >= Duration::from_millis(250) || completed + 1 == segments {
                let clone_start = Instant::now();
                last_snapshot = Some(track.clone());
                clone_time += clone_start.elapsed();
                publications += 1;
                last_publish = Instant::now();
            }
        }
    });
    assert_eq!(track.computed_frames(), track.frame_count());
    let snapshot = last_snapshot.unwrap();
    assert_eq!(snapshot.computed_frames(), track.frame_count());
    println!(
        "performance,label={label},seconds={:.3},workers={threads},elapsed_ms={:.3},first_result_ms={:.3},frames={},segments={segments},pcm_bytes={},track_bytes={},two_tracks_bytes={},publications={publications},clone_ms={:.3}",
        stereo.len() as f64 / (SR * 2) as f64,
        start.elapsed().as_secs_f64() * 1000.0,
        first_result.unwrap_or_default().as_secs_f64() * 1000.0,
        track.frame_count(),
        std::mem::size_of_val(stereo),
        track.storage_bytes(),
        track.storage_bytes() + snapshot.storage_bytes(),
        clone_time.as_secs_f64() * 1000.0
    );
    track
}

fn push(tracker: &mut HarmNetTracker, stereo: &[f32], mono: &mut [f32], frame: usize) {
    let end = frame * HARMNET_HOP_SAMPLES;
    for (offset, sample) in mono.iter_mut().enumerate() {
        *sample = end
            .checked_add(offset)
            .and_then(|n| n.checked_sub(HARMNET_LONG_WINDOW_SAMPLES))
            .and_then(|n| stereo.get(n * 2..n * 2 + 2))
            .map_or(0.0, |pair| {
                let left = if pair[0].is_finite() { pair[0] } else { 0.0 };
                let right = if pair[1].is_finite() { pair[1] } else { 0.0 };
                0.5 * left + 0.5 * right
            });
    }
    tracker.push_frame(mono);
}

#[derive(Default)]
struct Agreement {
    samples: usize,
    mismatches: usize,
    current_run: usize,
    longest_run: usize,
    previous_index: Option<usize>,
}

impl Agreement {
    fn observe(&mut self, index: usize, different: bool) {
        if self.previous_index != index.checked_sub(1) {
            self.current_run = 0;
        }
        self.samples += 1;
        if different {
            self.mismatches += 1;
            self.current_run += 1;
            self.longest_run = self.longest_run.max(self.current_run);
        } else {
            self.current_run = 0;
        }
        self.previous_index = Some(index);
    }

    fn print(&self, mode: &str, scope: &str) {
        println!(
            "agreement,mode={mode},scope={scope},samples={},mismatches={},rate_percent={:.4},total_sampled_mismatch_secs={:.3},longest_contiguous_sampled_secs={:.3}",
            self.samples,
            self.mismatches,
            100.0 * self.mismatches as f64 / self.samples.max(1) as f64,
            self.mismatches as f64 * 0.02,
            self.longest_run as f64 * 0.02
        );
    }
}

fn compare_markers() {
    let mut stereo = vec![0.0; SR * 42 * 2];
    add_tones(&mut stereo, 0.0, 10.0, &[(40, 10, 1.0)], 0.1);
    add_tones(
        &mut stereo,
        11.0,
        20.05,
        &[(43, 8, 1.0), (60, 6, 2.0), (64, 6, 2.0)],
        0.1,
    );
    add_tones(&mut stereo, 20.05, 30.0, &[(38, 10, 1.0)], 0.1);
    for step in 0..20 {
        // Low-level harmonic bass probes the selection threshold, including silence.
        let gain = 10.0_f64.powf(-5.0 + step as f64 * 0.2);
        add_tones(
            &mut stereo,
            30.0 + step as f64 * 0.5,
            30.5 + step as f64 * 0.5,
            &[(40, 10, 1.0)],
            gain,
        );
    }
    let track = parallel_track(&stereo, "agreement-fixture");
    let mut tracker = HarmNetTracker::new(SR as u32);
    let mut mono = vec![0.0; HARMNET_LONG_WINDOW_SAMPLES];
    let mut continuous = vec![(BassEstimate::default(), 0.0); track.frame_count()];
    for frame in 1..=track.frame_count() + 8 {
        push(&mut tracker, &stereo, &mut mono, frame);
        if frame >= 9 {
            let probabilities = tracker.probabilities(3);
            continuous[frame - 9] = (
                select_bass(&probabilities),
                probabilities[..35].iter().copied().fold(0.0, f32::max),
            );
        }
    }
    for seek in [false, true] {
        let mut all = Agreement::default();
        let mut switches = Agreement::default();
        let mut boundary = Agreement::default();
        let mut threshold = Agreement::default();
        for (index, &(continuous_bass, continuous_peak)) in continuous.iter().enumerate() {
            let secs = index as f64 * 0.02;
            let Some(timeline) = track.at_secs(secs) else {
                continue;
            };
            let near_switch = [10.0_f64, 11.0, 20.05, 30.0, 40.0]
                .iter()
                .any(|t| (secs - t).abs() <= 0.5);
            let near_boundary = [20.0_f64, 40.0].iter().any(|t| (secs - t).abs() <= 0.5);
            let threshold_window = (30.0..40.0).contains(&secs);
            // Seek mode samples contiguous switch/boundary/level-sweep windows.
            if seek && !(near_switch || near_boundary || threshold_window) {
                continue;
            }
            let (marker, peak) = if seek {
                tracker.reset();
                let end = pitch_frame_at_secs(secs) as usize + 8;
                for frame in end.saturating_sub(58).max(1)..=end {
                    push(&mut tracker, &stereo, &mut mono, frame);
                }
                let probabilities = tracker.probabilities(3);
                (
                    select_bass(&probabilities),
                    probabilities[..35].iter().copied().fold(0.0, f32::max),
                )
            } else {
                (continuous_bass, continuous_peak)
            };
            let different = marker.midi != timeline.midi;
            all.observe(index, different);
            if near_switch {
                switches.observe(index, different);
            }
            if near_boundary {
                boundary.observe(index, different);
            }
            if threshold_window
                && ((0.35..=0.65).contains(&peak)
                    || (0.35..=0.65).contains(&continuous_peak)
                    || (0.35..=0.65).contains(&timeline.confidence))
            {
                threshold.observe(index, different);
            }
        }
        let mode = if seek { "seek-warmup" } else { "continuous" };
        all.print(mode, "all-sampled");
        switches.print(mode, "note-switch-plus-minus-0.5s");
        boundary.print(mode, "segment-boundary-plus-minus-0.5s");
        threshold.print(mode, "level-sweep-probability-0.35-to-0.65");
    }
}

fn main() {
    if cfg!(debug_assertions) {
        panic!("Run this example with --release");
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(index) = args.iter().position(|arg| arg == "--pcm") {
        let path = args.get(index + 1).expect("--pcm requires a file path");
        let bytes = std::fs::read(path).expect("read PCM file");
        assert_eq!(
            bytes.len() % 8,
            0,
            "PCM must contain complete stereo f32 frames"
        );
        let pcm: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert!(pcm.iter().all(|sample| sample.is_finite()));
        parallel_track(&pcm, "real-song-pcm");
    } else if !args.iter().any(|arg| arg == "--compare-only") {
        let seconds: Vec<usize> = args.iter().filter_map(|arg| arg.parse().ok()).collect();
        for seconds in if seconds.is_empty() {
            vec![180, 3600]
        } else {
            seconds
        } {
            assert!(seconds > 0);
            let stereo = long_pcm(seconds);
            parallel_track(&stereo, "repeated-synthetic-20s");
        }
    }
    if !args.iter().any(|arg| arg == "--no-compare") {
        compare_markers();
    }
}

fn lower_thread_priority() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> *mut std::ffi::c_void;
            fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
        }
        // Same BELOW_NORMAL (-1) setting as the production segment workers.
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -1) }, 0);
    }
}
