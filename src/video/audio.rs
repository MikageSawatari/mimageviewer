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
/// 順序通りに行い、別動画への切替時に前動画の音声が残らないようにする (Codex 指摘)。
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
pub fn start(
    audio_rx: Receiver<AudioFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<crate::video::engine::EngineEvent>,
    engine_state: Arc<std::sync::atomic::AtomicU8>,
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
    }));

    let cancel = Arc::new(AtomicBool::new(false));
    let (shutdown_tx, shutdown_rx) = bounded::<()>(1);

    // ── pump thread: audio_rx → buffer ──
    let pump_buffer = buffer.clone();
    let pump_cancel = cancel.clone();
    let pump_clock = clock.clone();
    let pump_engine_event_tx = engine_event_tx.clone();
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
fn publish_buffer_secs(buf: &AudioBuffer, clock: &AvClock) {
    let secs = buf.samples.len() as f64 / buf.samples_per_sec;
    clock.set_audio_pump_buf_secs(secs);
}

fn run_pump(
    rx: Receiver<AudioFrame>,
    shutdown_rx: crossbeam_channel::Receiver<()>,
    buffer: Arc<Mutex<AudioBuffer>>,
    cancel: Arc<AtomicBool>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<crate::video::engine::EngineEvent>,
) {
    // 厚さ ~1.5 秒のバッファを目安に流量制御する。
    // 0.5 秒だと cpal RT 周期と decoder 不安定さで underrun → ブチブチに。
    // 1.5 秒でも遅延体感は問題なく、シーク時にバッファ捨てるので問題なし。
    // ※ samples は interleaved stereo (channels=2) なので sample_rate * 2 * 1.5。
    // sample_rate は構築時に固定なので 1 度だけロックして拾う。
    let cap_samples = {
        let b = buffer.lock().unwrap();
        (b.sample_rate as usize * 2 * 3) / 2
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

        // バッファ満杯なら一旦待つ。Condvar 化は将来の最適化。
        loop {
            let len = buffer.lock().unwrap().samples.len();
            if len < cap_samples || cancel.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // 古い seek_serial の破棄 + push を 1 ロックで実行。
        // clock.current_seek_serial() より古いフレームは、audio_tx に
        // 積まれていた pre-seek の遅延フレームなので捨てる (新世代に追い付くため)。
        let clock_serial = clock.current_seek_serial();
        let mut buf = buffer.lock().unwrap();
        if frame.seek_serial < buf.pump_seek_serial || frame.seek_serial < clock_serial {
            continue;
        }
        // audio master 化は **stale 破棄を通過したフレーム** で実施 (Codex 指摘)。
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
        buf.samples.extend(frame.samples.iter().copied());
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
    crate::logger::log("audio-pump terminated");
}

fn fill_output(
    out: &mut [f32],
    buffer: &Arc<Mutex<AudioBuffer>>,
    clock: &Arc<AvClock>,
    engine_state: &Arc<std::sync::atomic::AtomicU8>,
) {
    // 先に clock の seek_serial を読む。post-seek 直後で pump がまだ古い世代の
    // バッファを抱えている場合は、ここで全消去して silence を出す
    // (Codex 指摘: 古い samples を再生して set_audio_pts でクロックを巻き戻す経路を塞ぐ)。
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

    // 一時停止中は無音 (PTS も進めない)
    if !clock.is_playing() {
        out.fill(0.0);
        return;
    }

    // ── Warmup gate (Phase 9.B、2026-04-30 追加) ──
    //
    // 動画 open / シーク直後、engine state が `Buffering` (= まだ readiness latch が
    // 揃っていない期間) の間は **音声出力を silence にして PTS を進めない**。
    // これにより future_frames のプリフィルと UI 表示 pace の同期が取れる:
    //
    // - 旧挙動: notify_seek_completed が anchor を Audio source で書く → cpal callback
    //   で set_audio_pts が anchor.pts を 1x wall で進める → UI tick が
    //   "現在 pace_now" に追いついた frames を **複数まとめて** display 経路に流す
    //   → GPU 早期 return 経路で dropped_past が発生 (= perf overlay の赤縦線)。
    // - 新挙動: Buffering 中は cpal を silence にして anchor.pts を凍結 → UI が
    //   常に target 位置の frame だけを display → engine が Playing に遷移したら
    //   anchor を進め始める → 1 frame/tick で滑らかに消費。
    //
    // Buffering の典型的な持続時間は 100-200ms (= audio buffer が READY_THRESHOLD
    // に到達するまで)。この間ユーザーは brief な silence を聞くが、シーク直後の
    // 体感としては自然 (= 音声デバイスのレイテンシと同程度)。
    let engine_playing = engine_state.load(std::sync::atomic::Ordering::Acquire)
        == crate::video::engine::actor::state_code::PLAYING;
    if !engine_playing {
        out.fill(0.0);
        // bookkeeping は通常通り実施 (= pump からの publish との race を避ける)。
        publish_buffer_secs(&buf, clock);
        return;
    }

    let vol = clock.effective_volume();

    let mut written = 0;
    while written < want {
        match buf.samples.pop_front() {
            Some(s) => {
                out[written] = s * vol;
                written += 1;
            }
            None => {
                // underrun: 残りを silence で埋める
                for o in &mut out[written..] {
                    *o = 0.0;
                }
                written = want;
            }
        }
    }

    // pump が pre-seek サンプルを抱えている間 (clock_serial > pump_serial) に
    // set_audio_pts を呼ぶとシーク前位置でクロックが進み、シークバーがスナップバック。
    // 二段読みで race を排除する。
    let consumed_secs = want as f64 / buf.samples_per_sec;
    buf.next_pts_secs += consumed_secs;
    let pts_now = buf.next_pts_secs;
    let pump_serial = buf.pump_seek_serial;
    // publish は Mutex 内で行う。Mutex 外に出すと run_pump の push 後 publish が
    // stale な fill_output 側 publish に上書きされる race がある (Codex 指摘)。
    // overhead = 1 atomic store ≈ 1 ns 程度なので RT への影響は無視できる。
    publish_buffer_secs(&buf, clock);
    drop(buf);
    if pump_serial >= clock.current_seek_serial() {
        clock.set_audio_pts(pts_now);
        // override クリアは「target 近傍のサンプル」を消費した時のみ。
        // target から大きく離れた場合は decoder 側 seek が外れて古い位置の audio が
        // 新世代 serial で来ているケースで、ここで clear するとシークバーが
        // 元位置に戻る (override 中は now_secs が target を返すので diff で判定可能、
        // 通常再生中は now_secs ≈ pts_now なので diff ~0 で常に clear する)。
        let now = clock.now_secs();
        if (pts_now - now).abs() <= SEEK_TARGET_TOLERANCE_SECS {
            clock.clear_seek_target_override(pump_serial);
        }
    }
}
