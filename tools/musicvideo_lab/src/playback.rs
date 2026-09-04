use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::decode::AudioData;

pub struct AudioPlayer {
    _stream: cpal::Stream,
    shared: Arc<Mutex<PlayerState>>,
}

struct PlayerState {
    audio: Arc<AudioData>,
    source_frames_per_output_frame: f64,
    position_frames: f64,
    playing: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlaybackSnapshot {
    pub position_secs: f64,
    pub playing: bool,
}

impl AudioPlayer {
    pub fn new(audio: Arc<AudioData>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "既定の音声出力デバイスがありません".to_owned())?;
        let supported = device
            .default_output_config()
            .map_err(|error| format!("音声出力形式を取得できません: {error}"))?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!(
                "音声プレビューは f32 出力に対応したデバイスが必要です (現在: {:?})",
                supported.sample_format()
            ));
        }
        let stream_config: cpal::StreamConfig = supported.into();
        let output_rate = stream_config.sample_rate.0;
        let shared = Arc::new(Mutex::new(PlayerState {
            source_frames_per_output_frame: f64::from(audio.sample_rate)
                / f64::from(output_rate.max(1)),
            audio,
            position_frames: 0.0,
            playing: false,
        }));
        let callback_shared = Arc::clone(&shared);
        let output_channels = stream_config.channels.max(1) as usize;
        let stream = device
            .build_output_stream(
                &stream_config,
                move |output: &mut [f32], _| {
                    fill_output(output, output_channels, &callback_shared);
                },
                |error| eprintln!("musicvideo_lab の音声出力エラー: {error}"),
                None,
            )
            .map_err(|error| format!("音声出力を開始できません: {error}"))?;
        stream
            .play()
            .map_err(|error| format!("音声出力を再生状態にできません: {error}"))?;
        Ok(Self {
            _stream: stream,
            shared,
        })
    }

    pub fn toggle(&self) -> PlaybackSnapshot {
        if let Ok(mut state) = self.shared.lock() {
            if state.position_frames >= (state.audio.stereo_pcm.len() / 2) as f64 {
                state.position_frames = 0.0;
            }
            state.playing = !state.playing;
            return snapshot(&state);
        }
        PlaybackSnapshot::default()
    }

    pub fn pause(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.playing = false;
        }
    }

    pub fn seek(&self, position_secs: f64) -> PlaybackSnapshot {
        if let Ok(mut state) = self.shared.lock() {
            let frames = state.audio.stereo_pcm.len() / 2;
            state.position_frames = (position_secs.max(0.0) * f64::from(state.audio.sample_rate))
                .round()
                .clamp(0.0, frames as f64);
            return snapshot(&state);
        }
        PlaybackSnapshot::default()
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.shared
            .lock()
            .map(|state| snapshot(&state))
            .unwrap_or_default()
    }
}

fn snapshot(state: &PlayerState) -> PlaybackSnapshot {
    PlaybackSnapshot {
        position_secs: state.position_frames / f64::from(state.audio.sample_rate.max(1)),
        playing: state.playing,
    }
}

fn fill_output(output: &mut [f32], channels: usize, shared: &Arc<Mutex<PlayerState>>) {
    let Ok(mut state) = shared.lock() else {
        output.fill(0.0);
        return;
    };
    for output_frame in output.chunks_mut(channels) {
        let available = state.audio.stereo_pcm.len() / 2;
        let (left, right) = if state.playing && state.position_frames < available as f64 {
            let frame = state.position_frames.floor() as usize;
            let next = frame.saturating_add(1).min(available.saturating_sub(1));
            let amount = (state.position_frames - frame as f64) as f32;
            let left = lerp(
                state.audio.stereo_pcm[frame * 2],
                state.audio.stereo_pcm[next * 2],
                amount,
            );
            let right = lerp(
                state.audio.stereo_pcm[frame * 2 + 1],
                state.audio.stereo_pcm[next * 2 + 1],
                amount,
            );
            state.position_frames = (state.position_frames + state.source_frames_per_output_frame)
                .min(available as f64);
            (left, right)
        } else {
            if state.position_frames >= available as f64 {
                state.playing = false;
            }
            (0.0, 0.0)
        };
        for (channel, sample) in output_frame.iter_mut().enumerate() {
            *sample = match channel {
                0 => left,
                1 => right,
                _ => (left + right) * 0.5,
            };
        }
    }
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_uses_the_audio_cursor_as_time() {
        let state = PlayerState {
            audio: Arc::new(AudioData {
                stereo_pcm: vec![0.0; 96_000],
                sample_rate: 48_000,
                duration_secs: 1.0,
            }),
            source_frames_per_output_frame: 1.0,
            position_frames: 12_000.0,
            playing: true,
        };
        assert_eq!(
            snapshot(&state),
            PlaybackSnapshot {
                position_secs: 0.25,
                playing: true,
            }
        );
    }

    #[test]
    fn output_rate_conversion_advances_the_source_clock_without_a_second_buffer() {
        let shared = Arc::new(Mutex::new(PlayerState {
            audio: Arc::new(AudioData {
                stereo_pcm: vec![0.25; 48_000 * 2],
                sample_rate: 48_000,
                duration_secs: 1.0,
            }),
            source_frames_per_output_frame: 48_000.0 / 44_100.0,
            position_frames: 0.0,
            playing: true,
        }));
        let mut output = vec![0.0; 44_100 * 2];
        fill_output(&mut output, 2, &shared);

        assert!(output.iter().all(|sample| *sample == 0.25));
        let state = shared.lock().unwrap();
        assert!((snapshot(&state).position_secs - 1.0).abs() < 0.000_001);
        assert_eq!(Arc::strong_count(&state.audio), 1);
    }
}
