use music_core::bass::chroma_from_probabilities;
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
fn chroma_maps_every_piano_key_to_c_based_pitch_classes() {
    // MIDI 21 starts at A, while chroma starts at C.
    let piano_octave = [9, 10, 11, 0, 1, 2, 3, 4, 5, 6, 7, 8];
    for key in 0..88 {
        let mut probabilities = [0.0; 88];
        probabilities[key] = 0.25; // No thresholding: even sub-0.5 values contribute.
        let mut expected = [0.0; 12];
        expected[piano_octave[key % 12]] = 0.25;
        assert_eq!(
            chroma_from_probabilities(&probabilities),
            expected,
            "MIDI {}",
            key + 21
        );
    }
    // All octaves contribute, including the keys above the previous FFT cutoff.
    let chroma = chroma_from_probabilities(&[0.25; 88]);
    assert_eq!(
        chroma,
        [
            2.0, 1.75, 1.75, 1.75, 1.75, 1.75, 1.75, 1.75, 1.75, 2.0, 2.0, 2.0
        ]
    );
    let mut probabilities = [0.0; 88];
    probabilities[24 - 21] = 0.5;
    probabilities[36 - 21] = 0.75;
    probabilities[96 - 21] = 0.25;
    probabilities[108 - 21] = 0.5;
    probabilities[107 - 21] = 0.125;
    let chroma = chroma_from_probabilities(&probabilities);
    assert_eq!(chroma[0], 2.0);
    assert_eq!(chroma[11], 0.125);
    assert_eq!(chroma.iter().sum::<f32>(), 2.125);
}

fn merge_silent_segment(track: &mut TimelineBassTrack, segment_index: usize) {
    let len = (track.frame_count() - segment_index * TIMELINE_BASS_SEGMENT_FRAMES)
        .min(TIMELINE_BASS_SEGMENT_FRAMES);
    assert!(track.merge_segment(TimelineBassSegment {
        segment_index,
        estimates: vec![BassEstimate::default(); len],
        chroma: vec![[0.0; 12]; len],
    }));
}

#[test]
fn computed_windows_clip_to_real_pcm_and_check_silent_segment_boundaries() {
    let mut track = TimelineBassTrack::new(SR * 40, SR as u32);
    assert!(!track.window_is_computed(-3.0, 3.0));
    merge_silent_segment(&mut track, 0);
    assert!(track.window_is_computed(-3.0, 3.0));
    assert!(track.window_is_computed(0.0, 0.0));
    assert!(track.window_is_computed(0.0, 19.989_999));
    assert!(!track.window_is_computed(19.98, 20.02));
    assert!(!track.window_is_computed(17.0, 23.0));
    // The only pending data in this window is silent. Zero weight must not hide it.
    merge_silent_segment(&mut track, 2);
    assert!(!track.window_is_computed(17.0, 43.0));
    assert!(!track.window_is_computed(37.0, 43.0));
    merge_silent_segment(&mut track, 1);
    assert!(track.window_is_computed(17.0, 23.0));
    assert!(track.window_is_computed(37.0, 43.0));
    assert!(track.window_is_computed(-3.0, 43.0));
    for (start, end) in [
        (f64::NAN, 1.0),
        (0.0, f64::INFINITY),
        (f64::NEG_INFINITY, 1.0),
        (1.0, 0.0),
        (-3.0, -0.1),
        (40.0, 43.0),
    ] {
        assert!(
            !track.window_is_computed(start, end),
            "window [{start}, {end}]"
        );
    }
    assert!(!TimelineBassTrack::new(0, SR as u32).window_is_computed(-3.0, 3.0));
    // 20.0001s has an extra padding display frame in segment 1. At 19.989999s
    // segment 0 suffices; a window reaching real EOF requires segment 1.
    let mut fractional = TimelineBassTrack::new(SR * 20 + 5, SR as u32);
    merge_silent_segment(&mut fractional, 0);
    assert!(fractional.window_is_computed(-3.0, 19.989_999));
    assert!(!fractional.window_is_computed(19.98, 25.0));
    merge_silent_segment(&mut fractional, 1);
    assert!(fractional.window_is_computed(19.98, 25.0));
    let mut padding_only = TimelineBassTrack::new(SR * 20 - 955, SR as u32);
    assert_eq!(padding_only.segment_count(), 2);
    merge_silent_segment(&mut padding_only, 0);
    assert!(padding_only.window_is_computed(-3.0, 25.0));
    assert!(padding_only.computed_frames() < padding_only.frame_count());
}

#[test]
fn segment_stores_bass_and_chroma_from_the_same_head3_output() {
    use music_core::pitch::{HARMNET_HOP_SAMPLES, HARMNET_LONG_WINDOW_SAMPLES, HarmNetTracker};
    let mut stereo = vec![0.0; SR / 4 * 2];
    add_tones(&mut stereo, 0.0, 0.25, &[(40, 10, 1.0), (100, 1, 1.0)]);
    let segment = calculate_timeline_bass_segment(&stereo, SR as u32, 0, &|| false).unwrap();
    let mut tracker = HarmNetTracker::new(SR as u32);
    let mut mono = vec![0.0; HARMNET_LONG_WINDOW_SAMPLES];
    for inference_frame in 1..=segment.estimates.len() + 8 {
        let end_sample = inference_frame * HARMNET_HOP_SAMPLES;
        for (offset, sample) in mono.iter_mut().enumerate() {
            let source =
                end_sample as isize + offset as isize - HARMNET_LONG_WINDOW_SAMPLES as isize;
            *sample = if source >= 0 && (source as usize) < stereo.len() / 2 {
                (stereo[source as usize * 2] + stereo[source as usize * 2 + 1]) * 0.5
            } else {
                0.0
            };
        }
        tracker.push_frame(&mono);
        if inference_frame >= 9 {
            let probabilities = tracker.probabilities(3);
            let index = inference_frame - 9;
            assert_eq!(segment.estimates[index], select_bass(&probabilities));
            assert_eq!(
                segment.chroma[index],
                chroma_from_probabilities(&probabilities)
            );
        }
    }
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
        chroma: vec![[0.4; 12]; TIMELINE_BASS_SEGMENT_FRAMES],
    }));
    assert_eq!(track.computed_frames(), 1000);
    let snapshot = track.clone();
    assert_eq!(track.at_secs(25.0).unwrap().midi, 40);
    assert_eq!(track.chroma_at_secs(25.0), Some([0.4; 12]));
    assert_eq!(track.at_secs(19.98), None);
    assert_eq!(track.at_secs(19.990_001).unwrap().midi, 40);
    assert!(track.merge_segment(TimelineBassSegment {
        segment_index: 0,
        estimates: vec![BassEstimate::default(); TIMELINE_BASS_SEGMENT_FRAMES],
        chroma: vec![[0.0; 12]; TIMELINE_BASS_SEGMENT_FRAMES],
    }));
    assert_eq!(track.computed_frames(), 2000);
    assert_eq!(track.at_secs(1.0), Some(BassEstimate::default()));
    assert_eq!(snapshot.at_secs(1.0), None);
    assert_eq!(snapshot.chroma_at_secs(1.0), None);
    assert_eq!(track.chroma_at_secs(1.0), Some([0.0; 12]));
    assert!(track.merge_segment(TimelineBassSegment {
        segment_index: 2,
        estimates: vec![BassEstimate::default()],
        chroma: vec![[0.0; 12]],
    }));
    assert_eq!(track.computed_frames(), frames);
    assert_eq!(track.at_secs(39.999), Some(BassEstimate::default()));
    for secs in [f64::NAN, f64::INFINITY, -0.01, 40.0, 40.1] {
        assert_eq!(track.at_secs(secs), None);
        assert_eq!(track.chroma_at_secs(secs), None);
    }
    assert!(!track.merge_segment(TimelineBassSegment {
        segment_index: 0,
        estimates: vec![],
        chroma: vec![],
    }));
    assert!(!track.merge_segment(TimelineBassSegment {
        segment_index: 3,
        estimates: vec![],
        chroma: vec![],
    }));
    assert_eq!(
        track.storage_bytes(),
        frames * (std::mem::size_of::<BassEstimate>() + std::mem::size_of::<[f32; 12]>())
            + track.segment_count() * std::mem::size_of::<bool>()
    );
    assert_eq!(track.clone().storage_bytes(), track.storage_bytes());
    for index in 0..4000 {
        let secs = index as f64 * 0.01;
        assert_eq!(
            snapshot.at_secs(secs).is_some(),
            snapshot.chroma_at_secs(secs).is_some(),
            "snapshot secs={secs}"
        );
        assert_eq!(
            track.at_secs(secs).is_some(),
            track.chroma_at_secs(secs).is_some()
        );
    }
}

#[test]
fn invalid_partial_segment_does_not_mark_any_frames_computed() {
    let mut track = TimelineBassTrack::new(SR, SR as u32);
    assert!(!track.merge_segment(TimelineBassSegment {
        segment_index: 0,
        estimates: vec![BassEstimate::default(); 20],
        chroma: vec![[0.0; 12]; 20],
    }));
    assert_eq!(track.computed_frames(), 0);
    assert_eq!(track.at_secs(0.0), None);
    assert_eq!(track.chroma_at_secs(0.0), None);
    let count = track.frame_count();
    for (bass_count, chroma_count) in [(count, count - 1), (count - 1, count)] {
        assert!(!track.merge_segment(TimelineBassSegment {
            segment_index: 0,
            estimates: vec![BassEstimate::default(); bass_count],
            chroma: vec![[0.0; 12]; chroma_count],
        }));
        assert_eq!(track.computed_frames(), 0);
        assert_eq!(track.chroma_at_secs(0.0), None);
    }
}

#[test]
fn silence_short_and_fractional_eof_are_computed_with_zero_padding() {
    for samples in [1, 960 + 13, 8 * 960 - 1] {
        let stereo = vec![0.0; samples * 2];
        let track = calculate(&stereo);
        assert_eq!(track.computed_frames(), track.frame_count());
        assert_eq!(track.at_secs(0.0), Some(BassEstimate::default()));
        assert_eq!(track.chroma_at_secs(0.0), Some([0.0; 12]));
        assert_eq!(track.duration_secs(), samples as f64 / SR as f64);
        assert!(track.window_is_computed(-3.0, 3.0));
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
    assert_eq!(segment.estimates.len(), segment.chroma.len());
    assert!(segment.chroma.iter().all(|chroma| *chroma == [0.0; 12]));
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
    assert_eq!(track.chroma_at_secs(2.3), Some([0.0; 12]));
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
    assert_eq!(actual.chroma, expected.chroma);
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
