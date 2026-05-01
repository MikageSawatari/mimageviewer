//! cpal を使った検証用音源と出力ループ。
//!
//! 入力源:
//!  - サイン波 (周波数指定可能、A/B 聴き比べに使う)
//!  - 短い WAV を読み込んでループ再生 (将来追加、Phase 0b では sine のみ)
//!
//! 信号フローは 2 モード切替:
//!  - Bypass: 生成 → cpal 出力 (プラグイン経由なし)
//!  - Through: 生成 → bridge.push_audio() → bridge.pull_audio() → cpal 出力
//!
//! Phase 0b ではプラグイン latency 補償は実装しない (= 単純な「先入れ先出し」)。
//! プラグインのリアルタイム性確認 + パススルー音質確認が目的。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::bridge::Bridge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 直接 cpal 出力 (プラグイン経路を経由しない)
    Bypass,
    /// bridge 経由でプラグイン処理した結果を出力
    Through,
}

#[derive(Clone)]
pub struct ToneParams {
    pub freq_hz: Arc<AtomicU32>, // u32 で 1Hz 単位、UI は f32 でも保持
    pub amplitude_milli: Arc<AtomicU32>, // 0..1000
    pub muted: Arc<AtomicBool>,
}

impl Default for ToneParams {
    fn default() -> Self {
        Self {
            freq_hz: Arc::new(AtomicU32::new(440)),
            amplitude_milli: Arc::new(AtomicU32::new(150)), // 0.15
            muted: Arc::new(AtomicBool::new(true)),         // 起動直後はミュート
        }
    }
}

pub struct AudioEngine {
    // Stream は drop で停止するため保持必須 (UI で参照しないので #[allow])
    #[allow(dead_code)]
    pub stream: cpal::Stream,
    pub sample_rate: u32,
    pub block_size: u32,
    // tone / mode は callback と共有しているが、UI 側からも操作するので Arc で公開
    #[allow(dead_code)]
    pub tone: ToneParams,
    #[allow(dead_code)]
    pub mode: Arc<Mutex<Mode>>,
    /// cpal callback で実際に渡される n_frames (= 最後の値)。
    pub actual_n_frames: Arc<AtomicU32>,
    /// 1 秒間の最小 / 最大 callback frame 数。これが安定していれば cpal は
    /// 一定サイズで呼ばれているが、ばらつくとそれ自体がノイズ要因になる。
    pub min_n_frames: Arc<AtomicU32>,
    pub max_n_frames: Arc<AtomicU32>,
    /// callback 内で発生したアンダーラン回数 (= bridge から十分なサンプルを
    /// 取れなかった回数)。
    pub underruns: Arc<AtomicU32>,
    /// callback 内で部分的にしか取れなかった回数 (= bridge から got>0 だが
    /// got<期待 のケース、= 半端取得)。
    pub partial_pulls: Arc<AtomicU32>,
}

impl AudioEngine {
    /// 既定出力デバイスでステレオ f32 ストリームを開く。
    /// `bridge` が `Some` なら Through モードで bridge を経由できる。
    pub fn start(
        bridge: Option<Arc<Bridge>>,
        mode: Arc<Mutex<Mode>>,
        tone: ToneParams,
    ) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let config = device
            .default_output_config()
            .map_err(|e| format!("default_output_config: {e}"))?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as u32;
        if channels < 2 {
            return Err(format!(
                "output device has only {channels} channel(s); need stereo"
            ));
        }

        let block_size = 480u32; // bridge 側に渡す処理ブロックサイズ
        // cpal の callback サイズを bridge と一致させる。
        // 一致しないと「441 push / 480 wait」のような数サンプル単位のずれが
        // 1 秒間に積み重なって、ring buffer のアンダー/オーバーランが起きてブチブチノイズになる。
        // WASAPI Shared では Fixed が拒否される場合があるが、その場合 Default に
        // フォールバックする。
        let actual_n_frames = Arc::new(AtomicU32::new(0));
        let min_n_frames = Arc::new(AtomicU32::new(u32::MAX));
        let max_n_frames = Arc::new(AtomicU32::new(0));
        let underruns = Arc::new(AtomicU32::new(0));
        let partial_pulls = Arc::new(AtomicU32::new(0));
        let actual_n_frames_cb = Arc::clone(&actual_n_frames);
        let min_n_frames_cb = Arc::clone(&min_n_frames);
        let max_n_frames_cb = Arc::clone(&max_n_frames);
        let underruns_cb = Arc::clone(&underruns);
        let partial_pulls_cb = Arc::clone(&partial_pulls);
        let mut phase: f32 = 0.0;
        let tone_for_cb = tone.clone();
        let mode_for_cb = Arc::clone(&mode);
        let bridge_for_cb = bridge.clone();
        // bridge へ渡す in/out 一時バッファ
        let mut bridge_in_buf: Vec<f32> = Vec::new();
        let mut bridge_out_buf: Vec<f32> = Vec::new();

        let err_fn = |e| eprintln!("cpal stream error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mut stream_config: cpal::StreamConfig = config.clone().into();
                // バッファサイズ固定を試みる (失敗時は default のままで継続)
                stream_config.buffer_size = cpal::BufferSize::Fixed(block_size);
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let n_frames = data.len() / channels as usize;
                        // 毎回上書き (UI 側で polling)
                        actual_n_frames_cb.store(n_frames as u32, Ordering::Relaxed);
                        // 最小/最大トラッキング (UI 側で 1 秒ごとに読んでリセット)
                        let nu = n_frames as u32;
                        min_n_frames_cb.fetch_min(nu, Ordering::Relaxed);
                        max_n_frames_cb.fetch_max(nu, Ordering::Relaxed);
                        let amp =
                            tone_for_cb.amplitude_milli.load(Ordering::Relaxed) as f32 / 1000.0;
                        let muted = tone_for_cb.muted.load(Ordering::Relaxed);
                        let freq = tone_for_cb.freq_hz.load(Ordering::Relaxed) as f32;
                        let mode_val = *mode_for_cb.lock().unwrap();

                        // 1. tone を生成 (stereo)
                        bridge_in_buf.clear();
                        bridge_in_buf.reserve(n_frames * 2);
                        for _ in 0..n_frames {
                            let s = if muted { 0.0 } else { phase.sin() * amp };
                            bridge_in_buf.push(s);
                            bridge_in_buf.push(s);
                            phase += 2.0 * std::f32::consts::PI * freq / sample_rate as f32;
                            if phase > 2.0 * std::f32::consts::PI {
                                phase -= 2.0 * std::f32::consts::PI;
                            }
                        }

                        // 2. mode に応じて bridge 経路 or 直結
                        match (mode_val, &bridge_for_cb) {
                            (Mode::Through, Some(br)) => {
                                let _ = br.push_audio(&bridge_in_buf);
                                bridge_out_buf.resize(n_frames * 2, 0.0);
                                let got = br.pull_audio(&mut bridge_out_buf, 50).unwrap_or(0);
                                if got == bridge_in_buf.len() {
                                    // 3. cpal の channels 数に合わせて先頭 2ch だけ書き戻す
                                    for f in 0..n_frames {
                                        for c in 0..channels as usize {
                                            let src_ch = c.min(1);
                                            data[f * channels as usize + c] =
                                                bridge_out_buf[f * 2 + src_ch];
                                        }
                                    }
                                } else {
                                    // bridge から戻ってこないときは silence (= 安全側)
                                    if got == 0 {
                                        underruns_cb.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        partial_pulls_cb.fetch_add(1, Ordering::Relaxed);
                                    }
                                    for s in data.iter_mut() {
                                        *s = 0.0;
                                    }
                                }
                            }
                            _ => {
                                // Bypass: 生成した tone を直接書き出す
                                for f in 0..n_frames {
                                    for c in 0..channels as usize {
                                        let src_ch = c.min(1);
                                        data[f * channels as usize + c] =
                                            bridge_in_buf[f * 2 + src_ch];
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            other => return Err(format!("unsupported sample format: {other:?}")),
        }
        .map_err(|e| format!("build_output_stream: {e}"))?;
        stream.play().map_err(|e| format!("stream.play: {e}"))?;

        Ok(Self {
            stream,
            sample_rate,
            block_size,
            tone,
            mode,
            actual_n_frames,
            min_n_frames,
            max_n_frames,
            underruns,
            partial_pulls,
        })
    }
}
