//! Timeline bass and chroma from the same full-range HarmNet inference as the keyboard.
//! Segments own only their results; callers borrow the decoded stereo PCM.

use crate::pitch::{
    HARMNET_HOP_SAMPLES, HARMNET_HOP_SECS, HARMNET_LONG_WINDOW_SAMPLES, HarmNetTracker,
};

pub const TIMELINE_BASS_SEGMENT_FRAMES: usize = 1000;
const LOOKAHEAD_FRAMES: usize = 8;
const WARMUP_FRAMES: usize = 58;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BassEstimate {
    /// Absolute MIDI key; zero means no bass note met the threshold.
    pub midi: u8,
    pub confidence: f32,
}

/// Probabilities are ordered MIDI 21..=108, as returned by HarmNet.
/// Select the lowest key in 21..=55 meeting the inclusive 0.5 threshold.
pub fn select_bass(probabilities: &[f32]) -> BassEstimate {
    probabilities
        .iter()
        .take(35)
        .enumerate()
        .find(|(_, probability)| **probability >= 0.5)
        .map_or(BassEstimate::default(), |(index, &confidence)| {
            BassEstimate {
                midi: index as u8 + 21,
                confidence,
            }
        })
}

/// Sum all 88 fundamental probabilities (MIDI 21..=108) by MIDI pitch class.
/// C is index zero; octaves add without normalization or a bass-range cutoff.
pub fn chroma_from_probabilities(probabilities: &[f32; 88]) -> [f32; 12] {
    let mut chroma = [0.0; 12];
    for (index, &probability) in probabilities.iter().enumerate() {
        chroma[(index + 21) % 12] += probability;
    }
    chroma
}

/// Display frame k, before the eight-frame head-3 lookahead. This uses exactly
/// the spectrum worker's normalization, rounding and sample-index bound.
pub fn pitch_frame_at_secs(center_secs: f64) -> i64 {
    let center = if center_secs.is_finite() {
        center_secs.max(0.0)
    } else {
        0.0
    };
    (((center + 0.02) / HARMNET_HOP_SECS).round() as i64).clamp(
        1,
        i64::MAX / HARMNET_HOP_SAMPLES as i64 - LOOKAHEAD_FRAMES as i64,
    )
}

fn frame_count(pcm_frames: usize, sample_rate: u32) -> usize {
    if pcm_frames == 0 || sample_rate == 0 {
        return 0;
    }
    // Include the nearest frame at EOF, since the final half-hop rounds up.
    // Integer arithmetic also handles unsupported rates without an FFT plan.
    (pcm_frames as u128 * 50).div_ceil(sample_rate as u128) as usize + 1
}

#[derive(Clone, Debug)]
pub struct TimelineBassSegment {
    pub segment_index: usize,
    pub estimates: Vec<BassEstimate>,
    pub chroma: Vec<[f32; 12]>,
}

/// Immutable clones are published by the analysis owner, never by its parallel
/// segment workers. A computed silent frame remains distinct from pending work.
#[derive(Clone, Debug)]
pub struct TimelineBassTrack {
    duration_secs: f64,
    estimates: Vec<BassEstimate>,
    chroma: Vec<[f32; 12]>,
    computed: Vec<bool>,
}

impl TimelineBassTrack {
    pub fn new(pcm_frames: usize, sample_rate: u32) -> Self {
        let count = frame_count(pcm_frames, sample_rate);
        Self {
            duration_secs: if sample_rate == 0 {
                0.0
            } else {
                pcm_frames as f64 / sample_rate as f64
            },
            estimates: vec![BassEstimate::default(); count],
            chroma: vec![[0.0; 12]; count],
            computed: vec![false; count.div_ceil(TIMELINE_BASS_SEGMENT_FRAMES)],
        }
    }

    pub fn segment_count(&self) -> usize {
        self.computed.len()
    }

    pub fn frame_count(&self) -> usize {
        self.estimates.len()
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }

    /// Reject invalid or duplicate segments without changing the published state.
    pub fn merge_segment(&mut self, segment: TimelineBassSegment) -> bool {
        let Some(computed) = self.computed.get_mut(segment.segment_index) else {
            return false;
        };
        let start = segment.segment_index * TIMELINE_BASS_SEGMENT_FRAMES;
        let end = (start + TIMELINE_BASS_SEGMENT_FRAMES).min(self.estimates.len());
        if *computed
            || segment.estimates.len() != end - start
            || segment.chroma.len() != end - start
        {
            return false;
        }
        self.estimates[start..end].copy_from_slice(&segment.estimates);
        self.chroma[start..end].copy_from_slice(&segment.chroma);
        *computed = true;
        true
    }

    /// None means pending or outside [0, duration); Some(default) means computed
    /// with no qualifying bass note. Do not use a neighbouring computed segment.
    pub fn at_secs(&self, secs: f64) -> Option<BassEstimate> {
        self.estimates
            .get(self.computed_index_at_secs(secs)?)
            .copied()
    }

    /// Shares the bass frame's computed mark. Computed silence is Some([0; 12]).
    pub fn chroma_at_secs(&self, secs: f64) -> Option<[f32; 12]> {
        self.chroma.get(self.computed_index_at_secs(secs)?).copied()
    }

    fn computed_index_at_secs(&self, secs: f64) -> Option<usize> {
        if !secs.is_finite() || secs < 0.0 || secs >= self.duration_secs {
            return None;
        }
        let index = (pitch_frame_at_secs(secs) - 1) as usize;
        if !self
            .computed
            .get(index / TIMELINE_BASS_SEGMENT_FRAMES)
            .copied()
            .unwrap_or(false)
        {
            return None;
        }
        (index < self.estimates.len()).then_some(index)
    }

    /// Check every display frame touched by the inclusive time window, clipped
    /// to the PCM's actual [0, duration) range. Silence still requires computation.
    /// Invalid windows and windows containing no real PCM time return false.
    /// Lookahead/padding-only frames beyond the last usable display frame do not
    /// participate; nearest-frame rounding matches at_secs/chroma_at_secs.
    pub fn window_is_computed(&self, start_secs: f64, end_secs: f64) -> bool {
        if !start_secs.is_finite() || !end_secs.is_finite() || start_secs > end_secs {
            return false;
        }
        let start = start_secs.max(0.0);
        let end = end_secs.min(self.duration_secs.next_down());
        if start > end {
            return false;
        }
        let first = (pitch_frame_at_secs(start) - 1) as usize;
        let last = (pitch_frame_at_secs(end) - 1) as usize;
        if last >= self.estimates.len() {
            return false;
        }
        self.computed[first / TIMELINE_BASS_SEGMENT_FRAMES..=last / TIMELINE_BASS_SEGMENT_FRAMES]
            .iter()
            .all(|&computed| computed)
    }

    pub fn computed_frames(&self) -> usize {
        self.computed
            .iter()
            .enumerate()
            .filter(|(_, computed)| **computed)
            .map(|(index, _)| {
                (self.estimates.len() - index * TIMELINE_BASS_SEGMENT_FRAMES)
                    .min(TIMELINE_BASS_SEGMENT_FRAMES)
            })
            .sum()
    }

    /// Retained allocation bytes, excluding the small track struct itself.
    pub fn storage_bytes(&self) -> usize {
        self.estimates.capacity() * std::mem::size_of::<BassEstimate>()
            + self.chroma.capacity() * std::mem::size_of::<[f32; 12]>()
            + self.computed.capacity() * std::mem::size_of::<bool>()
    }
}

/// Calculate one segment after decode success. A fresh tracker receives the
/// absolute warmup range; each target k uses inference k+8, head 3. Both PCM
/// ends are padded with zeros, including lookahead beyond a completed EOF.
pub fn calculate_timeline_bass_segment(
    stereo: &[f32],
    sample_rate: u32,
    segment_index: usize,
    cancel: &impl Fn() -> bool,
) -> Option<TimelineBassSegment> {
    let count = frame_count(stereo.len() / 2, sample_rate);
    let first_index = segment_index.checked_mul(TIMELINE_BASS_SEGMENT_FRAMES)?;
    if first_index >= count || cancel() {
        return None;
    }
    let end_index = first_index
        .saturating_add(TIMELINE_BASS_SEGMENT_FRAMES)
        .min(count);
    let mut estimates = Vec::with_capacity(end_index - first_index);
    let mut chroma = Vec::with_capacity(end_index - first_index);
    if sample_rate != 48_000 {
        // Match HarmNetTracker's valid all-zero unsupported-rate contract.
        for _ in first_index..end_index {
            if cancel() {
                return None;
            }
            estimates.push(BassEstimate::default());
            chroma.push([0.0; 12]);
        }
        return Some(TimelineBassSegment {
            segment_index,
            estimates,
            chroma,
        });
    }
    let first_k = first_index + 1;
    let from = (first_k + LOOKAHEAD_FRAMES)
        .saturating_sub(WARMUP_FRAMES)
        .max(1);
    let mut tracker = HarmNetTracker::new(sample_rate);
    let mut mono = vec![0.0; HARMNET_LONG_WINDOW_SAMPLES];
    for inference_frame in from..=end_index + LOOKAHEAD_FRAMES {
        if cancel() {
            return None;
        }
        let end_sample = inference_frame.checked_mul(HARMNET_HOP_SAMPLES)?;
        for (offset, sample) in mono.iter_mut().enumerate() {
            *sample = end_sample
                .checked_add(offset)
                .and_then(|end| end.checked_sub(HARMNET_LONG_WINDOW_SAMPLES))
                .and_then(|index| index.checked_mul(2))
                .and_then(|index| stereo.get(index..index + 2))
                .map_or(0.0, |pair| {
                    let left = if pair[0].is_finite() { pair[0] } else { 0.0 };
                    let right = if pair[1].is_finite() { pair[1] } else { 0.0 };
                    0.5 * left + 0.5 * right
                });
        }
        tracker.push_frame(&mono);
        if inference_frame >= first_k + LOOKAHEAD_FRAMES {
            let probabilities = tracker.probabilities(3);
            estimates.push(select_bass(&probabilities));
            chroma.push(chroma_from_probabilities(&probabilities));
        }
    }
    // No partial segment escapes cancellation, including the last inference.
    if cancel() {
        return None;
    }
    Some(TimelineBassSegment {
        segment_index,
        estimates,
        chroma,
    })
}
