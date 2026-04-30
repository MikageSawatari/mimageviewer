//! 動画再生用の音声出力 (cpal / WASAPI Shared)。
//!
//! [`super::decoder`] が送ってくる [`super::decoder::AudioFrame`] (interleaved stereo f32)
//! を受け、`cpal` の出力ストリームに流す。AV 同期のため、コールバックで「直近に
//! 出力したサンプルの PTS」を [`super::clock::AvClock::set_audio_pts`] で報告する。
//!
//! ## アーキテクチャ
//! - 別スレッド (`audio-pump`) が `audio_rx` から AudioFrame を取り出し、共有
//!   ring buffer に push する (バッファ目安 ~1 秒分)。
//! - `cpal` の出力ストリーム (= `cpal` 内部の RT スレッド) が ring buffer から pop して
//!   出力バッファに書き込む。バッファ枯渇時は無音で埋める。
//!
//! ## 注意
//! `cpal::Stream` は !Send なので、cpal の RT スレッドに「閉じ込めて」管理する必要がある。
//! [`AudioOutput`] が drop されたら自動で停止する。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, bounded};

use super::clock::{AvClock, SEEK_TARGET_TOLERANCE_SECS};
use super::decoder::AudioFrame;

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

/// 共有 ring buffer (interleaved stereo f32)。Mutex 保護。
/// RT 性能が必要なら lock-free に置き換え。今は WASAPI Shared 数 ms 周期なので
/// Mutex で実用上問題ない (コンテンションは pump thread と RT 1:1)。
struct AudioBuffer {
    /// 未消費サンプル列 (interleaved stereo)。
    samples: std::collections::VecDeque<f32>,
    /// `samples` の先頭サンプル (= 次に消費されるサンプル) の PTS (秒)。
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
        samples: std::collections::VecDeque::with_capacity((sample_rate as usize) * 2),
        next_pts_secs: 0.0,
        sample_rate,
        samples_per_sec: sample_rate as f64 * 2.0,
        pump_seek_serial: 0,
        pdc_latency_secs: 0.0,
        pdc_latency_secs_applied: 0.0,
    }));

    let cancel = Arc::new(AtomicBool::new(false));
    let (shutdown_tx, shutdown_rx) = bounded::<()>(1);

    // ── pump thread: audio_rx → (optional VST3 bridge) → buffer ──
    let pump_buffer = buffer.clone();
    let pump_cancel = cancel.clone();
    let pump_clock = clock.clone();
    let pump_engine_event_tx = engine_event_tx.clone();
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
                #[cfg(windows)]
                pump_dsp_bridge,
            );
        })
        .map_err(|e| format!("spawn audio-pump: {e}"))?;

    // ── cpal output callback: buffer → device ──
    let cb_buffer = buffer.clone();
    let cb_clock = clock.clone();
    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                fill_output(out, &cb_buffer, &cb_clock);
            },
            |err| crate::logger::log(format!("cpal output stream error: {err}")),
            None,
        )
        .map_err(|e| format!("build_output_stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("stream.play: {e}"))?;

    Ok(AudioOutput {
        stream: Some(stream),
        cancel,
        shutdown_tx,
        pump: Some(pump_handle),
        sample_rate,
    })
}

/// `AudioBuffer.samples` の長さを秒に変換して clock に publish する。
/// pump push / fill_output pop の両方から呼ばれる。
///
/// **PDC latency は pump_buf に含めない** (= Codex 助言、2026-05-01)。
/// 旧版は `secs + pdc_latency_secs` を publish していたが、それだと AudioBuffer が
/// 完全に空 (= cpal underrun 中) でも「PDC 分のバッファあり」に見えてしまい、
/// decoder pacing の `audio_escape` / emergency 補充が発動せず、結果として高 latency 時に
/// 音声がブツブツ途切れる退行を起こす。
///
/// **PDC latency は別 metric で publish する** (`set_vst3_pdc_latency_secs`)。
/// decoder pacing 側は actual buffer 残量で `audio_escape` を判定し、先読み許可量だけ
/// `PACE_LEAD + pdc_latency` を加算する設計。
fn publish_buffer_secs(buf: &AudioBuffer, clock: &AvClock) {
    let secs = buf.samples.len() as f64 / buf.samples_per_sec;
    clock.set_audio_pump_buf_secs(secs);
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

/// `dsp_bridge` が渡されていて enable 状態のとき、pump で受け取った frame を
/// VST3 プラグイン経由に通してから AudioBuffer に push する。
fn run_pump(
    rx: Receiver<AudioFrame>,
    shutdown_rx: crossbeam_channel::Receiver<()>,
    buffer: Arc<Mutex<AudioBuffer>>,
    cancel: Arc<AtomicBool>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<crate::video::engine::EngineEvent>,
    #[cfg(windows)] dsp_bridge: Option<std::sync::Arc<crate::video::dsp::DspBridge>>,
) {
    #[cfg(windows)]
    boost_audio_pump_priority();

    // VST3 プラグイン処理用の出力バッファ (= bridge.process_block の dst)。
    // frame サイズに応じて伸縮させるが、頻繁な realloc を避けるため
    // 初期容量を 4096 (= 約 0.04 秒@48kHz stereo) で確保する。
    #[cfg(windows)]
    let mut fx_out: Vec<f32> = Vec::with_capacity(4096);
    // ── 出力バッファ厚 (audio_buffer の cap) ──
    //
    // この長さは「VST プラグインが処理した audio が **スピーカーから出るまでの** 遅延」
    // に直結する。pump → bridge → audio_buffer → cpal → 出力 のパイプラインで、
    // pump はバッファが満杯にならない限りすぐに次のフレームを処理するため、
    // バッファ fill = 「**プラグインで加工済みの audio が並んでいる量**」となる。
    //
    // ユーザーが EQ ノブを動かす → プラグインの新しい係数で audio 加工 →
    // pump が新加工 audio を audio_buffer に push → cpal が audio_buffer から順次
    // pop して出力。**ユーザー操作 → 音への反映 = audio_buffer fill 量**。
    //
    // 旧版は 1.5 秒固定にしていたが、ユーザー報告 (2026-04) で「EQ 反映が
    // 数百 ms 遅れる」が判明 → **300 ms に縮小**して反応性を確保する。
    // 0.5 秒以下にすると cpal の RT 周期 (= 10-20 ms) と pump の処理時間ジッタ
    // で稀に underrun (= 一瞬の無音) が出るリスクがあるが、300 ms あれば
    // VST3 bridge IPC roundtrip (= ~1-5 ms) の数十倍の余裕があるので実用上は安定。
    //
    // ※ samples は interleaved stereo (channels=2)。
    // sample_rate は構築時に固定なので 1 度だけロックして拾う。
    const TARGET_BUFFER_SECS: f64 = 0.3;
    let cap_samples = {
        let b = buffer.lock().unwrap();
        (b.sample_rate as f64 * 2.0 * TARGET_BUFFER_SECS) as usize
    };

    // EngineActor::Buffering → Playing 遷移トリガとなる buffer 厚さ (秒)。
    // 設計 v3 では 500ms (= 250ms low + hysteresis 想定) で確定したが、Phase 8.K で
    // 実測した結果、典型的な audio_buf は 200-400ms 範囲を hover し 500ms に到達しない
    // ことが判明 (= 単一 demux+decode スレッドで video pacing と競合している影響)。
    // 結果として BufferReady が発火せず engine が永久 Buffering、video decoder の
    // pace_lead=0 で「ahead < 0.20s cap」状態が続き future_frames が枯渇 → buf strip
    // が常時黄色になっていた。150ms に下げて typical level の下回りに合わせる。
    // (= 後続の demux thread split refactor で本来の余裕に戻したい)
    const READY_THRESHOLD_SECS: f64 = 0.15;

    let mut activated = false;
    // VST3 sync reset 用の世代追跡。新 seek 世代の最初の有効 frame を検出したら、
    // 「VST process_block の前」に sync reset (= bridge audio thread が in/out ring drain
    // + plugin reset + ResetDone ack) を実行し、ack 受信後に post-seek audio を流す。
    // これで pre-seek audio が plugin delay-line から漏れることを防ぐ
    // (= Codex 助言、2026-05-01、シーク後 pre-seek audio 残留問題の解消)。
    let mut last_seen_seek_serial: u64 = 0;
    while !cancel.load(Ordering::Acquire) {
        // shutdown 通知が先に来たら即抜ける (drop 時のレイテンシ削減)。
        // 通常時は audio_rx の recv_timeout で 100ms 周期に cancel 確認。
        let frame = crossbeam_channel::select! {
            recv(shutdown_rx) -> _ => return,
            recv(rx) -> msg => match msg {
                Ok(f) => f,
                Err(_) => break, // disconnected
            },
        };

        // recv 直後に audio_tx queued 合計から減算 (decoder send 時 + に対応)
        clock.add_audio_tx_queued_secs(-frame.duration_secs);

        // ── 新 seek 世代の検出 → VST plugin sync reset (= 内部 delay-line + ring 完全 flush) ──
        // PDC が大きいプラグインは内部 delay-line に pre-seek 音声を持っており、
        // 単純な non-sync reset では bridge audio thread と GUI thread の race で
        // pre-seek audio が漏れていた。bridge 側で audio thread 自身が reset を実行し、
        // ResetDone ack を待つことで race 完全排除 (= 200ms timeout、Codex 助言)。
        // skip された stale frame では発火しない (= 後述の skip check より前なので
        // pump_seek_serial と clock_serial の比較も含む)。
        let frame_seek_serial = frame.seek_serial;
        let cur_clock_serial = clock.current_seek_serial();
        if frame_seek_serial > last_seen_seek_serial
            && frame_seek_serial >= cur_clock_serial
        {
            #[cfg(windows)]
            if let Some(b) = &dsp_bridge {
                if b.is_enabled() && b.active_slot_count() > 0 {
                    b.reset_plugins_sync();
                }
            }
            last_seen_seek_serial = frame_seek_serial;
        }

        // バッファ満杯なら一旦待つ。Condvar 化は将来の最適化。
        loop {
            let len = buffer.lock().unwrap().samples.len();
            if len < cap_samples || cancel.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // ── VST3 plugin processing (optional) ──
        // bridge が enable=true & active_slot_count > 0 のときだけ frame.samples を bridge に通す。
        // 通常の動画再生 (= VST3 disable / 全 bypass) ではゼロオーバーヘッド (= 2 つの atomic 読み)。
        // 同時に PDC (Plugin Delay Compensation) 用の latency 値も取得しておく。
        #[cfg(windows)]
        let (processed_samples, current_pdc_latency_secs): (std::borrow::Cow<'_, [f32]>, f64) =
            if let Some(b) = &dsp_bridge {
                if b.is_enabled() && b.active_slot_count() > 0 {
                    fx_out.resize(frame.samples.len(), 0.0);
                    let processed: std::borrow::Cow<'_, [f32]> =
                        if let Err(e) = b.process_block(&frame.samples, &mut fx_out) {
                            crate::logger::log(format!("vst3 process_block failed: {e}"));
                            std::borrow::Cow::Borrowed(&frame.samples[..])
                        } else {
                            std::borrow::Cow::Borrowed(&fx_out[..])
                        };
                    // PDC: アクティブスロット (= !bypass && Loaded) の latency_samples を合算。
                    // sample_rate は buffer 構築時に固定なので、frame.sample_rate ではなく
                    // buffer の sample_rate を使う (= cpal output と同じ rate)。
                    let total_lat_samples = b.total_latency_samples();
                    let lat_secs = if total_lat_samples > 0 {
                        let sr = buffer.lock().unwrap().sample_rate;
                        total_lat_samples as f64 / sr as f64
                    } else {
                        0.0
                    };
                    (processed, lat_secs)
                } else {
                    (std::borrow::Cow::Borrowed(&frame.samples[..]), 0.0)
                }
            } else {
                (std::borrow::Cow::Borrowed(&frame.samples[..]), 0.0)
            };
        #[cfg(not(windows))]
        let processed_samples: &[f32] = &frame.samples;
        #[cfg(not(windows))]
        let current_pdc_latency_secs: f64 = 0.0;

        // 古い seek_serial の破棄 + push を 1 ロックで実行。
        // clock.current_seek_serial() より古いフレームは、audio_tx に
        // 積まれていた pre-seek の遅延フレームなので捨てる (新世代に追い付くため)。
        let clock_serial = clock.current_seek_serial();
        let mut buf = buffer.lock().unwrap();
        if frame.seek_serial < buf.pump_seek_serial || frame.seek_serial < clock_serial {
            continue;
        }
        // audio master 化は **stale 破棄を通過したフレーム** で実施
        // (= pre-seek の古い frame で audio master をフラグ立てない)。
        if !activated {
            clock.notify_audio_active();
            activated = true;
        }
        if frame.seek_serial > buf.pump_seek_serial {
            // 新しい seek 世代: バッファをクリアして PTS を再設定
            buf.samples.clear();
            buf.next_pts_secs = frame.pts_secs;
            buf.pump_seek_serial = frame.seek_serial;
            // Phase 8.K: BufferReady は level event 化したため latch 不要。
            // (旧 buffer_ready_emitted_serial は撤去)
        } else if buf.samples.is_empty() && frame.pts_secs > buf.next_pts_secs {
            // underrun resync は前進方向のみ許可 (clock 後退を防ぐため)
            buf.next_pts_secs = frame.pts_secs;
        }
        buf.samples.extend(processed_samples.iter().copied());
        // PDC (Plugin Delay Compensation) latency を反映。fill_output で video clock
        // 計算時に減算される。値が変化したらログに残す (= プラグインモード切替の検出)。
        if (buf.pdc_latency_secs - current_pdc_latency_secs).abs() > 1e-6 {
            crate::logger::log(format!(
                "PDC latency changed: {:.3}ms -> {:.3}ms",
                buf.pdc_latency_secs * 1000.0,
                current_pdc_latency_secs * 1000.0
            ));
            buf.pdc_latency_secs = current_pdc_latency_secs;
        }
        publish_buffer_secs(&buf, &clock);

        // ── BufferReady emit (Phase 3d / 8.K) ──
        // buffer 残量が READY_THRESHOLD に到達したら EngineActor に通知する
        // (= Buffering → Playing 遷移トリガ)。
        //
        // Phase 8.K (Codex P1): 旧コードは「同 seek_serial 内では 1 度だけ」
        // (`buffer_ready_emitted_serial`) で edge event 化していたが、エンジンが
        // Loading 状態の間に届くと InfoReceived → transition_to_buffering で
        // latch がリセットされ、その後二度と emit されない race があった。
        // → level event 化し、threshold を超えている間は毎 frame emit する。
        // EngineActor 側は idempotent (`latch.buffer_ready=true` の repeat set)
        // なので過剰 emit は無害。`bounded(64)` の channel は full なら try_send
        // で drop されるが、次 frame の emit で復旧する。
        let buf_secs = buf.samples.len() as f64 / buf.samples_per_sec;
        let cur_pts = buf.next_pts_secs;
        let cur_serial = buf.pump_seek_serial;
        drop(buf);
        if buf_secs >= READY_THRESHOLD_SECS {
            let _ = engine_event_tx.try_send(
                crate::video::engine::EngineEvent::Audio(
                    crate::video::engine::state::AudioEvent::BufferReady {
                        epoch: cur_serial,
                        pts: cur_pts,
                        wall_now: std::time::Instant::now(),
                    },
                ),
            );
        }
    }
    // ── 終了時の silence flush ──
    // VST3 bridge の in_ring に残った最終 audio がプラグイン visualizer (LUFS / EQ
    // analyzer 等) に表示され続けるのを止めるため、無音ブロックを 10 個 (= 約 100ms)
    // 流して bridge → プラグインの内部状態を 0 に落とす。
    // bridge プロセス自体は DspBridge に保持されたまま生存するので、次の動画再生で
    // 再利用される。
    #[cfg(windows)]
    if let Some(b) = &dsp_bridge {
        if b.is_enabled() && b.active_slot_count() > 0 {
            // block_size は decoder/cpal と同じ 480 で固定 (= 暫定)。
            b.flush_silence(480, 10);
        }
    }
    crate::logger::log("audio-pump terminated");
}

fn fill_output(
    out: &mut [f32],
    buffer: &Arc<Mutex<AudioBuffer>>,
    clock: &Arc<AvClock>,
) {
    // ── 設計 (counter consolidation 後の bookkeeping 上流移動) ──
    //
    // 旧版は cpal callback ごとに無条件で `next_pts_secs += want / samples_per_sec`
    // を加算していた (= 「常に full 期間分進める」)。silence 出力中も進むので、
    // cpal stream 起動直後 (~50ms) の pre-fill burst で callback が連続発火すると
    // anchor pts が wall の 2-3x 速で前進、decoder の pacing が誤判定して future_frames
    // が枯渇する問題があった (= Phase 9.A 報告)。
    //
    // 旧版の 2 段防御の現状:
    //   - Phase 9.B/9.E LOADING/IDLE silence gate: **撤去** (= 上流 bookkeeping で対処済)
    //   - Phase 9.A `set_audio_pts` wall-rate cap: **保持** (defensive safety net、
    //     詳細は `clock.rs::set_audio_pts` の doc コメント参照)
    //
    // 新版: **実際に消費したサンプル数だけ `next_pts_secs` を進める** (= bookkeeping を
    // 上流に正確化)。silence 出力 (= underrun または warmup で buffer 空) では
    // `real_consumed = 0` となり pts は 1 mode も進まない → silence gate は不要に。
    // wall-rate cap は通常動作で無発動だが、buffer 非空での pre-fill burst (= callback
    // 連続 pop が wall 進行を超えるシナリオ) への保険として残す (Codex P? 反映)。
    //
    // 残った早期 return:
    //   - pre-seek サンプル全消去 (= clock_serial > pump_serial)
    //   - 一時停止中の silence (= clock.is_playing()=false)
    // どちらも「**buffer から pop しない**」設計上必要な分岐。
    let clock_serial = clock.current_seek_serial();
    let mut buf = buffer.lock().unwrap();

    if buf.pump_seek_serial < clock_serial {
        // 古い (pre-seek) サンプルを破棄。pump がすぐに新世代を埋めてくれる。
        buf.samples.clear();
        // pacing が古い残量を読まないよう 0 を publish (Mutex 内、stale 上書き race 回避)。
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

    // ── 実消費サンプル数を数えながらドレイン ──
    let mut real_consumed: usize = 0;
    let mut written = 0;
    while written < want {
        match buf.samples.pop_front() {
            Some(s) => {
                out[written] = s * vol;
                written += 1;
                real_consumed += 1;
            }
            None => {
                // underrun: 残りを silence で埋める。real_consumed はここで止まる。
                for o in &mut out[written..] {
                    *o = 0.0;
                }
                break;
            }
        }
    }

    // ── bookkeeping (実消費分のみ) ──
    if real_consumed == 0 {
        // 完全 underrun (= buffer 空 から callback 来た)。pts を進めず、buffer 状態を
        // pump 側に publish して終了。warmup 期間の pre-fill burst が来ても pts drift
        // しない (= 旧 Phase 9.A wall-rate cap / 9.B silence gate を不要にする根拠)。
        publish_buffer_secs(&buf, clock);
        return;
    }
    let consumed_secs = real_consumed as f64 / buf.samples_per_sec;
    buf.next_pts_secs += consumed_secs;
    let pts_now = buf.next_pts_secs;
    let pump_serial = buf.pump_seek_serial;
    // PDC (Plugin Delay Compensation): video clock 用の pts は input PTS から
    // プラグインチェーン latency を引いた時刻。プラグインの output 1 sample が
    // 実際に対応する input の時刻 = pts_now - pdc_latency_secs。
    // VST 無効 / 全 bypass のときは pdc_latency_secs = 0 なので影響なし (= 既存動作)。
    let pdc_latency = buf.pdc_latency_secs;
    let pts_for_video = (pts_now - pdc_latency).max(0.0);
    // PDC latency 変化検出 + 閾値付きジャンプ判定:
    // - **大きい変化 (> 100ms)**: monotonic guard をバイパスして video clock を強制
    //   再設定 (= 「映像ジャンプ」)。プラグインモード切替や linear-phase ON/OFF 等の
    //   ステップ変化を凍結時間なしで反映する。
    // - **小さい変化 (≤ 100ms)**: 通常 set_audio_pts を呼ぶ (= monotonic + wall-rate cap)。
    //   小刻みなノイズ (= プラグインが ±20-50ms の jitter を出すケース) でも頻繁な
    //   ジャンプを発生させず、滑らかな再生を維持する。Δ100ms 以内の遅れ/早回しは
    //   人間の知覚しきい値以下で実用上気付かない。
    //
    // 閾値の根拠: 一般的な VST plugin の latency は 0 / 数ms / 数十ms / 数百ms /
    // 1秒以上 の段差で離散的に変化する。100ms はモード切替を確実に拾えて、サンプル
    // 単位ノイズ (= ±100ms 以下) には反応しない実用的な値。
    const PDC_JUMP_THRESHOLD_SECS: f64 = 0.1;
    let delta_secs = pdc_latency - buf.pdc_latency_secs_applied;
    let latency_jumped = delta_secs.abs() > PDC_JUMP_THRESHOLD_SECS;
    // 値変化検出のしきい値は微小 (= 1us)。jitter でも applied は更新する
    // (= 累積による誤検知防止)。jump 判定は別の閾値で行う。
    if delta_secs.abs() > 1e-6 {
        buf.pdc_latency_secs_applied = pdc_latency;
        if latency_jumped {
            crate::logger::log(format!(
                "[VST3 PDC] fill_output: large latency change ({:+.1}ms) -> jump video clock to {:.3}s",
                delta_secs * 1000.0,
                pts_for_video
            ));
        }
    }
    // publish は Mutex 内で行う。Mutex 外に出すと run_pump の push 後 publish が
    // stale な fill_output 側 publish に上書きされる race がある。
    // overhead = 1 atomic store ≈ 1 ns 程度なので RT への影響は無視できる。
    publish_buffer_secs(&buf, clock);
    drop(buf);
    // pump が pre-seek サンプルを抱えている間 (clock_serial > pump_serial) に
    // set_audio_pts を呼ぶとシーク前位置でクロックが進み、シークバーがスナップバック。
    // 二段読みで race を排除する。
    if pump_serial >= clock.current_seek_serial() {
        if latency_jumped {
            // PDC latency 変化時: monotonic guard をバイパスして anchor を強制再設定。
            // 後退方向 (= latency 増加) でも前進方向 (= latency 減少) でも、video clock が
            // 即座に新しい pts_for_video へジャンプし、その後通常 anchor 更新に戻る。
            clock.set_audio_pts_jump(pts_for_video);
        } else {
            clock.set_audio_pts(pts_for_video);
        }
        // override クリアは「target 近傍のサンプル」を消費した時のみ。
        // target から大きく離れた場合は decoder 側 seek が外れて古い位置の audio が
        // 新世代 serial で来ているケースで、ここで clear するとシークバーが
        // 元位置に戻る (override 中は now_secs が target を返すので diff で判定可能、
        // 通常再生中は now_secs ≈ pts_now なので diff ~0 で常に clear する)。
        // NB: override クリア判定は pts_for_video ベース (= clock.now_secs() と同じ
        // 時間軸) で行うので PDC 適用後の値で比較する。
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
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    fn make_clock() -> Arc<AvClock> {
        let seek_serial = Arc::new(AtomicU64::new(0));
        let clock = Arc::new(AvClock::new(0.6, seek_serial));
        clock.set_playing(true);
        clock.notify_audio_active();
        clock
    }

    fn make_buffer(sample_rate: u32) -> Arc<Mutex<AudioBuffer>> {
        Arc::new(Mutex::new(AudioBuffer {
            samples: std::collections::VecDeque::new(),
            next_pts_secs: 0.0,
            sample_rate,
            samples_per_sec: sample_rate as f64 * 2.0,
            pump_seek_serial: 0,
            pdc_latency_secs: 0.0,
            pdc_latency_secs_applied: 0.0,
        }))
    }

    /// 完全 underrun (= buffer 空) で callback が来ても `next_pts_secs` が進まない。
    /// Codex review 提案 (P? runtime-check の対案、= bookkeeping を実消費に寄せた
    /// 効果の核心)。pre-fill burst で buffer 空のまま callback 連続発火しても、
    /// pts drift しないことを保証する。
    #[test]
    fn fill_output_empty_buffer_does_not_advance_pts() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let pts_before = buf.lock().unwrap().next_pts_secs;

        let mut out = [1.0_f32; 480]; // non-zero で初期化、silence 化されること確認
        fill_output(&mut out, &buf, &clock);

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

    /// 部分 drain (= want 未満の samples が buffer にある) で callback が来た場合、
    /// `next_pts_secs` は **実消費サンプル数だけ** 進む (= want 分ではない)。
    /// 旧版 (= consumed_secs = want / samples_per_sec) と新版 (= real_consumed /
    /// samples_per_sec) の動作差を直接確認する。
    #[test]
    fn fill_output_partial_drain_advances_only_real_consumed() {
        let buf = make_buffer(48_000);
        let clock = make_clock();

        // want = 480 samples、buffer に 100 samples だけ入れる → 部分 drain
        {
            let mut b = buf.lock().unwrap();
            for _ in 0..100 {
                b.samples.push_back(0.5);
            }
        }
        let pts_before = buf.lock().unwrap().next_pts_secs;

        let mut out = [0.0_f32; 480];
        fill_output(&mut out, &buf, &clock);

        let pts_after = buf.lock().unwrap().next_pts_secs;
        let expected_advance = 100.0 / (48_000.0 * 2.0);
        assert!(
            (pts_after - pts_before - expected_advance).abs() < 1e-12,
            "partial drain: pts must advance by real_consumed/samples_per_sec only \
             (expected {expected_advance:.9}s, got {:.9}s)",
            pts_after - pts_before
        );

        // 出力: 最初 100 samples は 0.5 * 0.6 (volume) = 0.3、残り 380 は silence
        for (i, &v) in out.iter().take(100).enumerate() {
            assert!((v - 0.3).abs() < 1e-6, "sample {i} should be 0.3, got {v}");
        }
        for (i, &v) in out.iter().skip(100).enumerate() {
            assert_eq!(v, 0.0, "sample {} (idx {}) should be silence", i + 100, i + 100);
        }
    }

    /// 完全 drain (= buffer に want 以上 samples) では旧版と同じ進行量。
    /// 通常再生中の挙動が refactor で変わっていないことを保証する回帰テスト。
    #[test]
    fn fill_output_full_drain_advances_full_amount() {
        let buf = make_buffer(48_000);
        let clock = make_clock();

        {
            let mut b = buf.lock().unwrap();
            for _ in 0..480 {
                b.samples.push_back(0.5);
            }
        }
        let pts_before = buf.lock().unwrap().next_pts_secs;

        let mut out = [0.0_f32; 480];
        fill_output(&mut out, &buf, &clock);

        let pts_after = buf.lock().unwrap().next_pts_secs;
        let expected_advance = 480.0 / (48_000.0 * 2.0);
        assert!(
            (pts_after - pts_before - expected_advance).abs() < 1e-12,
            "full drain: pts advances by want/samples_per_sec (expected {expected_advance:.9}s, got {:.9}s)",
            pts_after - pts_before
        );
    }
}
