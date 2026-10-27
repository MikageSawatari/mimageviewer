use music_core::pitch::{
    HARMNET_HEAD_AGES, HARMNET_HOP_SAMPLES, HARMNET_LONG_WINDOW_SAMPLES, HARMNET_PITCH_COUNT,
    HarmNetTracker,
};
use sha2::{Digest, Sha256};

const SR: usize = 48_000;
const GOLDEN_TOLERANCE: f32 = 1.0e-3;

fn segment(signal: &mut [f32], start: f64, end: f64, tones: &[(i32, usize, f64)]) {
    let a = (start * SR as f64).round() as usize;
    let b = (end * SR as f64).round() as usize;
    for (n, sample) in signal.iter_mut().enumerate().take(b).skip(a) {
        let fade = 1.0_f64.min(((n - a).min(b - n)) as f64 / 240.0);
        let mut value = f64::from(*sample);
        for &(midi, harmonics, exponent) in tones {
            let frequency = 440.0 * ((midi as f64 - 69.0) / 12.0).exp2();
            for h in 1..=harmonics {
                value += 0.1
                    * (h as f64).powf(-exponent)
                    * (2.0 * std::f64::consts::PI * frequency * h as f64 * n as f64 / SR as f64)
                        .sin()
                    * fade;
            }
        }
        *sample = value as f32;
    }
}

fn golden_signal() -> Vec<f32> {
    let mut signal = vec![0.0; SR * 28 / 10];
    segment(&mut signal, 0.30, 1.10, &[(57, 8, 1.0)]);
    segment(
        &mut signal,
        1.10,
        1.70,
        &[(60, 6, 1.3), (64, 6, 1.3), (67, 6, 1.3)],
    );
    segment(&mut signal, 2.00, 2.60, &[(40, 10, 1.0)]);
    signal
}

fn push(tracker: &mut HarmNetTracker, signal: &[f32], frame: usize) {
    let end = frame * HARMNET_HOP_SAMPLES;
    tracker.push_frame(&signal[end.saturating_sub(HARMNET_LONG_WINDOW_SAMPLES)..end]);
}

fn difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "comparison lengths must match");
    for (name, values) in [("actual", a), ("expected", b)] {
        for (index, value) in values.iter().enumerate() {
            assert!(value.is_finite(), "{name}[{index}] is not finite: {value}");
        }
    }
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

#[test]
fn golden_comparison_rejects_nonfinite_actual_and_expected_values() {
    let finite = [0.25; HARMNET_PITCH_COUNT];
    assert_eq!(difference(&finite, &finite), 0.0);
    for index in [0, HARMNET_PITCH_COUNT / 2, HARMNET_PITCH_COUNT - 1] {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut invalid = finite;
            invalid[index] = value;
            for (actual, expected) in [(&invalid, &finite), (&finite, &invalid)] {
                assert!(
                    std::panic::catch_unwind(|| difference(actual, expected)).is_err(),
                    "nonfinite value at key {index} must fail the golden comparison"
                );
            }
        }
    }
}

#[test]
fn embedded_weights_match_effetune_provenance() {
    let weights = include_bytes!("../assets/harmnet_l_effetune_dsp_0.13.0.bin");
    assert_eq!(weights.len(), 188_048);
    assert_eq!(weights.len() / 4, 47_012);
    assert_eq!(
        format!("{:x}", Sha256::digest(weights)),
        "0164ecb8b4eb9143a20255669612aed306c9db9b670d892de1aee7492051b6a3"
    );
}

#[test]
fn golden_age_zero_and_eight_match_python_including_silence_boundaries() {
    let bytes = include_bytes!("fixtures/harmnet_golden_effetune_dsp_0.13.0.bin");
    let frames = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 4 + frames * 2 * HARMNET_PITCH_COUNT * 4);
    let expected: Vec<f32> = bytes[4..]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let signal = golden_signal();
    let mut tracker = HarmNetTracker::new(SR as u32);
    let mut maximum = [0.0_f32; 2];
    for frame in 1..=frames {
        push(&mut tracker, &signal, frame);
        for (index, head) in [0, 3].into_iter().enumerate() {
            let offset = (index * frames + frame - 1) * HARMNET_PITCH_COUNT;
            let actual = tracker.probabilities(head);
            let diff = difference(&actual, &expected[offset..offset + HARMNET_PITCH_COUNT]);
            maximum[index] = maximum[index].max(diff);
            assert!(
                diff <= GOLDEN_TOLERANCE,
                "L={frame}, age={}, maximum difference {diff}",
                HARMNET_HEAD_AGES[head]
            );
        }
    }
    eprintln!(
        "HarmNet golden max difference: age0={:.9}, age8={:.9}",
        maximum[0], maximum[1]
    );
}

#[test]
fn target_frame_silence_follows_each_head_age() {
    let mut tracker = HarmNetTracker::new(SR as u32);
    let tone: Vec<f32> = (0..HARMNET_LONG_WINDOW_SAMPLES)
        .map(|n| (2.0 * std::f32::consts::PI * 220.0 * n as f32 / SR as f32).sin() * 0.1)
        .collect();
    for _ in 0..12 {
        tracker.push_frame(&tone);
    }
    for silent_frames in 1..=9 {
        tracker.push_frame(&[]);
        for (head, age) in HARMNET_HEAD_AGES.into_iter().enumerate() {
            let probabilities = tracker.probabilities(head);
            if silent_frames > age {
                assert_eq!(probabilities, [0.0; HARMNET_PITCH_COUNT]);
            } else {
                assert!(
                    probabilities.iter().any(|p| *p > 0.0),
                    "head {head} prematurely gated"
                );
            }
        }
    }
    tracker.reset();
    for voiced_frames in 1..=9 {
        tracker.push_frame(&tone);
        for (head, age) in HARMNET_HEAD_AGES.into_iter().enumerate() {
            if voiced_frames <= age {
                assert_eq!(tracker.probabilities(head), [0.0; HARMNET_PITCH_COUNT]);
            } else {
                assert!(tracker.probabilities(head).iter().any(|p| *p > 0.0));
            }
        }
    }
}

#[test]
fn reset_restores_sequence_and_short_history_matches_left_padding() {
    let signal = golden_signal();
    let mut tracker = HarmNetTracker::new(SR as u32);
    for frame in 1..=40 {
        push(&mut tracker, &signal, frame);
    }
    let first = tracker.probabilities(3);
    tracker.reset();
    let mut padded = vec![0.0; HARMNET_LONG_WINDOW_SAMPLES];
    padded.extend_from_slice(&signal);
    for frame in 1..=40 {
        let end = HARMNET_LONG_WINDOW_SAMPLES + frame * HARMNET_HOP_SAMPLES;
        tracker.push_frame(&padded[end - HARMNET_LONG_WINDOW_SAMPLES..end]);
    }
    assert_eq!(tracker.probabilities(3), first);
}

#[test]
fn unsupported_rates_are_valid_zero_results() {
    for rate in [0, 8_000, 44_100, 96_000, u32::MAX] {
        let mut tracker = HarmNetTracker::new(rate);
        tracker.push_frame(&[0.1; 256]);
        for head in 0..4 {
            assert_eq!(tracker.probabilities(head), [0.0; HARMNET_PITCH_COUNT]);
        }
        tracker.reset();
        assert_eq!(tracker.probabilities(3), [0.0; HARMNET_PITCH_COUNT]);
    }
}

fn active_midi(probabilities: &[f32]) -> Vec<u8> {
    probabilities
        .iter()
        .enumerate()
        .filter_map(|(pitch, probability)| (*probability >= 0.5).then_some(pitch as u8 + 21))
        .collect()
}

#[test]
fn restart_warmup_preserves_correct_active_keys_for_four_signal_cases() {
    // The design owner chose active-key agreement, rather than a GRU convergence
    // bound. The warmup chord uses six harmonics with h^-2 amplitudes; the
    // independent Python golden retains its original h^-1.3 chord.
    // The request targets 3.42s (or 7.42s with four more seconds of history),
    // at least 0.3s after each onset/chord switch/reattack. A restart processes
    // from=end-58 inclusive: 59 frames over 58 hop intervals.
    let cases = [
        ("sustained", vec![(0.0, 4.0, vec![(57, 8, 1.0)])], vec![57]),
        (
            "chord_switch",
            vec![
                (0.0, 2.6, vec![(60, 6, 2.0), (64, 6, 2.0), (67, 6, 2.0)]),
                (2.6, 4.0, vec![(62, 6, 2.0), (65, 6, 2.0), (69, 6, 2.0)]),
            ],
            vec![62, 65, 69],
        ),
        ("onset", vec![(2.5, 4.0, vec![(57, 8, 1.0)])], vec![57]),
        (
            "silence_reattack",
            vec![
                (0.0, 1.8, vec![(40, 10, 1.0)]),
                (2.6, 4.0, vec![(40, 10, 1.0)]),
            ],
            vec![40],
        ),
    ];
    let mut measurements = Vec::new();
    for history_seconds in [0, 4] {
        for (name, segments, expected) in &cases {
            let mut signal = vec![0.0; SR * (4 + history_seconds)];
            for (start, stop, tones) in segments {
                let shifted_start = if *start == 0.0 {
                    0.0
                } else {
                    start + history_seconds as f64
                };
                segment(
                    &mut signal,
                    shifted_start,
                    stop + history_seconds as f64,
                    tones,
                );
            }
            let end = 180 + history_seconds * 50;
            let mut continuous = HarmNetTracker::new(SR as u32);
            let mut restarted = HarmNetTracker::new(SR as u32);
            for frame in 1..=end {
                push(&mut continuous, &signal, frame);
                if frame >= end - 58 {
                    push(&mut restarted, &signal, frame);
                }
            }
            let a = continuous.probabilities(3);
            let b = restarted.probabilities(3);
            let continuous_keys = active_midi(&a);
            let restart_keys = active_midi(&b);
            let diff = difference(&a, &b);
            eprintln!(
                "HarmNet warmup {name}, extra_history={history_seconds}s, L={end}: continuous={continuous_keys:?}, restart={restart_keys:?}, expected={expected:?}, reference_maxdiff={diff:.9}"
            );
            measurements.push((
                *name,
                history_seconds,
                expected.clone(),
                continuous_keys,
                restart_keys,
            ));
        }
    }
    for (name, history_seconds, expected, continuous, restart) in measurements {
        assert_eq!(
            continuous, expected,
            "{name}, extra_history={history_seconds}s, continuous"
        );
        assert_eq!(
            restart, expected,
            "{name}, extra_history={history_seconds}s, restart"
        );
    }
}
