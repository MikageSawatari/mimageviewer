use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::decode::AudioData;

pub struct AudioPlayer {
    _stream: cpal::Stream,
    shared: Arc<Mutex<PlayerState>>,
}

struct PlayerState {
    audio: Arc<AudioData>,
    source_frames_per_output_frame: f64,
    next_source_frame: f64,
    playing: bool,
    playback_anchors: VecDeque<PlaybackAnchor>,
}

#[derive(Clone, Copy)]
struct PlaybackAnchor {
    wall_instant: Instant,
    source_frame: f64,
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
            next_source_frame: 0.0,
            playing: false,
            playback_anchors: VecDeque::new(),
        }));
        let callback_shared = Arc::clone(&shared);
        let output_channels = stream_config.channels.max(1) as usize;
        let stream = device
            .build_output_stream(
                &stream_config,
                move |output: &mut [f32], info| {
                    let timestamp = info.timestamp();
                    let now = Instant::now();
                    let playback_delay = timestamp
                        .playback
                        .duration_since(&timestamp.callback)
                        .unwrap_or_default();
                    let playback_wall = now.checked_add(playback_delay).unwrap_or(now);
                    fill_output_at(output, output_channels, &callback_shared, playback_wall);
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
            let now = Instant::now();
            if state.playing {
                let position = playback_position_at(&state, now);
                state.next_source_frame = position;
                state.playback_anchors.clear();
                state.playing = false;
            } else {
                if state.next_source_frame >= (state.audio.stereo_pcm.len() / 2) as f64 {
                    state.next_source_frame = 0.0;
                }
                state.playback_anchors.clear();
                state.playing = true;
            }
            return snapshot_at(&mut state, now);
        }
        PlaybackSnapshot::default()
    }

    pub fn pause(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.next_source_frame = playback_position_at(&state, Instant::now());
            state.playback_anchors.clear();
            state.playing = false;
        }
    }

    pub fn seek(&self, position_secs: f64) -> PlaybackSnapshot {
        if let Ok(mut state) = self.shared.lock() {
            let frames = state.audio.stereo_pcm.len() / 2;
            state.next_source_frame = (position_secs.max(0.0) * f64::from(state.audio.sample_rate))
                .round()
                .clamp(0.0, frames as f64);
            state.playback_anchors.clear();
            return snapshot_at(&mut state, Instant::now());
        }
        PlaybackSnapshot::default()
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.shared
            .lock()
            .map(|mut state| snapshot_at(&mut state, Instant::now()))
            .unwrap_or_default()
    }
}

fn snapshot_at(state: &mut PlayerState, now: Instant) -> PlaybackSnapshot {
    let position_frames = playback_position_at(state, now);
    if position_frames >= (state.audio.stereo_pcm.len() / 2) as f64 {
        state.playing = false;
    }
    PlaybackSnapshot {
        position_secs: position_frames / f64::from(state.audio.sample_rate.max(1)),
        playing: state.playing,
    }
}

fn playback_position_at(state: &PlayerState, now: Instant) -> f64 {
    if !state.playing {
        return state.next_source_frame;
    }
    let Some(anchor) = state
        .playback_anchors
        .iter()
        .rev()
        .find(|anchor| anchor.wall_instant <= now)
    else {
        return state
            .playback_anchors
            .front()
            .map(|anchor| anchor.source_frame)
            .unwrap_or(state.next_source_frame);
    };
    let elapsed_frames =
        now.duration_since(anchor.wall_instant).as_secs_f64() * f64::from(state.audio.sample_rate);
    (anchor.source_frame + elapsed_frames)
        .min(state.next_source_frame)
        .min((state.audio.stereo_pcm.len() / 2) as f64)
}

fn fill_output_at(
    output: &mut [f32],
    channels: usize,
    shared: &Arc<Mutex<PlayerState>>,
    playback_wall: Instant,
) {
    let Ok(mut state) = shared.lock() else {
        output.fill(0.0);
        return;
    };
    let available = state.audio.stereo_pcm.len() / 2;
    if state.playing && state.next_source_frame < available as f64 {
        let source_frame = state.next_source_frame;
        state.playback_anchors.push_back(PlaybackAnchor {
            wall_instant: playback_wall,
            source_frame,
        });
        while state.playback_anchors.len() > 4 {
            state.playback_anchors.pop_front();
        }
    }
    for output_frame in output.chunks_mut(channels) {
        let (left, right) = if state.playing && state.next_source_frame < available as f64 {
            let frame = state.next_source_frame.floor() as usize;
            let next = frame.saturating_add(1).min(available.saturating_sub(1));
            let amount = (state.next_source_frame - frame as f64) as f32;
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
            state.next_source_frame = (state.next_source_frame
                + state.source_frames_per_output_frame)
                .min(available as f64);
            (left, right)
        } else {
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
    use std::time::Duration;

    #[test]
    fn snapshot_uses_the_audio_cursor_as_time() {
        let mut state = PlayerState {
            audio: Arc::new(AudioData {
                stereo_pcm: vec![0.0; 96_000],
                sample_rate: 48_000,
                duration_secs: 1.0,
            }),
            source_frames_per_output_frame: 1.0,
            next_source_frame: 12_000.0,
            playing: true,
            playback_anchors: VecDeque::from([PlaybackAnchor {
                wall_instant: Instant::now(),
                source_frame: 12_000.0,
            }]),
        };
        let now = state.playback_anchors[0].wall_instant;
        assert_eq!(
            snapshot_at(&mut state, now),
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
            next_source_frame: 0.0,
            playing: true,
            playback_anchors: VecDeque::new(),
        }));
        let mut output = vec![0.0; 44_100 * 2];
        let playback_wall = Instant::now();
        fill_output_at(&mut output, 2, &shared, playback_wall);

        assert!(output.iter().all(|sample| *sample == 0.25));
        let mut state = shared.lock().unwrap();
        assert!(
            (snapshot_at(&mut state, playback_wall + Duration::from_secs(1)).position_secs - 1.0)
                .abs()
                < 0.000_001
        );
        assert_eq!(Arc::strong_count(&state.audio), 1);
    }

    #[test]
    fn large_callback_buffer_does_not_make_the_audio_clock_jump() {
        let shared = Arc::new(Mutex::new(PlayerState {
            audio: Arc::new(AudioData {
                stereo_pcm: vec![0.25; 48_000 * 2],
                sample_rate: 48_000,
                duration_secs: 1.0,
            }),
            source_frames_per_output_frame: 1.0,
            next_source_frame: 0.0,
            playing: true,
            playback_anchors: VecDeque::new(),
        }));
        let mut output = vec![0.0; 24_000 * 2];
        let playback_wall = Instant::now() + Duration::from_millis(40);
        fill_output_at(&mut output, 2, &shared, playback_wall);

        let mut state = shared.lock().unwrap();
        assert_eq!(
            snapshot_at(&mut state, playback_wall - Duration::from_millis(20)).position_secs,
            0.0
        );
        let quarter = snapshot_at(&mut state, playback_wall + Duration::from_millis(250));
        assert!((quarter.position_secs - 0.25).abs() < 0.000_001);
    }
}
