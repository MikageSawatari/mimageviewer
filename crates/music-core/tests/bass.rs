use music_core::{
    BassEstimate, TIMELINE_BASS_SEGMENT_FRAMES, TimelineBassSegment, TimelineBassTrack,
    calculate_timeline_bass_segment, pitch_frame_at_secs, select_bass,
};
use std::cell::Cell;

const SR: usize = 48_000;

fn add_tones(stereo: &mut [f32], start: f64, end: f64, tones: &[(u8, usize, f64)]) {
    let first = (start * SR as f64).round() as usize;
    let last = (end * SR as f64).round() as usize;
    for (frame, pair) in stereo
        .chunks_exact_mut(2)
        .enumerate()
        .take(last)
        .skip(first)
    {
        let fade = 1.0_f64.min((frame - first).min(last - frame) as f64 / 240.0);
        let mut value = 0.0;
        for &(midi, harmonics, exponent) in tones {
            let hz = 440.0 * ((midi as f64 - 69.0) / 12.0).exp2();
            for h in 1..=harmonics {
                value += 0.1
                    * (h as f64).powf(-exponent)
                    * (std::f64::consts::TAU * hz * h as f64 * frame as f64 / SR as f64).sin()
                    * fade;
            }
        }
        pair[0] += value as f32;
        pair[1] += value as f32;
    }
}

fn calculate(stereo: &[f32]) -> TimelineBassTrack {
    let mut track = TimelineBassTrack::new(stereo.len() / 2, SR as u32);
    for index in 0..track.segment_count() {
        assert!(track.merge_segment(
            calculate_timeline_bass_segment(stereo, SR as u32, index, &|| false).unwrap()
        ));
    }
    track
}

#[test]
fn selector_uses_inclusive_endpoints_threshold_and_lowest_octave() {
    assert_eq!(select_bass(&[]), BassEstimate::default());
    let mut probabilities = [0.0; 88];
    probabilities[35] = 1.0; // MIDI 56 is outside bass range.
    probabilities[87] = 1.0;
    probabilities[0] = 0.499_999;
    assert_eq!(select_bass(&probabilities), BassEstimate::default());
    probabilities[34] = 0.5;
    assert_eq!(
        select_bass(&probabilities),
        BassEstimate {
            midi: 55,
            confidence: 0.5
        }
    );
    probabilities[12] = 0.99; // Same pitch class, stronger octave/harmonic.
    probabilities[24] = 1.0;
    probabilities[0] = 0.5;
    assert_eq!(
        select_bass(&probabilities),
        BassEstimate {
            midi: 21,
            confidence: 0.5
        }
    );
    probabilities[0] = f32::NAN;
    assert_eq!(select_bass(&probabilities).midi, 33);
}

#[test]
fn frame_rounding_matches_spectrum_formula_and_bounds() {
    for position in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0] {
        assert_eq!(pitch_frame_at_secs(position), 1);
    }
    assert_eq!(pitch_frame_at_secs(0.009_999), 1);
    assert_eq!(pitch_frame_at_secs(0.01), 2);
    assert_eq!(pitch_frame_at_secs(19.989_999), 1000);
    // Preserve the existing floating-point formula even at a decimal tie.
    assert_eq!(
        pitch_frame_at_secs(19.99),
        ((19.99_f64 + 0.02) / 0.02).round() as i64
    );
    assert_eq!(pitch_frame_at_secs(19.990_001), 1001);
    assert_eq!(pitch_frame_at_secs(f64::MAX), i64::MAX / 960 - 8);
    for index in 0..3000 {
        let secs = index as f64 * 0.007_13;
        assert_eq!(
            pitch_frame_at_secs(secs),
            ((secs + 0.02) / 0.02).round() as i64
        );
    }
}

#[test]
fn reversed_segment_completion_preserves_pending_silent_and_computed_frames() {
    let mut track = TimelineBassTrack::new(SR * 40, SR as u32);
    let frames = track.frame_count();
    assert_eq!(frames, 2001);
    assert_eq!(track.segment_count(), 3);
    assert!(track.at_secs(25.0).is_none());
    assert!(track.merge_segment(TimelineBassSegment {
        segment_index: 1,
        estimates: vec![
            BassEstimate {
                midi: 40,
                confidence: 0.9
            };
            TIMELINE_BASS_SEGMENT_FRAMES
        ],
    }));
    assert_eq!(track.computed_frames(), 1000);
    let snapshot = track.clone();
    assert_eq!(track.at_secs(25.0).unwrap().midi, 40);
    assert_eq!(track.at_secs(19.98), None);
    assert_eq!(track.at_secs(19.990_001).unwrap().midi, 40);
    assert!(track.merge_segment(TimelineBassSegment {
        segment_index: 0,
        estimates: vec![BassEstimate::default(); TIMELINE_BASS_SEGMENT_FRAMES],
    }));
    assert_eq!(track.computed_frames(), 2000);
    assert_eq!(track.at_secs(1.0), Some(BassEstimate::default()));
    assert_eq!(snapshot.at_secs(1.0), None);
    assert!(track.merge_segment(TimelineBassSegment {
        segment_index: 2,
        estimates: vec![BassEstimate::default()],
    }));
    assert_eq!(track.computed_frames(), frames);
    assert_eq!(track.at_secs(39.999), Some(BassEstimate::default()));
    for secs in [f64::NAN, f64::INFINITY, -0.01, 40.0, 40.1] {
        assert_eq!(track.at_secs(secs), None);
    }
    assert!(!track.merge_segment(TimelineBassSegment {
        segment_index: 0,
        estimates: vec![]
    }));
    assert!(!track.merge_segment(TimelineBassSegment {
        segment_index: 3,
        estimates: vec![]
    }));
    assert!(track.storage_bytes() >= frames * std::mem::size_of::<BassEstimate>());
}

#[test]
fn invalid_partial_segment_does_not_mark_any_frames_computed() {
    let mut track = TimelineBassTrack::new(SR, SR as u32);
    assert!(!track.merge_segment(TimelineBassSegment {
        segment_index: 0,
        estimates: vec![BassEstimate::default(); 20],
    }));
    assert_eq!(track.computed_frames(), 0);
    assert_eq!(track.at_secs(0.0), None);
}

#[test]
fn silence_short_and_fractional_eof_are_computed_with_zero_padding() {
    for samples in [1, 960 + 13, 8 * 960 - 1] {
        let stereo = vec![0.0; samples * 2];
        let track = calculate(&stereo);
        assert_eq!(track.computed_frames(), track.frame_count());
        assert_eq!(track.at_secs(0.0), Some(BassEstimate::default()));
        assert_eq!(
            track.at_secs((samples as f64 - 0.01) / SR as f64),
            Some(BassEstimate::default())
        );
    }
    assert_eq!(TimelineBassTrack::new(0, SR as u32).segment_count(), 0);
    assert!(calculate_timeline_bass_segment(&[], SR as u32, 0, &|| false).is_none());
    assert_eq!(TimelineBassTrack::new(10, 0).segment_count(), 0);
    assert!(calculate_timeline_bass_segment(&[0.0; 20], 0, 0, &|| false).is_none());
    let segment = calculate_timeline_bass_segment(&[0.5; 200], 44_100, 0, &|| false).unwrap();
    assert!(
        segment
            .estimates
            .iter()
            .all(|bass| *bass == BassEstimate::default())
    );
}

#[test]
fn cancellation_discards_partial_and_final_frame_results() {
    let stereo = vec![0.0; SR * 2];
    assert!(calculate_timeline_bass_segment(&stereo, SR as u32, 0, &|| true).is_none());
    for cancel_check in [2, 11, 61] {
        let checks = Cell::new(0);
        let segment = calculate_timeline_bass_segment(&stereo, SR as u32, 0, &|| {
            checks.set(checks.get() + 1);
            checks.get() >= cancel_check
        });
        assert!(segment.is_none(), "cancel check {cancel_check}");
        assert_eq!(checks.get(), cancel_check);
    }
}

#[test]
fn synthetic_bass_chord_and_silence_recover_correct_notes() {
    let mut stereo = vec![0.0; SR * 6 * 2];
    add_tones(&mut stereo, 0.0, 1.8, &[(40, 10, 1.0)]);
    add_tones(
        &mut stereo,
        2.6,
        6.0,
        &[(43, 8, 1.0), (60, 6, 2.0), (64, 6, 2.0), (67, 6, 2.0)],
    );
    let track = calculate(&stereo);
    for secs in [0.82, 1.4] {
        assert_eq!(track.at_secs(secs).unwrap().midi, 40, "secs={secs}");
    }
    assert_eq!(track.at_secs(2.3), Some(BassEstimate::default()));
    for secs in [3.42, 5.0] {
        assert_eq!(track.at_secs(secs).unwrap().midi, 43, "secs={secs}");
    }
    assert!(track.at_secs(5.999_99).is_some());
}

#[test]
fn nonfinite_channels_match_spectrum_mono_sanitization() {
    let mut stereo = vec![0.0; SR * 2 * 2];
    add_tones(&mut stereo, 0.0, 2.0, &[(40, 10, 1.0)]);
    let mut expected = stereo.clone();
    for (index, pair) in stereo.chunks_exact_mut(2).enumerate() {
        if index % 2 == 0 {
            pair[0] = if index % 4 == 0 {
                f32::NAN
            } else {
                f32::INFINITY
            };
            expected[index * 2] = 0.0;
        } else {
            pair[1] = f32::NEG_INFINITY;
            expected[index * 2 + 1] = 0.0;
        }
    }
    let actual = calculate_timeline_bass_segment(&stereo, SR as u32, 0, &|| false).unwrap();
    let expected = calculate_timeline_bass_segment(&expected, SR as u32, 0, &|| false).unwrap();
    assert_eq!(actual.estimates, expected.estimates);
    assert!(actual.estimates.iter().any(|bass| bass.midi == 40));
}

#[test]
fn sustained_harmonic_bass_crosses_segment_boundary_and_eof() {
    let mut stereo = vec![0.0; SR * 21 * 2 + 26];
    add_tones(&mut stereo, 0.0, 21.0, &[(40, 10, 1.0)]);
    let track = calculate(&stereo);
    for secs in [19.8, 19.98, 19.99, 20.0, 20.1, 20.8, 20.999] {
        let bass = track.at_secs(secs).unwrap();
        assert_eq!(bass.midi, 40, "secs={secs}, bass={bass:?}");
        assert!(bass.confidence >= 0.5);
    }
}
