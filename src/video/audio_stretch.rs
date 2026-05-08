//! Pitch-preserving time stretching for video playback speed.
//!
//! This wrapper keeps the `signalsmith-stretch` API out of the pump loop and
//! centralizes the 1.0x bypass/reset policy.

use crate::video::clock::clamp_playback_speed;

const CHANNELS: usize = 2;
const BYPASS_EPSILON: f64 = 1.0e-6;

#[derive(Debug)]
pub(crate) struct StretchedAudio {
    pub samples: Vec<f32>,
    pub source_secs_per_output_sec: f64,
    pub stretcher_latency_output_secs: f64,
}

pub(crate) struct TimeStretcher {
    inner: signalsmith_stretch::Stretch,
    sample_rate: u32,
    output_frame_remainder: f64,
    was_bypassing: bool,
}

impl TimeStretcher {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            inner: signalsmith_stretch::Stretch::preset_default(CHANNELS as u32, sample_rate),
            sample_rate,
            output_frame_remainder: 0.0,
            was_bypassing: true,
        }
    }

    pub fn reset(&mut self) {
        self.inner.reset();
        self.output_frame_remainder = 0.0;
        self.was_bypassing = true;
    }

    pub fn process(
        &mut self,
        input: &[f32],
        _source_duration_secs: f64,
        speed: f64,
    ) -> StretchedAudio {
        let speed = clamp_playback_speed(speed);
        if input.is_empty() {
            return StretchedAudio {
                samples: Vec::new(),
                source_secs_per_output_sec: speed,
                stretcher_latency_output_secs: 0.0,
            };
        }

        if (speed - 1.0).abs() <= BYPASS_EPSILON {
            if !self.was_bypassing {
                self.inner.reset();
                self.output_frame_remainder = 0.0;
            }
            self.was_bypassing = true;
            return StretchedAudio {
                samples: input.to_vec(),
                source_secs_per_output_sec: 1.0,
                stretcher_latency_output_secs: 0.0,
            };
        }

        if self.was_bypassing {
            self.inner.reset();
            self.output_frame_remainder = 0.0;
            self.was_bypassing = false;
        }

        let input_frames = input.len() / CHANNELS;
        if input_frames == 0 {
            return StretchedAudio {
                samples: Vec::new(),
                source_secs_per_output_sec: speed,
                stretcher_latency_output_secs: 0.0,
            };
        }

        let exact_output_frames = input_frames as f64 / speed + self.output_frame_remainder;
        let mut output_frames = exact_output_frames.floor() as usize;
        self.output_frame_remainder = exact_output_frames - output_frames as f64;
        if output_frames == 0 {
            output_frames = 1;
        }

        let mut output = vec![0.0_f32; output_frames * CHANNELS];
        let process_t0 = std::time::Instant::now();
        self.inner.process(input, output.as_mut_slice());
        let stretch_ms = process_t0.elapsed().as_secs_f64() * 1000.0;
        let output_latency_samples = self.inner.output_latency();

        if crate::perf::is_enabled() {
            crate::perf::event(
                "audio",
                "stretch",
                None,
                0,
                &[
                    ("speed", serde_json::Value::from(speed)),
                    ("input_frames", serde_json::Value::from(input_frames as i64)),
                    (
                        "output_frames",
                        serde_json::Value::from(output_frames as i64),
                    ),
                    (
                        "output_latency_samples",
                        serde_json::Value::from(output_latency_samples as i64),
                    ),
                    ("process_ms", serde_json::Value::from(stretch_ms)),
                ],
            );
        }

        // Use the requested playback speed for per-chunk PTS rate. Deriving the
        // rate from rounded output length can turn tiny tail/seek chunks into
        // short 4x+ PTS bursts (e.g. 4 input frames at 3x -> 1 output frame).
        let source_secs_per_output_sec = speed;
        let latency_output_secs = output_latency_samples as f64 / self.sample_rate as f64;

        StretchedAudio {
            samples: output,
            source_secs_per_output_sec,
            stretcher_latency_output_secs: latency_output_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_keeps_input_length_and_source_rate() {
        let mut stretcher = TimeStretcher::new(48_000);
        let input = vec![0.0_f32; 960 * CHANNELS];
        let out = stretcher.process(&input, 0.020, 1.0);
        assert_eq!(out.samples.len(), input.len());
        assert!((out.source_secs_per_output_sec - 1.0).abs() < 1.0e-9);
    }

    #[test]
    fn double_speed_shortens_output() {
        let mut stretcher = TimeStretcher::new(48_000);
        let input = vec![0.0_f32; 960 * CHANNELS];
        let out = stretcher.process(&input, 0.020, 2.0);
        assert_eq!(out.samples.len(), 480 * CHANNELS);
        assert!((out.source_secs_per_output_sec - 2.0).abs() < 0.01);
    }

    #[test]
    fn tiny_high_speed_chunk_keeps_requested_source_rate() {
        let mut stretcher = TimeStretcher::new(48_000);
        let input = vec![0.0_f32; 4 * CHANNELS];
        let out = stretcher.process(&input, 4.0 / 48_000.0, 3.0);
        assert_eq!(out.samples.len(), CHANNELS);
        assert!((out.source_secs_per_output_sec - 3.0).abs() < 1.0e-9);
    }

    #[test]
    fn one_frame_high_speed_chunk_is_safe() {
        let mut stretcher = TimeStretcher::new(48_000);
        let input = vec![0.0_f32; CHANNELS];
        let out = stretcher.process(&input, 1.0 / 48_000.0, 3.0);
        assert_eq!(out.samples.len(), CHANNELS);
        assert!((out.source_secs_per_output_sec - 3.0).abs() < 1.0e-9);
    }
}
