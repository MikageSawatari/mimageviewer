//! 動画再生用の音声出力 (cpal / WASAPI Shared)。
//!
//! [`super::decoder`] が送ってくる [`super::decoder::AudioFrame`] (interleaved stereo f32)
//! を受け、`cpal` の出力ストリームに流す。AV 同期のため、コールバックで「直近に
//! 出力したサンプルの PTS」を [`super::clock::AvClock::set_audio_pts`] で報告する。
//!
//! ## アーキテクチャ
//! - 別スレッド (`audio-pump`) が `audio_rx` から AudioFrame を取り出し、共有
//!   ring buffer に push する (バッファ目安 ~100ms 分)。
//! - `cpal` の出力ストリーム (= `cpal` 内部の RT スレッド) が ring buffer から pop して
//!   出力バッファに書き込む。バッファ枯渇時は無音で埋める。
//!
//! ## 注意
//! `cpal::Stream` は !Send なので、cpal の RT スレッドに「閉じ込めて」管理する必要がある。
//! [`AudioOutput`] が drop されたら自動で停止する。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, bounded};

use super::clock::{AvClock, SEEK_TARGET_TOLERANCE_SECS};
use super::decoder::AudioFrame;
use super::engine::actor::state_code;

/// 音声出力ストリーム。drop すると `pause` + Stream drop + pump スレッド join を
/// 順序通りに行い、別動画への切替時に前動画の音声が残らないようにする。
pub struct AudioOutput {
    /// cpal Stream は !Send。Option にして drop 時に明示的に落とす。
    stream: Option<cpal::Stream>,
    /// pump thread の停止フラグ。
    cancel: Arc<AtomicBool>,
    /// pump スレッド起床用 (recv_timeout より速く抜けるため)。
    shutdown_tx: Sender<()>,
    /// pump スレッドハンドル。drop で join する。
    pump: Option<std::thread::JoinHandle<()>>,
    pub sample_rate: u32,
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        // 1. pump 停止指示
        self.cancel.store(true, Ordering::Release);
        let _ = self.shutdown_tx.try_send(());
        // 2. Stream を pause して直ちに新規 callback を停止 → drop で完全終了
        if let Some(stream) = self.stream.take() {
            use cpal::traits::StreamTrait;
            let _ = stream.pause();
            drop(stream);
        }
        // 3. pump を join (cancel + shutdown signal で 100ms 以内に終了)
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
    }
}

/// **post-VST 処理済 chunk** (raw/processed 分離設計、Codex 助言 2026-05)。
///
/// pump が raw_pending から取り出した AudioFrame を VST process_block して作る。
/// fill_output は processed queue の先頭 chunk から順に drain する。
///
/// chunk 単位で metadata を持つことで:
/// - PDC 変化時に「どの latency で処理した output か」を追跡できる
/// - seek 後の `audible_pts < target` を chunk 単位で trim できる (= pre-seek 漏れ防止)
/// - BufferReady 判定が**post-VST の audible 秒数のみ**で計算できる (= raw を含めない)
struct ProcessedChunk {
    /// post-VST interleaved stereo f32 サンプル。
    samples: Vec<f32>,
    /// **audible PTS** = この chunk の最初のサンプルが「実際にスピーカーから聞こえる」
    /// PTS (秒)。`input_pts - pdc_latency_at_process` で計算。video clock 同期に使う。
    /// 現状の fill_output は `buf.next_pts_secs` ベースで pts 進行を計算するため未参照だが、
    /// 将来の PDC chunk-aware drain 実装で使用予定 (= 同期精度向上)。
    #[allow(dead_code)]
    audible_pts_secs: f64,
    /// chunk の音声時間 (秒) = `samples.len() / samples_per_sec`。
    /// BufferReady 判定や processed cap 比較で再計算を避けるためキャッシュ。
    duration_secs: f64,
    /// この chunk がどの seek 世代で生成されたか (= stale 判定用)。
    seek_serial: u64,
    /// VST process した時点の合計 PDC latency。後続 chunk と差があれば video clock
    /// jump で吸収 (旧 `pdc_latency_secs_applied` 比較ロジックを chunk 単位に分離)。
    /// 現状は fill_output が `buf.pdc_latency_secs` (= 最新 pump 更新値) を使うため
    /// 未参照だが、将来の chunk-aware PDC jump 判定で使用予定。
    #[allow(dead_code)]
    pdc_latency_secs_at_process: f64,
}

/// 共有 ring buffer (Mutex 保護)。**raw / processed 2 段構造** (Codex 助言 2026-05)。
///
/// - `raw_pending`: pump が積む pre-VST AudioFrame queue (cap = 30 秒)。Buffering 中も
///   pump が back-pressure せずに処理を続けられるようにするためのバッファ。
/// - `processed`: post-VST chunk queue (cap = 0.10 秒 audible 相当)。fill_output が drain。
///   ここの長さが **EQ latency = ユーザーの設定変更が音に届くまでの時間**。
///
/// pump は「processed が target 未満の間だけ」raw → VST process → processed を実行する。
/// EQ 設定変更後の VST process_block には新しい係数が適用され、processed queue 末尾に
/// 並んで 0.10 秒以内にスピーカーへ届く。
struct AudioBuffer {
    /// post-VST 処理済 chunk queue。fill_output が `drain_offset_in_first` を進めて
    /// drain。chunk が空になったら pop_front + drain_offset_in_first=0 にリセット。
    processed: std::collections::VecDeque<ProcessedChunk>,
    /// `processed.front()` 内の **次に drain されるサンプルの index** (= interleaved stereo)。
    /// fill_output が advance、chunk fully drain で 0 にリセット + pop_front。
    drain_offset_in_first: usize,
    /// pre-VST raw AudioFrame queue。pump が積む。VST process は `pump_drain_raw_to_processed`
    /// で「processed が cap 未満の間だけ」実行される。
    /// cap = `RAW_OVERFLOW_SECS` 相当 (= 30 秒)。超過は overflow 回復で再 anchor する。
    raw_pending: std::collections::VecDeque<AudioFrame>,
    /// processed の **次に出力されるサンプルの input PTS (秒)**。
    /// 通常は `processed.front().audible_pts_secs + drain_offset/samples_per_sec` だが、
    /// 旧コードとの互換のため `next_pts_secs` を維持する (= clock.set_audio_pts に渡す値)。
    /// processed が空 (= underrun) のときは「次に届く samples の予測 pts」として保持。
    next_pts_secs: f64,
    /// 出力サンプルレート (Hz)。
    sample_rate: u32,
    /// 1 秒あたりのサンプル数 = sample_rate * 2 (stereo)。
    samples_per_sec: f64,
    /// 最後に push したフレームの seek_serial。AvClock の現行 serial と異なれば
    /// pump thread 側で破棄する。
    pump_seek_serial: u64,
    /// PDC (Plugin Delay Compensation) 用: 直近 push 時点でのプラグインチェーン
    /// 全体の構造的遅延 (秒)。`fill_output` がこれを `clock.set_audio_pts` で減算し、
    /// video clock を audio より遅らせて A/V sync を保つ。
    ///
    /// **意味**: VST プラグインに `latency_samples = N` の lookahead がある場合、
    /// プラグインの output 1 sample は input N サンプル前に対応する。pump は
    /// 「output sample の input PTS = input frame の PTS」として buffer に push して
    /// いるが、実際にスピーカーから出る音は input PTS - N/sr の時刻に対応するため、
    /// video clock もそこに合わせる必要がある。
    ///
    /// **更新タイミング**: pump push 時に `bridge.total_latency_samples()` から計算。
    /// バッファ内のサンプルは厳密には push 時点ごとに異なる latency 値で処理されている
    /// 可能性があるが、(a) 通常は latency は固定 / 緩慢に変化、(b) 変化時は
    /// 後続 frame で値が更新されるので緩やかに追従、という設計で許容する。
    pdc_latency_secs: f64,
    /// PDC latency 変化を `fill_output` 側で検出するためのトラッキング値。
    /// `pdc_latency_secs` (pump 更新) と `pdc_latency_secs_applied` (fill_output 適用済)
    /// を比較して、不一致なら latency 変化が起きた → video clock を jump 再設定する
    /// (= 通常の monotonic guard を一回バイパス)。
    ///
    /// これによりプラグインモード切替で latency が ±100ms..±数秒 変化しても、
    /// 動画凍結 / 長時間の wall-rate キャッチアップ無しで瞬時に映像位置が補正される
    /// (= ユーザー要望: 「映像ジャンプの方が好ましい」)。
    pdc_latency_secs_applied: f64,
}

const SAFETY_LIMITER_LOOKAHEAD_SECS: f64 = 0.005;
const SAFETY_LIMITER_RELEASE_SECS: f64 = 0.100;
const SAFETY_LIMITER_CEILING_DBFS: f32 = -1.0;

/// VST3 チェーン後段の保険用 lookahead limiter。
///
/// ユーザーがチェーン末尾に limiter を入れていない場合でも、過大出力が WASAPI /
/// OS mixer 側で hard clip するのを避けるための最終安全網。制作向け limiter ではなく
/// 視聴用の保護なので、パラメータは固定し、VST3 チェーンが active のときだけ動かす。
struct SafetyLimiter {
    channels: usize,
    lookahead_frames: usize,
    delay: Vec<f32>,
    delayed_frame: Vec<f32>,
    write_frame: usize,
    gain: f32,
    ceiling: f32,
    release_coeff: f32,
    sample_rate: u32,
}

impl SafetyLimiter {
    fn new(sample_rate: u32, channels: usize) -> Self {
        let lookahead_frames =
            ((sample_rate as f64 * SAFETY_LIMITER_LOOKAHEAD_SECS).round() as usize).max(1);
        let release_coeff =
            (-1.0_f32 / (SAFETY_LIMITER_RELEASE_SECS as f32 * sample_rate as f32)).exp();
        let ceiling = 10.0_f32.powf(SAFETY_LIMITER_CEILING_DBFS / 20.0);
        Self {
            channels,
            lookahead_frames,
            delay: vec![0.0; lookahead_frames * channels],
            delayed_frame: vec![0.0; channels],
            write_frame: 0,
            gain: 1.0,
            ceiling,
            release_coeff,
            sample_rate,
        }
    }

    fn latency_secs(&self) -> f64 {
        self.lookahead_frames as f64 / self.sample_rate as f64
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.write_frame = 0;
        self.gain = 1.0;
    }

    fn process_block(&mut self, samples: &mut [f32]) {
        if samples.is_empty() || self.channels == 0 {
            return;
        }
        debug_assert_eq!(samples.len() % self.channels, 0);

        let frames = samples.len() / self.channels;
        for frame in 0..frames {
            let in_base = frame * self.channels;
            let delay_base = self.write_frame * self.channels;

            let mut peak = 0.0_f32;
            for (ch, delayed_sample) in self.delayed_frame.iter_mut().enumerate() {
                let d = self.delay[delay_base + ch];
                *delayed_sample = d;
                peak = peak.max(d.abs());
                self.delay[delay_base + ch] = samples[in_base + ch];
            }
            for &v in &self.delay {
                peak = peak.max(v.abs());
            }

            let target_gain = if peak > self.ceiling {
                (self.ceiling / peak).min(1.0)
            } else {
                1.0
            };
            if target_gain < self.gain {
                self.gain = target_gain;
            } else {
                self.gain = target_gain + (self.gain - target_gain) * self.release_coeff;
            }

            for (ch, &d) in self.delayed_frame.iter().enumerate() {
                samples[in_base + ch] = (d * self.gain).clamp(-self.ceiling, self.ceiling);
            }
            self.write_frame = (self.write_frame + 1) % self.lookahead_frames;
        }
    }
}

/// 既定音声出力デバイスのサンプルレートを取得する (実際にはストリームは開かない)。
///
/// デコーダー (swresample) と音声出力 (cpal) で **同じサンプルレート** を使わないと、
/// デバイス側で N Hz 期待のところに別の M Hz のサンプルが届くと
/// 「ピッチが下がってスロー再生」 (M < N の場合) になる。
/// 動画再生開始前に本関数で取得して decoder::spawn に渡し、両者を揃える。
pub fn default_output_sample_rate() -> Option<u32> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let cfg = device.default_output_config().ok()?;
    Some(cfg.sample_rate().0)
}

/// 音声出力ストリームを開く。デフォルトデバイスを使う。
///
/// `audio_rx` がドロップされると pump スレッドは終了するが、cpal Stream は無音で
/// 鳴り続ける (UI から AudioOutput を drop すれば停止)。
///
/// `engine_event_tx` (Phase 3d / 8.K): 音声バッファが READY_THRESHOLD に到達して
/// いる間、audio frame ごとに `AudioEvent::BufferReady` を level event として
/// emit する。EngineActor が Buffering → Playing に遷移するためのトリガ。
/// 旧 Phase 3d の「1 度だけ emit」では Loading 中に届いた event が latch reset
/// で消える race があったため、Phase 8.K で level 化した。
/// 音声出力ストリームを起動する。`dsp_bridge` を渡すと audio-pump で VST3 プラグイン
/// 処理 (チェーン) を挿入する。`is_enabled()=true` かつアクティブスロット
/// (= bypass=false の Loaded スロット) が 1 個以上のときのみ実行され、それ以外はパススルー。
pub fn start(
    audio_rx: Receiver<AudioFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<crate::video::engine::EngineEvent>,
    engine_state: Arc<AtomicU8>,
    #[cfg(windows)] dsp_bridge: Option<std::sync::Arc<crate::video::dsp::DspBridge>>,
) -> Result<AudioOutput, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "音声出力デバイスが見つかりません".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("default_output_config: {e}"))?;
    let sample_rate = supported.sample_rate().0;

    // Stereo packed f32 で固定 (decoder 側 swresample に合わせる)。
    let channels: u16 = 2;
    let config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let buffer = Arc::new(Mutex::new(AudioBuffer {
        processed: std::collections::VecDeque::with_capacity(32),
        drain_offset_in_first: 0,
        raw_pending: std::collections::VecDeque::with_capacity(64),
        next_pts_secs: 0.0,
        sample_rate,
        samples_per_sec: sample_rate as f64 * 2.0,
        pump_seek_serial: 0,
        pdc_latency_secs: 0.0,
        pdc_latency_secs_applied: 0.0,
    }));

    let cancel = Arc::new(AtomicBool::new(false));
    let (shutdown_tx, shutdown_rx) = bounded::<()>(1);

    // ── pump thread: audio_rx → raw_pending → (VST process) → processed → buffer ──
    let pump_buffer = buffer.clone();
    let pump_cancel = cancel.clone();
    let pump_clock = clock.clone();
    let pump_engine_event_tx = engine_event_tx.clone();
    let pump_engine_state = engine_state.clone();
    #[cfg(windows)]
    let pump_dsp_bridge = dsp_bridge;
    let pump_handle = std::thread::Builder::new()
        .name("audio-pump".into())
        .spawn(move || {
            run_pump(
                audio_rx,
                shutdown_rx,
                pump_buffer,
                pump_cancel,
                pump_clock,
                pump_engine_event_tx,
                pump_engine_state,
                #[cfg(windows)]
                pump_dsp_bridge,
            );
        })
        .map_err(|e| format!("spawn audio-pump: {e}"))?;

    // ── cpal output callback: buffer → device ──
    let cb_buffer = buffer.clone();
    let cb_clock = clock.clone();
    let cb_engine_state = engine_state.clone();
    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                fill_output(out, &cb_buffer, &cb_clock, &cb_engine_state);
            },
            |err| crate::logger::log(format!("cpal output stream error: {err}")),
            None,
        )
        .map_err(|e| format!("build_output_stream: {e}"))?;

    stream.play().map_err(|e| format!("stream.play: {e}"))?;

    Ok(AudioOutput {
        stream: Some(stream),
        cancel,
        shutdown_tx,
        pump: Some(pump_handle),
        sample_rate,
    })
}

/// `AudioBuffer` の各 queue 残量を秒に変換して clock に publish する。
/// pump push / fill_output pop の両方から呼ばれる。
///
/// **3 つの metric を分離して publish** (Codex 助言、2026-05-01 改訂):
/// - `audio_pump_buf_secs` = post-VST `processed` queue 残量 (= EQ latency 指標、
///   かつ **唯一 cpal が今すぐ鳴らせる "cpal-ready playable"**)
/// - `audio_raw_pending_secs` = pre-VST `raw_pending` queue 残量 (= pump 内 raw supply、
///   VST 詰まり / PDC trim drop で playable にならない可能性あり)
/// - `audio_tx_queued_secs` (= 別経路で publish 済、ここでは触らない、decoder→pump 間の
///   bounded supply、cap=audio_tx 32 frames ≒ 0.7 秒)
///
/// `total_audio_buffer_secs()` は **pacing_audio_secs** (= `processed` + `tx_queued` の和)
/// を返す。これは「cpal-ready playable + decoder→pump 間の bounded 予測補助」であり、
/// **厳密な playable ではない** (= tx_queued は pre-VST/pre-pump、cpal が今すぐ鳴らせる
/// audio ではない)。decoder pacing は本値で `in_audio_escape` を判定するが、tx_queued
/// は cap=0.7 秒に縛られるので暴走 supply 誤認のリスクは小さい。
///
/// **raw_pending は含めない** (= Codex 改訂、2026-05-01): VST が遅い/詰まる/PDC trim
/// で drop される場合に raw が満杯でも cpal は silence になりうるため。raw を含めると
/// decoder pacing が `in_audio_escape` を解除できず video が burst → audio underrun
/// したまま映像だけ進む退行。raw_pending は [`AvClock::audio_supply_secs`] で別途参照可。
///
/// **PDC latency は pump_buf に含めない** (= Codex 助言、2026-05-01):
/// 旧版は `secs + pdc_latency_secs` を publish していたが、それだと AudioBuffer が
/// 完全に空 (= cpal underrun 中) でも「PDC 分のバッファあり」に見えてしまい、
/// decoder pacing の `audio_escape` / emergency 補充が発動せず、結果として高 latency 時に
/// 音声がブツブツ途切れる退行を起こす。PDC latency は別 metric で publish する
/// (`set_vst3_pdc_latency_secs`)。先読み許可量だけ `PACE_LEAD + pdc_latency` を加算する設計。
fn publish_buffer_secs(buf: &AudioBuffer, clock: &AvClock) {
    let processed_secs: f64 = buf.processed.iter().map(|c| c.duration_secs).sum::<f64>()
        + remaining_first_chunk_secs(buf);
    let raw_pending_secs: f64 = buf.raw_pending.iter().map(|f| f.duration_secs).sum::<f64>();
    clock.set_audio_pump_buf_secs(processed_secs.max(0.0));
    clock.set_audio_raw_pending_secs(raw_pending_secs.max(0.0));
    clock.set_vst3_pdc_latency_secs(buf.pdc_latency_secs);
}

/// audio-pump スレッドの Windows 優先度を `THREAD_PRIORITY_ABOVE_NORMAL` に上げる。
///
/// `audio-pump` は decoder → (VST3 bridge IPC) → cpal ring buffer の橋渡しを担うので、
/// ここが UI 描画スレッド (= 通常優先度) より遅延すると下流の cpal callback が
/// underrun (= プチプチノイズ) を起こす。`THREAD_PRIORITY_TIME_CRITICAL` まで
/// 上げると Windows のスケジューラが他タスクを大きく待たせるので AboveNormal 止まり
/// で十分。VST3 bridge プロセス側の audio thread はさらに TIME_CRITICAL + Pro Audio
/// MMCSS で動いているので、このスレッドが少し遅れても実害は小さい。
#[cfg(windows)]
fn boost_audio_pump_priority() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    unsafe {
        let h = GetCurrentThread();
        if SetThreadPriority(h, THREAD_PRIORITY_ABOVE_NORMAL).is_err() {
            crate::logger::log("audio-pump: SetThreadPriority(AboveNormal) failed");
        }
    }
}

/// raw / processed 2 段構造の audio pump (Codex 助言、2026-05)。
///
/// **動作**:
/// 1. audio_rx から `AudioFrame` を受信 (timeout 付き、自律 refill のため)
/// 2. seek serial 切替時は VST plugin sync reset (= 既存挙動)
/// 3. raw_pending に push (= cap=30秒 を超えたら overflow)
/// 4. processed が cap (= 0.10秒) 未満の間、raw → VST process → processed をループ
/// 5. processed の post-VST audible 秒数で BufferReady emit (= raw を**含めない**)
///
/// **mutex ポリシー** (Codex P2-B): VST IPC (= bridge.process_block) 中は
/// AudioBuffer mutex を解放する。lock → pop raw → unlock → process → lock → push processed。
///
/// **自律 refill** (= 2026-05-01 Codex 助言、`processed` starvation 対策):
/// pump の outer loop は `recv_timeout(REFILL_TICK_MS)` で起き、`audio_rx` が空でも
/// `raw_pending → processed` の補充ループを毎 tick 実行する。これにより cpal が
/// processed を drain しても、`audio_rx` の到着を待たずに pump 側が processed を
/// TARGET に保てる。旧設計では outer loop が `recv` で blocking していたため、
/// 1x 再生時に「audio_rx 到着 = ~23ms 間隔」と「cpal drain = ~10ms 間隔」のずれで
/// processed が cap 周期で枯渇 → cpal silence の繰返しになっていた。
fn run_pump(
    rx: Receiver<AudioFrame>,
    shutdown_rx: crossbeam_channel::Receiver<()>,
    buffer: Arc<Mutex<AudioBuffer>>,
    cancel: Arc<AtomicBool>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<crate::video::engine::EngineEvent>,
    engine_state: Arc<AtomicU8>,
    #[cfg(windows)] dsp_bridge: Option<std::sync::Arc<crate::video::dsp::DspBridge>>,
) {
    let _ = engine_state; // 現状は logging 等で使う想定、API 互換のため受け取る
    #[cfg(windows)]
    boost_audio_pump_priority();

    // VST3 process_block 用の出力バッファ。再利用して realloc を抑える。
    #[cfg(windows)]
    let mut fx_out: Vec<f32> = Vec::with_capacity(4096);

    // ── processed queue cap (= EQ latency target) ──
    // EQ 設定変更が音に届くまでの最大時間 = processed 秒数。
    // VST3 IPC 実測 1-2ms + AboveNormal pump で 100ms を試験する。
    const TARGET_PROCESSED_SECS: f64 = 0.10;
    // ── BufferReady 閾値 ──
    // Buffering → Playing の遷移トリガ (level event)。
    const READY_THRESHOLD_SECS: f64 = 0.10;
    // ── raw_pending overflow / warning ──
    // pump back-pressure を完全に避けるため raw 側は大きく持つが、安全網として上限。
    // 30 秒で overflow (= soft fallback)、15 秒で warning ログ。
    //
    // **値の根拠** (= 2026-05-01 ユーザー報告 H264 HW decode の overflow 多発で改訂):
    // engine_state gate active 中 (= Buffering) は fill_output が非 drain なので
    // pump は raw_pending に積み続ける。audio decoder の生成速度は ~23x real-time なので、
    // Buffering 1 秒あたり 23 秒分の raw が積まれる。AV1 long GOP の 5 秒級 Buffering でも
    // 余裕で収まるよう 30 秒に拡張。memory cost = 30 * 96000 * 4 byte = ~11 MB (許容)。
    // 旧 10 秒では H264 HW decode の 1-2 秒 Buffering で頻繁に overflow していた。
    const RAW_WARNING_SECS: f64 = 15.0;
    const RAW_OVERFLOW_SECS: f64 = 30.0;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum RawReanchorReason {
        Overflow,
    }

    impl RawReanchorReason {
        fn label(self) -> &'static str {
            match self {
                Self::Overflow => "overflow",
            }
        }
    }
    // ── 自律 refill tick (Codex 助言、2026-05-01) ──
    // `recv_timeout` の値。audio_rx が空でも pump をこの間隔で起こし、processed を
    // raw_pending から自律補充する。cpal の典型的な callback 間隔 (10ms) より短く取り、
    // processed underrun を防ぐ。**短すぎ**ると CPU 負荷増、**長すぎ**ると starvation 復活。
    // 2ms 設定: processed cap を 100ms へ縮めた分、低水位追従を速くする。
    const REFILL_TICK_MS: u64 = 2;

    let sample_rate = buffer.lock().unwrap().sample_rate;
    let samples_per_sec = (sample_rate as f64) * 2.0;
    let _ = (samples_per_sec * TARGET_PROCESSED_SECS) as usize; // future use: explicit cap_samples cache
    let mut safety_limiter = SafetyLimiter::new(sample_rate, 2);

    let mut activated = false;
    let mut last_seen_seek_serial: u64 = 0;
    // The VST bridge persists across videos, while each new VideoPlayer starts
    // its seek serial at 0. Reset plugins on the first valid frame too, not only
    // when the serial increases, so previous-video delay/ring tails cannot leak.
    let mut seen_valid_audio_frame = false;
    // このシーク世代の seek_target (= 最初の post-seek フレームの input PTS)。
    // PDC 適用後の chunk が `audible_pts < seek_target` だと「pre-target silence」と
    // 判定して push せずに drop する (= Codex P1-1: 早すぎる BufferReady 防止)。
    // 新 seek 世代の最初のフレームで Some(pts_secs) に set。
    // 「最初の post-target chunk が processed に届いた」時点で None にすると、その後の
    // chunk は無条件 push でいい。
    //
    // **Fast モードの注意**: ここでの seek_target は **PDC pre-target silence 判定用**
    // であり、frame.pts_secs (= Fast では keyframe_pts) を使うのが正しい。
    // BufferReady audio_anchor 用の **user-requested target** とは別管理
    // (= `pump_anchor_target_secs` 参照)。
    let mut seek_target_secs: Option<f64> = None;
    // **Codex P1 (2026-05-01)**: BufferReady の audio_anchor pts として報告する
    // **user-requested seek target** (Fast モードでも timeline に表示したい値)。
    // demux Flush 経由で全 AudioFrame に焼き付けられている `frame.seek_target_secs`
    // を毎 intake で取り出してここに保存する。
    //
    // 用途: BufferReady emit 時に `audible_pts.max(pump_anchor_target_secs)` で
    // 報告する。Precise モードでは audible_pts ≈ target なので max は no-op、
    // Fast モードでは audible_pts < target なので target 側が採用され、Playing 入場
    // 時の clock anchor が target に維持される (= timeline 表示が target 固定)。
    //
    // 旧版 (Codex P1 修正前) は `audible_pts` のみで BufferReady を出していたため、
    // Fast で Buffering→Playing 入場時に anchor が keyframe_pts に巻き戻り、
    // notify_seek_completed(target) で立てた anchor が上書きされていた。
    let mut pump_anchor_target_secs: Option<f64> = None;
    let mut last_warning_at: Option<std::time::Instant> = None;

    while !cancel.load(Ordering::Acquire) {
        // ── frame 受信 (timeout 付き、Codex 助言): audio_rx 到着を待たず自律 refill ──
        let frame_opt: Option<AudioFrame> = crossbeam_channel::select! {
            recv(shutdown_rx) -> _ => return,
            recv(rx) -> msg => match msg {
                Ok(f) => Some(f),
                Err(_) => break,
            },
            default(std::time::Duration::from_millis(REFILL_TICK_MS)) => None,
        };

        // ── frame intake (Some の場合のみ): seek serial 検出、raw_pending push ──
        // 'intake ラベル経由で skip するときも下流の refill / publish は実行する
        // (= Codex 助言の自律 refill)。
        'intake: {
            let Some(frame) = frame_opt else {
                break 'intake;
            };

            clock.add_audio_tx_queued_secs(-frame.duration_secs);

            // ── 新 seek 世代の検出 → VST plugin sync reset (= 既存挙動) ──
            let frame_seek_serial = frame.seek_serial;
            let cur_clock_serial = clock.current_seek_serial();
            let should_reset_plugins = frame_seek_serial >= cur_clock_serial
                && (!seen_valid_audio_frame || frame_seek_serial > last_seen_seek_serial);
            if should_reset_plugins {
                #[cfg(windows)]
                if let Some(b) = &dsp_bridge {
                    if b.is_enabled() && b.active_slot_count() > 0 {
                        b.reset_plugins_sync();
                    }
                }
                last_seen_seek_serial = frame_seek_serial;
                seen_valid_audio_frame = true;
                // 新 seek 世代: target / activate を reset
                safety_limiter.reset();
                // PDC trim 用 seek_target は frame.pts_secs (= Fast では keyframe_pts、
                // Precise では target ぴったり)。詳細は seek_target_secs の宣言コメント参照。
                seek_target_secs = Some(frame.pts_secs);
                // BufferReady anchor 用 target は demux Flush 経由で焼き付けられた
                // user-requested target (= Fast でも target を維持)。
                // None フォールバック: 失敗 seek / 初期 open など、target が無い場合は
                // BufferReady で audible_pts をそのまま使う。
                pump_anchor_target_secs = frame.seek_target_secs;
                activated = false; // 再度 audio master を試す
            }

            // ── stale check (= clock より古い世代は破棄) ──
            if frame_seek_serial < cur_clock_serial {
                break 'intake;
            }

            // ── raw_pending に積む + seek serial 切替を 1 lock で処理 ──
            let frame_pts_secs = frame.pts_secs;
            let frame_seek_target_secs = frame.seek_target_secs;
            let mut raw_reanchor_reason: Option<RawReanchorReason> = None;
            let mut raw_reanchor_threshold_secs = RAW_OVERFLOW_SECS;
            let raw_total_secs = {
                let mut buf = buffer.lock().unwrap();

                // pump_seek_serial 切替 → processed + raw_pending 両方クリア
                if frame_seek_serial > buf.pump_seek_serial {
                    buf.processed.clear();
                    buf.drain_offset_in_first = 0;
                    buf.raw_pending.clear();
                    buf.next_pts_secs = frame.pts_secs;
                    buf.pump_seek_serial = frame_seek_serial;
                } else if frame_seek_serial < buf.pump_seek_serial {
                    // pump_seek_serial より更に古い (= レース時の保険)
                    drop(buf);
                    break 'intake;
                } else if buf.processed.is_empty()
                    && buf.raw_pending.is_empty()
                    && frame.pts_secs > buf.next_pts_secs
                {
                    // underrun resync は前進方向のみ
                    buf.next_pts_secs = frame.pts_secs;
                }

                // overflow check: raw 合計秒数を計算 (= duration 単位、Codex P2-A 助言)
                let raw_secs: f64 = buf.raw_pending.iter().map(|f| f.duration_secs).sum::<f64>()
                    + frame.duration_secs;

                let raw_overflow = raw_secs > RAW_OVERFLOW_SECS;

                if raw_overflow {
                    buf.processed.clear();
                    buf.drain_offset_in_first = 0;
                    buf.raw_pending.clear();
                    buf.next_pts_secs = frame_pts_secs;
                    buf.raw_pending.push_back(frame);
                    publish_buffer_secs(&buf, &clock);
                    raw_reanchor_reason = Some(RawReanchorReason::Overflow);
                    raw_reanchor_threshold_secs = RAW_OVERFLOW_SECS;
                } else {
                    buf.raw_pending.push_back(frame);
                }
                raw_secs
            };

            // ── overflow / warning 通知 ──
            if let Some(raw_reanchor_reason) = raw_reanchor_reason {
                // Newer behavior: keep this seek generation alive. Clear the
                // queued backlog and treat the current frame as the new audio
                // anchor, which lets old AVI/DivX files recover after seek.
                crate::logger::log(format!(
                    "[audio-pump] raw_pending {} ({:.1}s > {:.1}s threshold) \
                     at seek_serial={frame_seek_serial}: re-anchor at {:.3}s",
                    raw_reanchor_reason.label(),
                    raw_total_secs,
                    raw_reanchor_threshold_secs,
                    frame_pts_secs,
                ));
                clock.zero_audio_tx_queued_secs();
                seek_target_secs = Some(frame_pts_secs);
                if pump_anchor_target_secs.is_none() {
                    pump_anchor_target_secs = frame_seek_target_secs.or(Some(frame_pts_secs));
                }
                #[cfg(windows)]
                if let Some(b) = &dsp_bridge {
                    if b.is_enabled() && b.active_slot_count() > 0 {
                        b.reset_plugins_sync();
                    }
                }
                safety_limiter.reset();
                activated = false;
            } else if raw_total_secs > RAW_WARNING_SECS {
                // warning: 5秒に 1 度ログ。長時間 Buffering の診断用。
                let now = std::time::Instant::now();
                let should_log = match last_warning_at {
                    None => true,
                    Some(t) => now.duration_since(t).as_secs() >= 5,
                };
                if should_log {
                    crate::logger::log(format!(
                        "[audio-pump] raw_pending high water ({:.1}s > {:.1}s warning)",
                        raw_total_secs, RAW_WARNING_SECS,
                    ));
                    last_warning_at = Some(now);
                }
            }

            // audio master 化 (= stale 通過後の最初のフレーム)
            if !activated {
                clock.notify_audio_active();
                activated = true;
            }
        } // 'intake

        // ── pre-refill seek staleness check (Codex 助言、2026-05-01 改訂 + P2 反映) ──
        // timeout tick で起きた場合、'intake で seek serial 更新が走らないため、
        // 旧 seek 世代の raw/processed を保持したまま VST process してしまう可能性がある。
        // 直接 clock.current_seek_serial() と buf.pump_seek_serial を比較し、
        // pump の方が古ければ raw/processed を clear + tx_queued を 0 化 + publish 0 して
        // refill loop を skip。
        //
        // **tx_queued も 0 化** (Codex P2、2026-05-01 改訂):
        // raw/processed だけ消しても、`total_audio_buffer_secs()` には audio_tx_queued が
        // 残る。decoder の `notify_seek_completed()` が走って add_tx_queued の差分が再構築
        // されるまでの窓で、旧世代の audio_tx 残量が pacing に「playable あり」と誤判断
        // させる可能性があった。`zero_audio_tx_queued_secs()` で即座に 0 化。
        // 旧世代の `add_tx_queued(-duration)` が後から届いても `max(0.0)` で clamp される。
        {
            let cur_clock_serial = clock.current_seek_serial();
            let mut buf = buffer.lock().unwrap();
            if buf.pump_seek_serial < cur_clock_serial {
                buf.processed.clear();
                buf.drain_offset_in_first = 0;
                buf.raw_pending.clear();
                clock.zero_audio_tx_queued_secs();
                publish_buffer_secs(&buf, &clock);
                // pump_seek_serial は変更しない (= 次の 'intake で frame.seek_serial 経由
                // で正規ルートで更新される)。本 tick は queue clear だけして outer loop へ。
                continue;
            }
        }

        // ── raw → VST process → processed loop ──
        // mutex を持たずに VST process_block を呼ぶ (Codex P2-B):
        // 1. lock → pop raw_pending → unlock
        // 2. process_block (no lock)
        // 3. lock → seek_serial check → push processed → unlock
        loop {
            // 現在の processed 秒数 (= cap 比較用) を lock 内で取得
            let (current_processed_secs, raw_chunk_opt, target_serial) = {
                let mut buf = buffer.lock().unwrap();
                let cur_secs: f64 = buf.processed.iter().map(|c| c.duration_secs).sum::<f64>()
                    + remaining_first_chunk_secs(&buf);
                if cur_secs >= TARGET_PROCESSED_SECS {
                    (cur_secs, None, buf.pump_seek_serial)
                } else if let Some(raw) = buf.raw_pending.pop_front() {
                    (cur_secs, Some(raw), buf.pump_seek_serial)
                } else {
                    (cur_secs, None, buf.pump_seek_serial)
                }
            };

            let _ = current_processed_secs; // 計測用に取得、未使用なら drop
            let raw = match raw_chunk_opt {
                Some(r) => r,
                None => break,
            };

            // ── VST process_block (mutex 解放中) ──
            #[cfg(windows)]
            let (mut output_samples, mut current_pdc_latency_secs, vst_chain_active): (
                Vec<f32>,
                f64,
                bool,
            ) = if let Some(b) = &dsp_bridge {
                if b.is_enabled() && b.active_slot_count() > 0 {
                    fx_out.resize(raw.samples.len(), 0.0);
                    let success = b.process_block(&raw.samples, &mut fx_out).is_ok();
                    if !success {
                        crate::logger::log("vst3 process_block failed");
                    }
                    let total_lat_samples = b.total_latency_samples();
                    let lat_secs = if total_lat_samples > 0 {
                        total_lat_samples as f64 / sample_rate as f64
                    } else {
                        0.0
                    };
                    if success {
                        (fx_out.clone(), lat_secs, true)
                    } else {
                        (raw.samples.clone(), lat_secs, true)
                    }
                } else {
                    (raw.samples.clone(), 0.0, false)
                }
            } else {
                (raw.samples.clone(), 0.0, false)
            };
            #[cfg(not(windows))]
            let (mut output_samples, mut current_pdc_latency_secs, vst_chain_active): (
                Vec<f32>,
                f64,
                bool,
            ) = (raw.samples.clone(), 0.0, false);

            if vst_chain_active {
                safety_limiter.process_block(&mut output_samples);
                current_pdc_latency_secs += safety_limiter.latency_secs();
            } else {
                safety_limiter.reset();
            }

            // ── chunk metadata 計算 ──
            // audible_pts = input_pts - pdc_latency (Codex P1-3 反映、PDC 正しい同期用)
            let duration_secs = output_samples.len() as f64 / samples_per_sec;
            let audible_pts_secs = (raw.pts_secs - current_pdc_latency_secs).max(0.0);

            // ── pre-target trim (Codex P1-1: PDC plugin で早すぎる BufferReady 防止) ──
            //
            // PDC=N の plugin では post-VST output 最初の N 秒分は delay-line silence
            // (= flush_with_silence で埋めた silence)。これを processed に push すると
            // BufferReady が pre-target silence で fire し、Playing 入場直後に無音 +
            // wrong clock anchor になる。
            //
            // 対策: pre_target_trim_decision で DropAll / TrimFront / KeepAll を判定。
            // 最初の post-target chunk (= TrimFront or KeepAll で push 成功) が
            // 出来た時点で seek_target_secs を None にする (= 以降は無条件 push)。
            let mut samples = output_samples;
            let mut chunk_audible_pts = audible_pts_secs;
            if let Some(target) = seek_target_secs {
                match pre_target_trim_decision(
                    audible_pts_secs,
                    duration_secs,
                    target,
                    samples_per_sec,
                    2,
                ) {
                    TrimResult::DropAll => continue,
                    TrimResult::TrimFront {
                        trim_samples,
                        new_audible_pts,
                    } => {
                        if trim_samples >= samples.len() {
                            continue;
                        }
                        samples.drain(..trim_samples);
                        chunk_audible_pts = new_audible_pts;
                        seek_target_secs = None;
                    }
                    TrimResult::KeepAll => {
                        seek_target_secs = None;
                    }
                }
            }
            if samples.is_empty() {
                continue; // 全 trim で空になった (= 通常ありえないが防御)
            }
            let chunk_duration = samples.len() as f64 / samples_per_sec;
            let chunk = ProcessedChunk {
                samples,
                audible_pts_secs: chunk_audible_pts,
                duration_secs: chunk_duration,
                seek_serial: raw.seek_serial,
                pdc_latency_secs_at_process: current_pdc_latency_secs,
            };

            // ── lock 再取得して processed に push (= seek serial check) ──
            let mut buf = buffer.lock().unwrap();
            if chunk.seek_serial != target_serial || chunk.seek_serial != buf.pump_seek_serial {
                // seek 世代が変わった (= chunk は stale) → drop
                continue;
            }
            // ── cap exceedance check (Codex P2-3): 単 chunk が処理済 cap を超える ──
            // AAC/Opus 等は 23ms/frame なので通常は cap=100ms に余裕。長い frame
            // (= 一部の独自エンコード) では cap を一時的に超える可能性があるので
            // ログだけ出して push する (= 分割は将来課題)。
            let cur_processed_secs: f64 =
                buf.processed.iter().map(|c| c.duration_secs).sum::<f64>()
                    + remaining_first_chunk_secs(&buf);
            if cur_processed_secs + chunk.duration_secs > TARGET_PROCESSED_SECS * 1.5 {
                crate::logger::log(format!(
                    "[audio-pump] processed cap exceeded: {:.3}s + {:.3}s > {:.3}s target \
                     (chunk too large to fit; EQ latency briefly elevated)",
                    cur_processed_secs, chunk.duration_secs, TARGET_PROCESSED_SECS,
                ));
            }
            // PDC latency 変化のログ + 同期 (= 既存挙動、chunk metadata だが
            // global pdc_latency_secs も維持)
            if (buf.pdc_latency_secs - current_pdc_latency_secs).abs() > 1e-6 {
                crate::logger::log(format!(
                    "PDC latency changed: {:.3}ms -> {:.3}ms",
                    buf.pdc_latency_secs * 1000.0,
                    current_pdc_latency_secs * 1000.0
                ));
                buf.pdc_latency_secs = current_pdc_latency_secs;
            }
            buf.processed.push_back(chunk);
        }

        // ── publish_buffer_secs + BufferReady emit ──
        // BufferReady は **processed のみ** で判定 (Codex P1-2、raw を含めない)。
        // pts は **next_audible_pts** = `processed.front().audible_pts + drain_offset/sps`
        // を使う (Codex P1-1 反映、2026-05): seek 直後 next_pts_secs は input pts のまま
        // なので、PDC 適用後の audible 値を渡さないと engine の audio_anchor が
        // target-pdc に固定される。
        //
        // **Codex P1 修正 (2026-05-01)**: Fast モードでは audible_pts = keyframe_pts <
        // user-requested target なので、`audible_pts` 単独だと Buffering→Playing 入場時の
        // anchor が keyframe へ巻き戻る。`pump_anchor_target_secs` (= Flush から焼き付け
        // られた target) との **max** で BufferReady の pts を決定する:
        //   - Precise: audible ≈ target → max(audible, target) ≈ target. 既存挙動と等価
        //   - Fast: audible = keyframe < target → max は target. anchor が target で固定
        //   - 失敗 / 初期 open: pump_anchor_target = None → audible 単独 (既存挙動)
        //   - BufferStarved (再 buffering): audible は再生位置 >> 旧 target → audible 採用
        let (processed_secs, cur_audible_pts, cur_serial) = {
            let buf = buffer.lock().unwrap();
            publish_buffer_secs(&buf, &clock);
            let secs: f64 = buf.processed.iter().map(|c| c.duration_secs).sum::<f64>()
                + remaining_first_chunk_secs(&buf);
            let audible = if let Some(first) = buf.processed.front() {
                first.audible_pts_secs + buf.drain_offset_in_first as f64 / buf.samples_per_sec
            } else {
                buf.next_pts_secs
            };
            (secs, audible, buf.pump_seek_serial)
        };
        if processed_secs >= READY_THRESHOLD_SECS {
            let report_pts = match pump_anchor_target_secs {
                Some(target) => cur_audible_pts.max(target),
                None => cur_audible_pts,
            };
            let _ = engine_event_tx.try_send(crate::video::engine::EngineEvent::Audio(
                crate::video::engine::state::AudioEvent::BufferReady {
                    epoch: cur_serial,
                    pts: report_pts,
                    wall_now: std::time::Instant::now(),
                },
            ));
        }
    }
    // ── 終了時の silence flush (= 既存) ──
    #[cfg(windows)]
    if let Some(b) = &dsp_bridge {
        if b.is_enabled() && b.active_slot_count() > 0 {
            b.flush_silence(480, 10);
        }
    }
    crate::logger::log("audio-pump terminated");
}

/// pre-target trim 結果。
enum TrimResult {
    /// chunk 全体が target 前 (= drop)
    DropAll,
    /// chunk 全体が target 以降 (= 無加工で push、`audible_pts` を返す)
    KeepAll,
    /// chunk が target を跨ぐ (= 先頭を sample-level trim、新 `audible_pts` を返す)
    TrimFront {
        /// 先頭から drop する sample 数 (= channel-aligned)
        trim_samples: usize,
        /// trim 後の chunk 先頭の audible PTS
        new_audible_pts: f64,
    },
}

/// PDC 有効時の pre-target trim 判定 (Codex P1-1 反映)。
///
/// chunk の audible 範囲 [audible_pts, audible_pts + duration) と target を比較:
/// - `audible_end <= target`: chunk 全体が pre-target → DropAll
/// - `audible_pts >= target`: chunk 全体が post-target → KeepAll
/// - 跨ぎ: TrimFront で先頭を `target` 秒 sample-level trim
///
/// 戻り値の `trim_samples` は **interleaved stereo の sample 数** (= channels=2 単位
/// に丸めた値)。呼出側はこの値で `samples.drain(..trim_samples)` する。
fn pre_target_trim_decision(
    audible_pts_secs: f64,
    duration_secs: f64,
    target_secs: f64,
    samples_per_sec: f64,
    channels: usize,
) -> TrimResult {
    let audible_end = audible_pts_secs + duration_secs;
    if audible_end <= target_secs - 1e-6 {
        TrimResult::DropAll
    } else if audible_pts_secs >= target_secs - 1e-6 {
        TrimResult::KeepAll
    } else {
        let trim_secs = target_secs - audible_pts_secs;
        let trim_samples_raw = (trim_secs * samples_per_sec).round() as usize;
        // channel-aligned (= interleaved stereo は 2 単位)
        let trim_samples = (trim_samples_raw / channels) * channels;
        let new_audible_pts = audible_pts_secs + trim_samples as f64 / samples_per_sec;
        TrimResult::TrimFront {
            trim_samples,
            new_audible_pts,
        }
    }
}

fn remaining_first_chunk_secs(buf: &AudioBuffer) -> f64 {
    if let Some(first) = buf.processed.front() {
        let consumed = buf.drain_offset_in_first as f64 / buf.samples_per_sec;
        // 完全 chunk の duration を一度引いて、残り部分だけ加える表現にすると複雑なので、
        // **front 全体は別途 sum() に含む前提で、消費分だけ引く**:
        // sum(duration_secs of all chunks) - drain_offset/samples_per_sec
        // 呼び出し元で `total_secs = sum() + remaining_first_chunk_secs(...)` となるよう、
        // ここでは負の補正値を返す (= 既に sum に含まれた drain_offset 相当を引く)。
        -consumed.min(first.duration_secs)
    } else {
        0.0
    }
}

fn fill_output(
    out: &mut [f32],
    buffer: &Arc<Mutex<AudioBuffer>>,
    clock: &Arc<AvClock>,
    engine_state: &Arc<AtomicU8>,
) {
    // ── pre-seek discard (= state gate より先、Codex P1-4) ──
    let clock_serial = clock.current_seek_serial();
    let mut buf = buffer.lock().unwrap();

    if buf.pump_seek_serial < clock_serial {
        buf.processed.clear();
        buf.drain_offset_in_first = 0;
        buf.raw_pending.clear();
        publish_buffer_secs(&buf, clock);
        out.fill(0.0);
        return;
    }

    // ── EngineState gate (Codex P1-4): PLAYING 以外は silence + 非 drain ──
    //
    // pump は raw_pending に積み続けるので back-pressure 連鎖は発生しない (= Codex
    // P1-1 の overflow handling と組み合わせて demux/audio_decode を止めない設計)。
    if engine_state.load(Ordering::Acquire) != state_code::PLAYING {
        publish_buffer_secs(&buf, clock);
        out.fill(0.0);
        return;
    }

    let want = out.len();

    // 一時停止中は無音 (samples 保持、PTS も進めない)
    if !clock.is_playing() {
        out.fill(0.0);
        return;
    }

    let vol = clock.effective_volume();

    // ── chunk-based drain (Codex P1-1 反映、2026-05) ──
    // processed の先頭 chunk から `drain_offset_in_first` 経由で順に取り出す。
    // **drain した最後のサンプルの audible_pts** を track し、`set_audio_pts` に
    // そのまま渡す (= PDC 引き算は不要、chunk metadata に baked-in 済み)。
    // 旧版は `buf.next_pts_secs - buf.pdc_latency_secs` で video clock を作って
    // いたが、`buf.next_pts_secs` は input pts のままなので PDC=1s で seek すると
    // `pts_for_video = target - 1s` で逆向きジャンプしていた。chunk.audible_pts を
    // 使えば `target` 起点の正しい時刻になる (Codex P1-1 修正)。
    let mut real_consumed: usize = 0;
    let mut written = 0;
    let samples_per_sec = buf.samples_per_sec;
    // 最後に drain したサンプル「直後」の audible PTS (= 次に drain される予定の PTS)。
    // chunk が pop_front されても、その chunk の最終 audible PTS を保持し続ける。
    let mut next_audible_pts: Option<f64> = None;
    // drain 中に chunk が切り替わったか (= PDC latency が変化した可能性)
    let mut chunk_pdc_latency_at_drain: Option<f64> = None;

    while written < want {
        let take = if let Some(first) = buf.processed.front() {
            let remaining = first
                .samples
                .len()
                .saturating_sub(buf.drain_offset_in_first);
            remaining.min(want - written)
        } else {
            0
        };
        if take == 0 {
            // underrun: 残りを silence
            for o in &mut out[written..] {
                *o = 0.0;
            }
            break;
        }
        let first = buf.processed.front().unwrap();
        let chunk_audible_pts = first.audible_pts_secs;
        let chunk_latency = first.pdc_latency_secs_at_process;
        chunk_pdc_latency_at_drain = Some(chunk_latency);

        for i in 0..take {
            out[written + i] = first.samples[buf.drain_offset_in_first + i] * vol;
        }
        written += take;
        real_consumed += take;
        buf.drain_offset_in_first += take;

        // chunk 内 drain 後の audible_pts を計算 (= 次に drain される予定の PTS)
        next_audible_pts =
            Some(chunk_audible_pts + buf.drain_offset_in_first as f64 / samples_per_sec);

        if buf.drain_offset_in_first >= buf.processed.front().map(|c| c.samples.len()).unwrap_or(0)
        {
            buf.processed.pop_front();
            buf.drain_offset_in_first = 0;
        }
    }

    // ── bookkeeping ──
    if real_consumed == 0 {
        publish_buffer_secs(&buf, clock);
        return;
    }

    // **buf.next_pts_secs を audible PTS で更新** (= chunk metadata baked-in)。
    // 以後 publish_buffer_secs / underrun resync は audible PTS ベースで動く。
    if let Some(audible) = next_audible_pts {
        buf.next_pts_secs = audible;
    }
    let pts_for_video = next_audible_pts.unwrap_or(buf.next_pts_secs);
    let pump_serial = buf.pump_seek_serial;

    // PDC latency 変化検出 + ジャンプ判定 (= chunk.pdc_latency_secs_at_process ベース、
    // Codex P2-C 反映)。
    // chunk が切り替わって新 latency が現 applied と差があれば jump。
    const PDC_JUMP_THRESHOLD_SECS: f64 = 0.1;
    let mut latency_jumped = false;
    if let Some(latency) = chunk_pdc_latency_at_drain {
        let delta_secs = latency - buf.pdc_latency_secs_applied;
        if delta_secs.abs() > PDC_JUMP_THRESHOLD_SECS {
            latency_jumped = true;
            crate::logger::log(format!(
                "[VST3 PDC] fill_output: chunk latency change ({:+.1}ms) -> jump video clock to {:.3}s",
                delta_secs * 1000.0,
                pts_for_video
            ));
        }
        if delta_secs.abs() > 1e-6 {
            buf.pdc_latency_secs_applied = latency;
        }
    }

    publish_buffer_secs(&buf, clock);
    drop(buf);

    if pump_serial >= clock.current_seek_serial() {
        if latency_jumped {
            clock.set_audio_pts_jump(pts_for_video);
        } else {
            clock.set_audio_pts(pts_for_video);
        }
        let now = clock.now_secs();
        if (pts_for_video - now).abs() <= SEEK_TARGET_TOLERANCE_SECS {
            clock.clear_seek_target_override(pump_serial);
        }
    }
}

#[cfg(test)]
mod tests {
    //! `fill_output` の bookkeeping invariant をテストで pin する。
    //!
    //! Codex review (= `.claude/codex-reviews/fill-output-bookkeeping-result.md`) の
    //! 提案テスト 2 件 + 完全 drain ケースを実装。実消費ベース bookkeeping が
    //! 各シナリオで意図通り動くことを構造的に保証する。

    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    fn make_clock() -> Arc<AvClock> {
        let seek_serial = Arc::new(AtomicU64::new(0));
        let clock = Arc::new(AvClock::new(0.6, seek_serial));
        clock.set_playing(true);
        clock.notify_audio_active();
        clock
    }

    fn make_buffer(sample_rate: u32) -> Arc<Mutex<AudioBuffer>> {
        Arc::new(Mutex::new(AudioBuffer {
            processed: std::collections::VecDeque::new(),
            drain_offset_in_first: 0,
            raw_pending: std::collections::VecDeque::new(),
            next_pts_secs: 0.0,
            sample_rate,
            samples_per_sec: sample_rate as f64 * 2.0,
            pump_seek_serial: 0,
            pdc_latency_secs: 0.0,
            pdc_latency_secs_applied: 0.0,
        }))
    }

    /// PLAYING engine state を test 用に作る。
    fn playing_state() -> Arc<AtomicU8> {
        Arc::new(AtomicU8::new(state_code::PLAYING))
    }

    /// テスト用の processed chunk を作る。
    fn make_chunk(samples: Vec<f32>, pts_secs: f64, samples_per_sec: f64) -> ProcessedChunk {
        let duration_secs = samples.len() as f64 / samples_per_sec;
        ProcessedChunk {
            samples,
            audible_pts_secs: pts_secs,
            duration_secs,
            seek_serial: 0,
            pdc_latency_secs_at_process: 0.0,
        }
    }

    #[test]
    fn safety_limiter_delays_audio_by_lookahead() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        assert_eq!(limiter.lookahead_frames, 5);

        let mut samples = vec![0.0_f32; 12];
        samples[0] = 0.5;
        samples[1] = -0.5;
        limiter.process_block(&mut samples);

        assert!(
            samples[..10].iter().all(|&v| v == 0.0),
            "first five stereo frames should be lookahead silence"
        );
        assert!((samples[10] - 0.5).abs() < 1e-6);
        assert!((samples[11] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn safety_limiter_catches_lookahead_spike() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut samples = vec![0.0_f32; 18];
        samples[0] = 0.25;
        samples[1] = -0.25;
        samples[2] = 2.0;
        samples[3] = -2.0;

        limiter.process_block(&mut samples);

        let ceiling = limiter.ceiling;
        let max_abs = samples.iter().fold(0.0_f32, |acc, &v| acc.max(v.abs()));
        assert!(
            max_abs <= ceiling + 1e-6,
            "limiter output should stay under ceiling ({max_abs} > {ceiling})"
        );
        assert!(
            samples.iter().any(|&v| v.abs() > 0.0),
            "delayed non-silence should still pass through"
        );
    }

    #[test]
    fn safety_limiter_keeps_delay_across_blocks() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut first = vec![0.0_f32; 6];
        first[0] = 0.5;
        first[1] = -0.5;
        limiter.process_block(&mut first);
        assert!(
            first.iter().all(|&v| v == 0.0),
            "first block is shorter than lookahead, so it should be delayed"
        );

        let mut second = vec![0.0_f32; 6];
        limiter.process_block(&mut second);
        assert!((second[4] - 0.5).abs() < 1e-6);
        assert!((second[5] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn safety_limiter_releases_gain_across_blocks() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut loud = vec![0.0_f32; 14];
        loud[0] = 2.0;
        loud[1] = -2.0;
        limiter.process_block(&mut loud);
        let gain_after_peak = limiter.gain;
        assert!(
            gain_after_peak < 0.6,
            "loud peak should force immediate gain reduction"
        );

        let mut quiet = vec![0.0_f32; 200];
        limiter.process_block(&mut quiet);
        assert!(
            limiter.gain > gain_after_peak,
            "gain should recover across later blocks"
        );
        assert!(limiter.gain < 1.0, "release should be gradual, not instant");
    }

    #[test]
    fn safety_limiter_reset_clears_delay_line() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut samples = vec![0.5_f32; 12];
        limiter.process_block(&mut samples);
        limiter.reset();

        let mut silence = vec![0.0_f32; 12];
        limiter.process_block(&mut silence);
        assert!(
            silence.iter().all(|&v| v == 0.0),
            "reset should prevent old delayed audio from leaking"
        );
    }

    /// 完全 underrun (= processed 空) で callback が来ても `next_pts_secs` が進まない。
    #[test]
    fn fill_output_empty_buffer_does_not_advance_pts() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let pts_before = buf.lock().unwrap().next_pts_secs;

        let mut out = [1.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &playing_state());

        let pts_after = buf.lock().unwrap().next_pts_secs;
        assert_eq!(
            pts_before, pts_after,
            "empty buffer must not advance next_pts_secs"
        );
        assert!(
            out.iter().all(|&s| s == 0.0),
            "output should be all silence on full underrun"
        );
    }

    /// 部分 drain: chunk に 100 samples だけ入れる → 100 sample drain → 残り silence。
    #[test]
    fn fill_output_partial_drain_advances_only_real_consumed() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let samples_per_sec = buf.lock().unwrap().samples_per_sec;

        {
            let mut b = buf.lock().unwrap();
            b.processed
                .push_back(make_chunk(vec![0.5; 100], 0.0, samples_per_sec));
        }
        let pts_before = buf.lock().unwrap().next_pts_secs;

        let mut out = [0.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &playing_state());

        let pts_after = buf.lock().unwrap().next_pts_secs;
        let expected_advance = 100.0 / (48_000.0 * 2.0);
        assert!(
            (pts_after - pts_before - expected_advance).abs() < 1e-12,
            "partial drain: pts must advance by real_consumed/samples_per_sec only",
        );
        for (i, &v) in out.iter().take(100).enumerate() {
            assert!((v - 0.3).abs() < 1e-6, "sample {i} should be 0.3, got {v}");
        }
        for (i, &v) in out.iter().skip(100).enumerate() {
            assert_eq!(v, 0.0, "sample {} should be silence", i + 100);
        }
    }

    /// 完全 drain: chunk に 480 samples → drain 完了。
    #[test]
    fn fill_output_full_drain_advances_full_amount() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let samples_per_sec = buf.lock().unwrap().samples_per_sec;

        {
            let mut b = buf.lock().unwrap();
            b.processed
                .push_back(make_chunk(vec![0.5; 480], 0.0, samples_per_sec));
        }
        let pts_before = buf.lock().unwrap().next_pts_secs;

        let mut out = [0.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &playing_state());

        let pts_after = buf.lock().unwrap().next_pts_secs;
        let expected_advance = 480.0 / (48_000.0 * 2.0);
        assert!(
            (pts_after - pts_before - expected_advance).abs() < 1e-12,
            "full drain advances pts by want/samples_per_sec",
        );
    }

    /// engine PLAYING 以外のときは silence + buffer drain しない。
    #[test]
    fn fill_output_silences_when_not_playing_state() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let samples_per_sec = buf.lock().unwrap().samples_per_sec;

        {
            let mut b = buf.lock().unwrap();
            b.processed
                .push_back(make_chunk(vec![0.5; 480], 0.0, samples_per_sec));
        }
        let len_before: usize = buf
            .lock()
            .unwrap()
            .processed
            .iter()
            .map(|c| c.samples.len())
            .sum();

        let buffering_state = Arc::new(AtomicU8::new(state_code::BUFFERING));
        let mut out = [1.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &buffering_state);

        let len_after: usize = buf
            .lock()
            .unwrap()
            .processed
            .iter()
            .map(|c| c.samples.len())
            .sum();
        assert_eq!(len_before, len_after, "Buffering: must not drain processed");
        assert!(
            out.iter().all(|&s| s == 0.0),
            "Buffering: output must be silence"
        );
    }

    /// PDC pre-target trim: chunk 全体が target 前なら DropAll。
    #[test]
    fn pre_target_trim_drops_chunk_fully_before_target() {
        // PDC = 1.0 sec、input pts = 5.0、duration = 0.023 (= 1 frame)
        // audible_pts = 5.0 - 1.0 = 4.0、audible_end = 4.023
        // target = 5.0 → audible_end < target → DropAll
        let result = pre_target_trim_decision(4.0, 0.023, 5.0, 96000.0, 2);
        assert!(matches!(result, TrimResult::DropAll));
    }

    /// PDC pre-target trim: chunk 全体が target 以降なら KeepAll。
    #[test]
    fn pre_target_trim_keeps_chunk_fully_after_target() {
        // input pts = 5.5、PDC = 0.0、audible_pts = 5.5
        // target = 5.0 → audible_pts >= target → KeepAll
        let result = pre_target_trim_decision(5.5, 0.023, 5.0, 96000.0, 2);
        assert!(matches!(result, TrimResult::KeepAll));
    }

    /// PDC pre-target trim: chunk が target を跨ぐ → TrimFront。
    #[test]
    fn pre_target_trim_splits_chunk_crossing_target() {
        // PDC = 0.5、input pts = 5.5、duration = 1.0、audible_pts = 5.0
        // target = 5.5 → 跨ぐ → 先頭 0.5 sec trim、new_audible = 5.5
        // sample_per_sec = 96000、trim = 0.5 * 96000 = 48000 samples (channel-aligned)
        let result = pre_target_trim_decision(5.0, 1.0, 5.5, 96000.0, 2);
        match result {
            TrimResult::TrimFront {
                trim_samples,
                new_audible_pts,
            } => {
                assert_eq!(trim_samples, 48000);
                assert!((new_audible_pts - 5.5).abs() < 1e-6);
            }
            _ => panic!("expected TrimFront"),
        }
    }

    /// PDC pre-target trim: PDC=1s + seek_target=10 のシナリオ。
    /// pump push 結果として最初の 1 秒は drop、2 秒目以降が processed に入るかの確認。
    #[test]
    fn pre_target_trim_pdc_one_second_seek_to_ten() {
        let target = 10.0;
        let pdc = 1.0;
        let frame_duration = 0.023;
        let sps = 96000.0;

        // input pts は target..target+pdc..target+pdc+0.1 まで増えていく。
        // audible_pts は (target-pdc)..target..target+0.1 と進む。
        // 各 frame の判定:
        //   audible_end <= target - eps: DropAll
        //   audible_pts >= target - eps: KeepAll (= 完全 post-target)
        //   それ以外: TrimFront (= 跨ぎ)
        // PDC=1s なので「最初の ~43 frame は DropAll」「~44 frame目で跨ぎ TrimFront」
        // 「45 frame目以降は KeepAll」になる想定。これを構造的に検証:
        let mut input_pts = target;
        let mut saw_drop_all = false;
        let mut saw_trim_front = false;
        let mut saw_keep_all = false;
        for _ in 0..50 {
            let audible_pts = input_pts - pdc;
            let result = pre_target_trim_decision(audible_pts, frame_duration, target, sps, 2);
            match result {
                TrimResult::DropAll => saw_drop_all = true,
                TrimResult::TrimFront { .. } => saw_trim_front = true,
                TrimResult::KeepAll => saw_keep_all = true,
            }
            input_pts += frame_duration;
        }
        assert!(saw_drop_all, "early frames should be DropAll");
        assert!(saw_trim_front, "transitional frame should be TrimFront");
        assert!(saw_keep_all, "later frames should be KeepAll");
    }

    /// chunk の audible_pts ベースで clock が更新される (Codex P1-1 修正の検証)。
    /// PDC=1s で seek=10 のシナリオを模擬: chunk.audible_pts = 10.0、drain 240 samples
    /// (= 0.0025秒@96kHz) → next_pts_secs = 10.0025、clock の audio_pts も同値。
    /// 旧コード `pts_for_video = next_pts - pdc_latency` だと 9.0025 になり target-pdc
    /// に逆ジャンプしていた。
    #[test]
    fn fill_output_uses_chunk_audible_pts_for_clock() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let samples_per_sec = buf.lock().unwrap().samples_per_sec;
        // PDC=1s plugin で input pts=11.0、audible=10.0 (= seek_target) の chunk
        let target_audible = 10.0;
        {
            let mut b = buf.lock().unwrap();
            // global pdc_latency_secs (= legacy field) は 1.0 にしておく (= PDC ON 状態)
            b.pdc_latency_secs = 1.0;
            b.pdc_latency_secs_applied = 1.0;
            // initial next_pts_secs は 0 のまま (= 旧コードでは pts_for_video=-1.0 になる)
            b.next_pts_secs = 0.0;
            // PDC=1s で input pts=11.0 → audible_pts=10.0 (= target)
            let chunk = ProcessedChunk {
                samples: vec![0.5; 480],
                audible_pts_secs: target_audible,
                duration_secs: 480.0 / samples_per_sec,
                seek_serial: 0,
                pdc_latency_secs_at_process: 1.0,
            };
            b.processed.push_back(chunk);
        }

        let mut out = [0.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &playing_state());

        // drain 後、buf.next_pts_secs が audible_pts ベースで更新されている
        let expected_audible_after = target_audible + 480.0 / samples_per_sec;
        let actual = buf.lock().unwrap().next_pts_secs;
        assert!(
            (actual - expected_audible_after).abs() < 1e-9,
            "next_pts_secs should be chunk.audible_pts + drain_offset/sps \
             (expected ~{expected_audible_after}, got {actual})",
        );
        // clock.now_secs() も target_audible 付近のはず (= 旧バグだと target-1s 付近)
        let clock_now = clock.now_secs();
        assert!(
            clock_now > target_audible - 0.1,
            "clock should be near target_audible ({target_audible}), not target-pdc \
             (got {clock_now})",
        );
    }

    /// 複数 chunk からの drain: 240 + 240 = 480 samples を 1 callback で取る。
    #[test]
    fn fill_output_drains_across_multiple_chunks() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let samples_per_sec = buf.lock().unwrap().samples_per_sec;

        {
            let mut b = buf.lock().unwrap();
            b.processed
                .push_back(make_chunk(vec![0.5; 240], 0.0, samples_per_sec));
            b.processed
                .push_back(make_chunk(vec![0.25; 240], 0.0, samples_per_sec));
        }

        let mut out = [0.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &playing_state());

        // 最初 240 samples: 0.5 * 0.6 = 0.3
        for &v in out.iter().take(240) {
            assert!((v - 0.3).abs() < 1e-6);
        }
        // 後 240 samples: 0.25 * 0.6 = 0.15
        for &v in out.iter().skip(240) {
            assert!((v - 0.15).abs() < 1e-6);
        }
        // 両 chunk drain 済み
        assert_eq!(buf.lock().unwrap().processed.len(), 0);
    }
}
