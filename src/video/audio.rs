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
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};

use super::audio_diagnostics::AudioDiagnostics;
use super::audio_stretch::TimeStretcher;
use super::clock::{AvClock, SEEK_TARGET_TOLERANCE_SECS};
use super::decoder::AudioFrame;
use super::engine::actor::state_code;

const MAX_STALE_AUDIO_DRAIN_PER_TICK: usize = 256;

/// 音声出力ストリーム。drop すると `pause` + Stream drop + pump スレッド join を
/// 順序通りに行い、別動画への切替時に前動画の音声が残らないようにする。
pub struct AudioOutput {
    /// cpal Stream は !Send。Option にして drop 時に明示的に落とす。
    stream: Option<cpal::Stream>,
    /// pump / cpal callback が共有する audio buffer。fast video switch では旧ソースの
    /// processed/raw 音声を先に捨て、ハードウェアへ流れる残りを最小化する。
    buffer: Arc<Mutex<AudioBuffer>>,
    /// pump thread の停止フラグ。
    cancel: Arc<AtomicBool>,
    /// pump スレッド起床用 (recv_timeout より速く抜けるため)。
    shutdown_tx: Sender<()>,
    /// pump スレッドハンドル。drop で join する。
    pump: Option<std::thread::JoinHandle<()>>,
    pub sample_rate: u32,
    /// A/V sync drift 計装用 atomic bundle。`clear_buffer` で `audio_out.buffer_clear` を
    /// emit するために保持する (callback / pump へは spawn 時に Arc clone を渡す)。
    diagnostics: Arc<AudioDiagnostics>,
    /// mIV Remote の streaming session が audio tap を着脱するための制御口。
    /// producer は常に audio-pump であり、session は [`AudioTapLease`] を所有する。
    #[allow(dead_code)] // 増分 5 で streaming session から接続する。
    audio_tap: AudioTapController,
}

impl AudioOutput {
    #[cfg(test)]
    pub(crate) fn connected_without_output_for_test(sample_rate: u32) -> Self {
        let buffer = Arc::new(Mutex::new(AudioBuffer {
            processed: std::collections::VecDeque::new(),
            drain_offset_in_first: 0,
            raw_pending: std::collections::VecDeque::new(),
            next_pts_secs: 0.0,
            sample_rate,
            samples_per_sec: sample_rate as f64 * 2.0,
            pump_seek_serial: 0,
            last_fill_stale_clear_logged_serial: 0,
            pdc_latency_secs: 0.0,
            pdc_latency_secs_applied: 0.0,
        }));
        let (shutdown_tx, shutdown_rx) = bounded(1);
        std::mem::forget(shutdown_rx);
        let (command_tx, command_rx) = unbounded();
        std::mem::forget(command_rx);
        Self {
            stream: None,
            buffer,
            cancel: Arc::new(AtomicBool::new(false)),
            shutdown_tx,
            pump: None,
            sample_rate,
            diagnostics: Arc::new(AudioDiagnostics::new(std::time::Instant::now())),
            audio_tap: AudioTapController {
                command_tx,
                next_owner_id: Arc::new(AtomicU64::new(1)),
            },
        }
    }

    pub fn pause_stream(&self) {
        if let Some(stream) = self.stream.as_ref() {
            let _ = stream.pause();
        }
    }

    pub fn clear_buffer(&self, clock: &AvClock) {
        // ⚠️ Codex 4 巡目 P1 ① 反映: clear と zero の **前** にすべての snapshot 値を
        // copy-out する。`audio_tx_queued_before` を `clock.zero_audio_tx_queued_secs()`
        // の後に読むと必ず 0 になってしまう (= 初回擬似コードのバグ)。同様に
        // `now_secs_at_clear` も `next_pts_secs = clock.now_secs()` 代入時の値を使いたい
        // ので、lock の中で 1 回だけ読んでおく。
        //
        // ⚠️ Codex 3 巡目 P1 ③ 反映: lock 中は値 copy のみ、`perf::event` は MutexGuard
        // drop 後に呼ぶ (= cpal callback ブロック防止)。
        let mut snapshot_for_log: Option<(f64, f64, f64, f64)> = None;
        if let Ok(mut buf) = self.buffer.lock() {
            if crate::perf::is_enabled() {
                let processed_secs: f64 =
                    buf.processed.iter().map(|c| c.duration_secs).sum::<f64>()
                        + remaining_first_chunk_secs(&buf);
                let raw_pending_secs: f64 =
                    buf.raw_pending.iter().map(|f| f.duration_secs).sum::<f64>();
                let audio_tx_queued_before = clock.audio_tx_queued_secs();
                let now_secs_at_clear = clock.now_secs();
                snapshot_for_log = Some((
                    processed_secs,
                    raw_pending_secs,
                    audio_tx_queued_before,
                    now_secs_at_clear,
                ));
            }
            buf.processed.clear();
            buf.raw_pending.clear();
            buf.drain_offset_in_first = 0;
            buf.next_pts_secs = clock.now_secs();
            publish_buffer_secs(&buf, clock);
        }
        // ← MutexGuard drop
        clock.zero_audio_tx_queued_secs();
        // Codex P2 ① 反映: clear で audio buffer が空になった瞬間、`audio_audible_pts` の
        // 旧値を残したままにしない。次の present で旧 audio_pts と新 video_pts を比較して
        // 偽の巨大 av_offset が出るのを防ぐ。
        // 次の audio callback が `set_audio_pts` を呼ぶまでは av_offset は NaN
        // (= overlay / analyzer は体感 A/V offset 未確定として扱う)。
        self.diagnostics.clear_audio_position();
        if let Some((processed, raw_pending, tx_queued, now_at_clear)) = snapshot_for_log {
            crate::perf::event(
                "audio_out",
                "buffer_clear",
                None,
                0,
                &[
                    ("processed_secs_before", serde_json::Value::from(processed)),
                    (
                        "raw_pending_secs_before",
                        serde_json::Value::from(raw_pending),
                    ),
                    ("audio_tx_queued_before", serde_json::Value::from(tx_queued)),
                    ("now_secs_at_clear", serde_json::Value::from(now_at_clear)),
                ],
            );
        }
    }

    /// audio-pump の post-processing tap を着脱するための controller を返す。
    ///
    /// controller 自体は音声を所有しない。streaming session が `attach` の戻り値である
    /// [`AudioTapLease`] と receiver を所有し、lease drop で同じ owner だけを detach する。
    #[allow(dead_code)] // 増分 5 で streaming session から接続する。
    pub(crate) fn audio_tap_controller(&self) -> AudioTapController {
        self.audio_tap.clone()
    }
}

pub fn warm_up_default_output_device() {
    let _ = std::thread::Builder::new()
        .name("cpal-warmup".into())
        .spawn(|| {
            let started = std::time::Instant::now();
            let host = cpal::default_host();
            let Some(device) = host.default_output_device() else {
                crate::logger::log("[startup] cpal warm-up skipped: no output device".to_string());
                return;
            };
            let Ok(supported) = device.default_output_config() else {
                crate::logger::log(
                    "[startup] cpal warm-up skipped: default output config unavailable".to_string(),
                );
                return;
            };
            let config = supported.config();
            let Ok(stream) = device.build_output_stream(
                &config,
                |out: &mut [f32], _: &cpal::OutputCallbackInfo| out.fill(0.0),
                |err| crate::logger::log(format!("cpal warm-up stream error: {err}")),
                None,
            ) else {
                crate::logger::log(
                    "[startup] cpal warm-up skipped: stream build failed".to_string(),
                );
                return;
            };
            if let Err(err) = stream.play() {
                crate::logger::log(format!(
                    "[startup] cpal warm-up skipped: stream.play: {err}"
                ));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = stream.pause();
            crate::logger::log(format!(
                "[startup] cpal warm-up done ms={:.1}",
                started.elapsed().as_secs_f64() * 1000.0
            ));
        });
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
        // 3. pump を join。通常は cancel + shutdown signal で 100ms 以内に終了するが、
        //    decoder/engine 側の back-pressure デッドロック等で pump が動けないと join が
        //    無限ブロックして UI thread を固める (2026-05-15、Escape 後の "応答なし"
        //    14秒の正体)。`NativeVideoOutput::drop` と同じく **専用 thread で join** に
        //    付け替え、Drop は即時返す。万一 pump が exit しなくても thread は単に
        //    残るだけで UI には影響しない。
        if let Some(p) = self.pump.take() {
            let _ = std::thread::Builder::new()
                .name("audio-output-drop-join".to_string())
                .spawn(move || {
                    let _ = p.join();
                });
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
#[derive(Clone, Debug)]
pub(crate) struct ProcessedChunk {
    /// post-VST interleaved stereo f32 サンプル。
    pub(crate) samples: Vec<f32>,
    /// **audible PTS** = この chunk の最初のサンプルが「実際にスピーカーから聞こえる」
    /// PTS (秒)。`input_pts - pdc_latency_at_process` で計算。video clock 同期に使う。
    /// mIV Remote の AAC input PTS にもこの source timeline 値を使う。
    pub(crate) audible_pts_secs: f64,
    /// chunk の音声時間 (秒) = `samples.len() / samples_per_sec`。
    /// BufferReady 判定や processed cap 比較で再計算を避けるためキャッシュ。
    pub(crate) duration_secs: f64,
    /// output/wall 秒 1 秒に対応する source timeline 秒。
    /// 2.0x なら約 2.0。fill_output はこれで PTS を進める。
    pub(crate) source_secs_per_output_sec: f64,
    /// この chunk がどの seek 世代で生成されたか (= stale 判定用)。
    pub(crate) seek_serial: u64,
    /// VST / safety limiter / stretcher を含む処理時点の合計 latency (source 秒)。
    /// 後続 chunk と差があれば video clock jump で吸収
    /// (旧 `pdc_latency_secs_applied` 比較ロジックを chunk 単位に分離)。
    /// mIV Remote は tap metadata の有限性も AAC input 前に検証する。
    pub(crate) pdc_latency_secs_at_process: f64,
}

#[derive(Clone)]
pub(crate) struct AudioTapController {
    command_tx: Sender<AudioTapCommand>,
    next_owner_id: Arc<AtomicU64>,
}

/// streaming session が所有する audio tap attachment。
/// owner id により、古い session の Drop が新しい session の tap を外すことはない。
pub(crate) struct AudioTapLease {
    owner_id: u64,
    command_tx: Sender<AudioTapCommand>,
    #[allow(dead_code)] // Increment 6 VideoStreamState will expose tap backpressure telemetry.
    dropped: Arc<AtomicU64>,
}

enum AudioTapCommand {
    Attach(ActiveAudioTap),
    Detach(u64),
}

struct ActiveAudioTap {
    owner_id: u64,
    payload_tx: Sender<ProcessedChunk>,
    dropped: Arc<AtomicU64>,
}

impl AudioTapController {
    pub(crate) fn attach(
        &self,
        capacity: usize,
    ) -> Result<(AudioTapLease, Receiver<ProcessedChunk>), &'static str> {
        if capacity == 0 {
            return Err("audio tap capacity must be non-zero");
        }
        let owner_id = self.next_owner_id.fetch_add(1, Ordering::Relaxed);
        let (payload_tx, payload_rx) = bounded(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        self.command_tx
            .send(AudioTapCommand::Attach(ActiveAudioTap {
                owner_id,
                payload_tx,
                dropped: Arc::clone(&dropped),
            }))
            .map_err(|_| "audio pump is no longer running")?;
        Ok((
            AudioTapLease {
                owner_id,
                command_tx: self.command_tx.clone(),
                dropped,
            },
            payload_rx,
        ))
    }
}

impl AudioTapLease {
    #[allow(dead_code)] // Increment 6 VideoStreamState will expose tap backpressure telemetry.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for AudioTapLease {
    fn drop(&mut self) {
        let _ = self
            .command_tx
            .try_send(AudioTapCommand::Detach(self.owner_id));
    }
}

/// 唯一の production tap 点から呼ぶ。command と payload のどちらも non-blocking。
/// 未接続時は chunk を変更せず allocation もしない。接続時だけ、PC 再生 queue と
/// streaming worker の独立所有に必要な payload Vec の clone を 1 回行う。
fn refresh_audio_tap(command_rx: &Receiver<AudioTapCommand>, active: &mut Option<ActiveAudioTap>) {
    while let Ok(command) = command_rx.try_recv() {
        match command {
            AudioTapCommand::Attach(tap) => *active = Some(tap),
            AudioTapCommand::Detach(owner_id)
                if active.as_ref().is_some_and(|tap| tap.owner_id == owner_id) =>
            {
                *active = None;
            }
            AudioTapCommand::Detach(_) => {}
        }
    }
}

enum PreparedAudioTapChunk {
    NotConnected,
    Full,
    Payload(ProcessedChunk),
}

fn prepare_audio_tap_chunk(
    active: &Option<ActiveAudioTap>,
    chunk: &ProcessedChunk,
) -> PreparedAudioTapChunk {
    let Some(tap) = active.as_ref() else {
        return PreparedAudioTapChunk::NotConnected;
    };
    if tap.payload_tx.is_full() {
        return PreparedAudioTapChunk::Full;
    }
    PreparedAudioTapChunk::Payload(chunk.clone())
}

fn publish_prepared_audio_tap_chunk(
    active: &mut Option<ActiveAudioTap>,
    prepared: PreparedAudioTapChunk,
) {
    let Some(tap) = active.as_ref() else {
        return;
    };
    let payload = match prepared {
        PreparedAudioTapChunk::NotConnected => return,
        PreparedAudioTapChunk::Full => {
            tap.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        PreparedAudioTapChunk::Payload(payload) => payload,
    };
    match tap.payload_tx.try_send(payload) {
        Ok(()) => {}
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            tap.dropped.fetch_add(1, Ordering::Relaxed);
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
            tap.dropped.fetch_add(1, Ordering::Relaxed);
            *active = None;
        }
    }
}

/// 共有 ring buffer (Mutex 保護)。**raw / processed 2 段構造** (Codex 助言 2026-05)。
///
/// - `raw_pending`: pump が積む pre-VST AudioFrame queue。数秒を超えたら
///   audio_rx intake を止め、bounded channel 経由で demux へ back-pressure を返す。
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
    /// cap = `RAW_BACKPRESSURE_SECS` 相当。超過時は pump が audio_rx intake を一時停止し、
    /// bounded channel 経由で demux に back-pressure を返す。
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
    /// `fill_output` の seek stale clear ログを seek 世代ごとに 1 回に抑えるための記録。
    last_fill_stale_clear_logged_serial: u64,
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

#[derive(Debug, Default)]
struct StaleAudioDrainResult {
    dropped: usize,
    deferred_serial: Option<u64>,
    hit_limit: bool,
    disconnected: bool,
    remaining_rx_len: usize,
}

fn drain_stale_audio_rx(
    rx: &Receiver<AudioFrame>,
    live_serial: u64,
    deferred_frame: &mut Option<AudioFrame>,
) -> StaleAudioDrainResult {
    let mut result = StaleAudioDrainResult::default();
    while result.dropped < MAX_STALE_AUDIO_DRAIN_PER_TICK {
        match rx.try_recv() {
            Ok(frame) if frame.seek_serial < live_serial => {
                result.dropped += 1;
            }
            Ok(frame) => {
                result.deferred_serial = Some(frame.seek_serial);
                *deferred_frame = Some(frame);
                break;
            }
            Err(crossbeam_channel::TryRecvError::Empty) => break,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                result.disconnected = true;
                break;
            }
        }
    }
    result.hit_limit = result.dropped >= MAX_STALE_AUDIO_DRAIN_PER_TICK
        && deferred_frame.is_none()
        && !result.disconnected;
    result.remaining_rx_len = rx.len();
    result
}

const SAFETY_LIMITER_LOOKAHEAD_SECS: f64 = 0.005;
const SAFETY_LIMITER_RELEASE_SECS: f64 = 0.100;
/// セーフティリミッターが信号を抑え込む上限。0 dBFS = フルスケール = 波形表現の上限
/// そのもの。これを超えた分だけゲインを下げて hard clip を防ぐ。視聴用の保護なので
/// true peak ヘッドルーム (-1 dBTP 等) は確保しない — それは制作・配信側のガイドライン
/// であって、再生プレイヤーの義務ではない。
const SAFETY_LIMITER_CEILING_DBFS: f32 = 0.0;
/// ピークランプ点灯のしきい値 (ゲインリダクション量)。リミッターが「ceiling に
/// 触れた瞬間」ではなく、ゲインリダクションがこの dB 以上に達したブロックでだけ
/// 点灯させる。タイムストレッチ由来の 1 dB 未満の微小オーバーや f32 演算誤差では
/// 点かず、VST / 音量 boost / normalize boost で実際に gain staging が破綻して
/// 可聴な抑え込みが起きたときだけ点く。
const SAFETY_LIMITER_INDICATOR_GR_DB: f32 = 1.0;
/// normalize gain の変更を dB 空間でならす時間。仮 gain → 確定 gain の段差を隠す。
const NORMALIZE_GAIN_RAMP_SECS: f64 = 4.0;

/// VST3 チェーン後段と 0dB 超の手動音量 boost の保険用 lookahead limiter。
///
/// ユーザーがチェーン末尾に limiter を入れていない場合でも、過大出力が WASAPI /
/// OS mixer 側で hard clip するのを避けるための最終安全網。制作向け limiter ではなく
/// 視聴用の保護なので、パラメータは固定し、VST3 チェーンまたは手動 boost が active の
/// ときだけ動かす。
struct SafetyLimiter {
    channels: usize,
    lookahead_frames: usize,
    delay: Vec<f32>,
    delayed_frame: Vec<f32>,
    write_frame: usize,
    gain: f32,
    ceiling: f32,
    /// ピークランプ点灯のしきい値 (線形ゲイン)。ブロック内で要求ゲイン
    /// (`target_gain`) がこの値以下になったら `process_block` が true を返す。
    /// `10^(-SAFETY_LIMITER_INDICATOR_GR_DB / 20)`。
    indicator_gain_threshold: f32,
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
        let indicator_gain_threshold = 10.0_f32.powf(-SAFETY_LIMITER_INDICATOR_GR_DB / 20.0);
        Self {
            channels,
            lookahead_frames,
            delay: vec![0.0; lookahead_frames * channels],
            delayed_frame: vec![0.0; channels],
            write_frame: 0,
            gain: 1.0,
            ceiling,
            indicator_gain_threshold,
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

    /// ブロックを処理する。**ゲインリダクション量が `SAFETY_LIMITER_INDICATOR_GR_DB`
    /// dB 以上に達した** (= 要求ゲインが `indicator_gain_threshold` 以下になった)
    /// 場合に true を返す。これはピークランプを点灯すべきかの判定で、リミッター本体は
    /// ceiling を超えた分を常に抑え込む — ランプだけがこのしきい値を持つ。
    ///
    /// 判定は音量フェーダー (出力ゲイン) に依存しない。リミッターはフェーダー前段で
    /// 内部信号に作用するので、戻り値は「内部チェーンが 0 dBFS をどれだけ超えたか」を
    /// そのまま表す。
    fn process_block(&mut self, samples: &mut [f32]) -> bool {
        if samples.is_empty() || self.channels == 0 {
            return false;
        }
        debug_assert_eq!(samples.len() % self.channels, 0);

        let mut min_target_gain = 1.0_f32;
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
            min_target_gain = min_target_gain.min(target_gain);
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
        min_target_gain <= self.indicator_gain_threshold
    }
}

/// preroll (測定前待機) が解除された最初のブロックか判定する (前ブロック suspended かつ
/// 今ブロック released)。true のブロックで normalize gain を snap し、確定 gain で即再生
/// 開始することで 4 秒 ramp (`NORMALIZE_GAIN_RAMP_SECS`) を回避する。中盤再生中の gain 変更
/// (provisional→final refine や曲中 Norm ON/OFF) は preroll を経由しないので従来どおり ramp する。
fn preroll_release_edge(prev_suspended: bool, now_suspended: bool) -> bool {
    prev_suspended && !now_suspended
}

/// audio-pump 内だけで持つ normalize gain smoother。
///
/// `AvClock` は目標 gain だけを atomic publish し、pump が block を処理する時点で
/// 目標変更を検出して dB 空間で ramp する。`audio_preroll_suspended` 中は
/// `snap_to_target` しておくことで、測定前待機解除後の最初の音は仮 gain で始まる。
struct NormalizeGainRamp {
    channels: usize,
    ramp_frames: usize,
    current_db: f32,
    target_db: f32,
    step_db_per_frame: f32,
    remaining_frames: usize,
}

impl NormalizeGainRamp {
    fn new(sample_rate: u32, channels: usize) -> Self {
        let ramp_frames = ((sample_rate as f64 * NORMALIZE_GAIN_RAMP_SECS).round() as usize).max(1);
        Self {
            channels: channels.max(1),
            ramp_frames,
            current_db: 0.0,
            target_db: 0.0,
            step_db_per_frame: 0.0,
            remaining_frames: 0,
        }
    }

    fn snap_to_target(&mut self, target_linear: f32) {
        let db = normalize_linear_to_db(target_linear);
        self.current_db = db;
        self.target_db = db;
        self.step_db_per_frame = 0.0;
        self.remaining_frames = 0;
    }

    /// `samples` に現在の ramp gain を掛け、block 内の最大 normalize gain を返す。
    fn apply_to_samples(&mut self, samples: &mut [f32], target_linear: f32) -> f32 {
        let requested_db = normalize_linear_to_db(target_linear);
        if (requested_db - self.target_db).abs() > 0.001 {
            self.target_db = requested_db;
            self.remaining_frames = self.ramp_frames;
            self.step_db_per_frame =
                (self.target_db - self.current_db) / self.remaining_frames as f32;
        }

        if samples.is_empty() {
            return normalize_db_to_linear(self.current_db);
        }

        let frames = samples.len() / self.channels;
        let mut max_gain = 0.0_f32;
        if self.remaining_frames == 0 {
            self.current_db = self.target_db;
            let gain = normalize_db_to_linear(self.current_db);
            max_gain = gain;
            if (gain - 1.0).abs() > f32::EPSILON {
                for s in samples {
                    *s *= gain;
                }
            }
            return max_gain;
        }

        for frame in 0..frames {
            if self.remaining_frames > 0 {
                self.current_db += self.step_db_per_frame;
                self.remaining_frames -= 1;
                if self.remaining_frames == 0 {
                    self.current_db = self.target_db;
                    self.step_db_per_frame = 0.0;
                }
            }
            let gain = normalize_db_to_linear(self.current_db);
            max_gain = max_gain.max(gain);
            if (gain - 1.0).abs() > f32::EPSILON {
                let base = frame * self.channels;
                for ch in 0..self.channels {
                    if let Some(s) = samples.get_mut(base + ch) {
                        *s *= gain;
                    }
                }
            }
        }

        // 万一 samples.len() が channels の倍数でない場合の defensive tail。
        let tail_start = frames * self.channels;
        if tail_start < samples.len() {
            let gain = normalize_db_to_linear(self.current_db);
            max_gain = max_gain.max(gain);
            for s in &mut samples[tail_start..] {
                *s *= gain;
            }
        }
        max_gain
    }

    #[cfg(test)]
    fn remaining_frames(&self) -> usize {
        self.remaining_frames
    }
}

fn normalize_linear_to_db(gain: f32) -> f32 {
    let clamped = if gain.is_finite() && gain > 0.0 {
        gain
    } else {
        1.0
    };
    20.0 * clamped.log10()
}

fn normalize_db_to_linear(db: f32) -> f32 {
    if !db.is_finite() {
        return 1.0;
    }
    10.0_f32.powf(db / 20.0)
}

/// output/wall 秒で測った DSP latency を source timeline 秒へ写し、先頭 sample の
/// audible PTS を返す。time stretch、VST PDC、safety limiter の latency はすべて
/// output 側で加算されるため、変速率を掛けてから source PTS から引く。
fn audible_pts_after_latency(
    input_pts_secs: f64,
    latency_output_secs: f64,
    source_secs_per_output_sec: f64,
) -> (f64, f64) {
    let latency_source_secs = latency_output_secs * source_secs_per_output_sec;
    (
        (input_pts_secs - latency_source_secs).max(0.0),
        latency_source_secs,
    )
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
    diagnostics: Arc<AudioDiagnostics>,
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
    let device_default_channels = supported.channels();

    // Stereo packed f32 で固定 (decoder 側 swresample に合わせる)。
    // WASAPI Shared モードの auto-mix で 5.1 / 7.1 デバイスでも 2 ch 出力できる
    // ケースが多いが、デバイス / driver 設定によっては失敗する (T19, Claude R3-1
    // 2026-05-16)。デフォルトが mono の特殊デバイスでも build に失敗しうる。
    //
    // 失敗時のエラー文言にデバイス既定 channel 数を含めて、ユーザーが Windows の
    // 「サウンドの設定」から既定形式を 2ch ステレオに切り替えれば直ることを示す。
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
        last_fill_stale_clear_logged_serial: 0,
        pdc_latency_secs: 0.0,
        pdc_latency_secs_applied: 0.0,
    }));

    let cancel = Arc::new(AtomicBool::new(false));
    let (shutdown_tx, shutdown_rx) = bounded::<()>(1);
    let (audio_tap_command_tx, audio_tap_command_rx) = unbounded();
    let audio_tap = AudioTapController {
        command_tx: audio_tap_command_tx,
        next_owner_id: Arc::new(AtomicU64::new(1)),
    };

    // ── pump thread: audio_rx → raw_pending → (VST process) → processed → buffer ──
    let pump_buffer = buffer.clone();
    let pump_cancel = cancel.clone();
    let pump_clock = clock.clone();
    let pump_engine_event_tx = engine_event_tx.clone();
    let pump_engine_state = engine_state.clone();
    let pump_diagnostics = Arc::clone(&diagnostics);
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
                pump_diagnostics,
                audio_tap_command_rx,
                #[cfg(windows)]
                pump_dsp_bridge,
            );
        })
        .map_err(|e| format!("spawn audio-pump: {e}"))?;

    // ── cpal output callback: buffer → device ──
    let cb_buffer = buffer.clone();
    let cb_clock = clock.clone();
    let cb_engine_state = engine_state.clone();
    let cb_diagnostics = Arc::clone(&diagnostics);
    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                fill_output(
                    out,
                    &cb_buffer,
                    &cb_clock,
                    &cb_engine_state,
                    &cb_diagnostics,
                );
            },
            |err| crate::logger::log(format!("cpal output stream error: {err}")),
            None,
        )
        .map_err(|e| {
            format!(
                "build_output_stream: {e} \
                 (デバイスの既定形式: {device_default_channels}ch、mIV は 2ch ステレオで出力します。\
                 失敗する場合は Windows サウンド設定 → 既定のデバイス → プロパティ → 詳細 で\
                 「2ch 16/24 bit ステレオ」を選択してください)"
            )
        })?;

    stream.play().map_err(|e| format!("stream.play: {e}"))?;

    Ok(AudioOutput {
        stream: Some(stream),
        buffer,
        cancel,
        shutdown_tx,
        pump: Some(pump_handle),
        sample_rate,
        diagnostics,
        audio_tap,
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
/// 3. raw_pending に push。raw が数秒を超えたら audio_rx intake を一時停止する。
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
    diagnostics: Arc<AudioDiagnostics>,
    audio_tap_command_rx: Receiver<AudioTapCommand>,
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
    // ── raw_pending back-pressure / warning ──
    // 以前は 30 秒を超えたら最新フレームで再 anchor していたが、video packet overflow
    // queue 導入後は demux が audio を大きく先読みできるため、通常の seek/open でも
    // 30 秒 overflow に到達し、曲頭を捨てる副作用が出た。現在は raw が数秒を超えたら
    // audio_rx intake を止め、audio_tx/audio_pkt_tx の bounded queue を通じて demux に
    // 自然な back-pressure を返す。音声は捨てず、先頭から順に消化する。
    const RAW_WARNING_SECS: f64 = 15.0;
    const RAW_BACKPRESSURE_PLAYING_SECS: f64 = 5.0;
    const RAW_BACKPRESSURE_PREROLL_SECS: f64 = 5.0;
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
    let mut time_stretcher = TimeStretcher::new(sample_rate);
    let mut normalize_gain_ramp = NormalizeGainRamp::new(sample_rate, 2);
    // preroll 解除エッジ検出。preroll 中 (測定前待機) は毎ブロック snap_to_target するが、
    // UI スレッドは「set_normalize_gain(確定 gain) → set_audio_preroll_suspended(false)」を
    // 連続で store するため、pump が preroll=true のまま新 gain を snap するブロックを
    // 挟めないことがある (race)。すると解除後の最初の可聴ブロックで旧 gain (=1.0) から
    // 確定 gain へ 4 秒 ramp してしまい、同期スキャンしたのに音量が徐々に上がる。
    // 対策 = 解除エッジ (true→false) の最初のブロックで確実に snap し確定 gain で即開始する。
    // 初期値は実 preroll 状態を Acquire load で取り込む (Codex P2: 超高速 scan で pump が
    // loop 内で preroll=true を一度も観測しなくても edge snap を保証する)。preroll を先に
    // 読んでから gain を snap する。preroll=false 観測時は直前に Release された確定 gain も
    // 必ず可視 (clock.rs の set_normalize_gain / set_audio_preroll_suspended は共に Release)。
    let mut was_preroll_suspended = clock.audio_preroll_suspended();
    normalize_gain_ramp.snap_to_target(clock.normalize_gain() as f32);

    let mut activated = false;
    let mut active_audio_tap = None;
    let mut last_seen_seek_serial: u64 = 0;
    // T07 (v0.9.0): VST3 bridge wedge auto-disable のための連続失敗カウンタ。
    // `process_block` が連続 N 回 (= 約 N * block_duration ms の停滞) 失敗したら
    // `disable_with_reason` でセッション中の VST3 を切る。閾値は Codex 助言 (= ~250ms
    // 相当の連続失敗) を反映して 3 回とする。
    //
    // **Counter reset の hysteresis** (Codex P1 round 2 反映): 単に「1 回成功でリセット」
    // すると、partial pipe desync 状態で偶然 Ok を返す block が間に挟まったときに
    // counter がゼロに戻ってしまい、auto-disable に至らない。`HEALTHY_RESET` 回連続で
    // 成功して初めて counter をリセットする。
    #[cfg(windows)]
    const VST3_CONSECUTIVE_FAILURE_DISABLE: u32 = 3;
    #[cfg(windows)]
    const VST3_HEALTHY_RESET: u32 = 5;
    #[cfg(windows)]
    let mut vst3_consecutive_failures: u32 = 0;
    #[cfg(windows)]
    let mut vst3_consecutive_successes: u32 = 0;
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
    // ここでの seek_target は **PDC pre-target silence 判定用**であり、
    // 実際に emit された最初の audio frame PTS を使う。seek 後の audio decode は
    // target まで trim 済みなので通常は target 近傍になる。
    // BufferReady audio_anchor 用の **user-requested target** とは別管理
    // (= `pump_anchor_target_secs` 参照)。
    let mut seek_target_secs: Option<f64> = None;
    // **Codex P1 (2026-05-01)**: BufferReady の audio_anchor pts として報告する
    // **user-requested seek target**。
    // demux Flush 経由で全 AudioFrame に焼き付けられている `frame.seek_target_secs`
    // を毎 intake で取り出してここに保存する。
    //
    // 用途: BufferReady emit 時に `audible_pts.max(pump_anchor_target_secs)` で
    // 報告する。通常は audio trim 後なので audible_pts ≈ target で max は no-op。
    // PDC > 0 などで audible_pts が target より前に見える場合も target 側を採用し、
    // Playing 入場時の clock anchor を target に維持する (= timeline 表示が target 固定)。
    //
    // 旧版 (Codex P1 修正前) は `audible_pts` のみで BufferReady を出していたため、
    // PDC などで audible が target より前に見える場合に anchor が巻き戻り、
    // notify_seek_completed(target) で立てた anchor が上書きされ得た。
    let mut pump_anchor_target_secs: Option<f64> = None;
    let mut last_warning_at: Option<std::time::Instant> = None;
    let mut last_stale_drain_log_serial: Option<u64> = None;
    let mut deferred_frame: Option<AudioFrame> = None;

    // ── A/V drift 計装: pump スレッド側の 1Hz snapshot + edge poll ──
    // RT callback (`fill_output`) は atomic 書き込みのみ。実際の `perf::event` は
    // ここから 1Hz / edge で emit する (Codex 3 巡目 P1 ① 反映、xrun 防止)。
    let mut last_diag_log_at = std::time::Instant::now();
    let mut last_silence_total_logged: u64 = 0;
    let mut last_seen_underrun_begin_seq: u64 = 0;
    let mut last_seen_underrun_end_seq: u64 = 0;
    let mut last_seen_pts_jump_seq: u64 = 0;

    while !cancel.load(Ordering::Acquire) {
        // ── frame 受信 (timeout 付き、Codex 助言): audio_rx 到着を待たず自律 refill ──
        let raw_backpressure_secs = if engine_state.load(Ordering::Acquire) == state_code::PLAYING {
            RAW_BACKPRESSURE_PLAYING_SECS
        } else {
            // The demux/decode packet queues are intentionally shallow now, so
            // Loading/Seeking/Buffering should not let audio run far ahead of
            // video. Use the same non-destructive back-pressure cap as playing.
            RAW_BACKPRESSURE_PREROLL_SECS
        };
        let pause_audio_intake = clock.audio_raw_pending_secs() >= raw_backpressure_secs;
        let frame_opt: Option<AudioFrame> = if let Some(frame) = deferred_frame.take() {
            Some(frame)
        } else if pause_audio_intake {
            crossbeam_channel::select! {
                recv(shutdown_rx) -> _ => return,
                default(std::time::Duration::from_millis(REFILL_TICK_MS)) => None,
            }
        } else {
            crossbeam_channel::select! {
                recv(shutdown_rx) -> _ => return,
                recv(rx) -> msg => match msg {
                    Ok(f) => Some(f),
                    Err(_) => break,
                },
                default(std::time::Duration::from_millis(REFILL_TICK_MS)) => None,
            }
        };

        // ── frame intake (Some の場合のみ): seek serial 検出、raw_pending push ──
        // 'intake ラベル経由で skip するときも下流の refill / publish は実行する
        // (= Codex 助言の自律 refill)。
        'intake: {
            let Some(frame) = frame_opt else {
                break 'intake;
            };

            clock.add_audio_tx_queued_secs_for_epoch(
                -frame.queued_wall_secs,
                frame.audio_tx_accounting_epoch,
            );

            // ── 新 seek 世代の検出 → VST plugin sync reset (= 既存挙動) ──
            let frame_seek_serial = frame.seek_serial;
            let cur_clock_serial = clock.current_seek_serial();
            let should_reset_plugins = frame_seek_serial >= cur_clock_serial
                && (!seen_valid_audio_frame || frame_seek_serial > last_seen_seek_serial);
            if should_reset_plugins {
                #[cfg(windows)]
                if let Some(b) = &dsp_bridge {
                    // T20 (Claude R3-3): cancel check を `reset_plugins_sync` 前に挟む。
                    // `reset_plugins_sync` は bridge ごとに最大 2 秒の timeout を持ち、複数
                    // bridge が active な状態で動画切替 → AudioOutput::drop が起きると、
                    // pump が reset 待ちで 4-8 秒進めなくなる。Drop は side-thread join に
                    // 切り替え済 (`audio-output-drop-join`) なので UI は freeze しないが、
                    // pump exit が遅れて次の動画で audio 再起動が遅延する。cancel check で
                    // shutdown 中は reset を skip して即 exit させる。
                    if !cancel.load(Ordering::Acquire)
                        && b.is_enabled()
                        && b.active_slot_count() > 0
                    {
                        b.reset_plugins_sync();
                    }
                }
                last_seen_seek_serial = frame_seek_serial;
                seen_valid_audio_frame = true;
                // 新 seek 世代: target / activate を reset
                safety_limiter.reset();
                // AV seek ではここで reset。1.0x bypass 境界の reset は
                // TimeStretcher::process 内で自動的に行う。
                time_stretcher.reset();
                // PDC trim 用 seek_target は実際に emit された最初の audio frame PTS。
                // audio decode 側は target まで trim 済みなので通常は target 近傍。
                // 詳細は seek_target_secs の宣言コメント参照。
                seek_target_secs = Some(frame.pts_secs);
                // BufferReady anchor 用 target は demux Flush 経由で焼き付けられた
                // user-requested target。
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

                buf.raw_pending.push_back(frame);
                raw_secs
            };

            // ── overflow / warning 通知 ──
            if raw_total_secs > RAW_WARNING_SECS {
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
        // pump の方が古ければ raw/processed を clear + tx_queued を 0 化 + publish 0 し、
        // audio_rx に残った stale frame も一気に drain してから refill loop を skip。
        //
        // **tx_queued も 0 化** (Codex P2、2026-05-01 改訂):
        // raw/processed だけ消しても、`total_audio_buffer_secs()` には audio_tx_queued が
        // 残る。decoder の `notify_seek_completed()` が走って add_tx_queued の差分が再構築
        // されるまでの窓で、旧世代の audio_tx 残量が pacing に「playable あり」と誤判断
        // させる可能性があった。`zero_audio_tx_queued_secs()` で即座に 0 化。
        // 旧世代の `add_tx_queued(-duration)` が後から届いても `max(0.0)` で clamp される。
        {
            let cur_clock_serial = clock.current_seek_serial();
            let cleared_pump_serial = {
                let mut buf = buffer.lock().unwrap();
                if buf.pump_seek_serial < cur_clock_serial {
                    let old_serial = buf.pump_seek_serial;
                    buf.processed.clear();
                    buf.drain_offset_in_first = 0;
                    buf.raw_pending.clear();
                    clock.zero_audio_tx_queued_secs();
                    publish_buffer_secs(&buf, &clock);
                    Some(old_serial)
                } else {
                    None
                }
            };

            if let Some(old_serial) = cleared_pump_serial {
                seek_target_secs = None;
                pump_anchor_target_secs = None;
                activated = false;

                let drain_result = drain_stale_audio_rx(&rx, cur_clock_serial, &mut deferred_frame);
                let should_log = drain_result.dropped > 0
                    || drain_result.deferred_serial.is_some()
                    || drain_result.hit_limit
                    || drain_result.disconnected
                    || last_stale_drain_log_serial != Some(cur_clock_serial);
                if should_log {
                    last_stale_drain_log_serial = Some(cur_clock_serial);
                    crate::logger::log(format!(
                        "[audio-pump] stale drain on seek: old_serial={} live_serial={} dropped={} deferred_serial={:?} rx_len={} hit_limit={} disconnected={}",
                        old_serial,
                        cur_clock_serial,
                        drain_result.dropped,
                        drain_result.deferred_serial,
                        drain_result.remaining_rx_len,
                        drain_result.hit_limit,
                        drain_result.disconnected
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "audio_pump",
                            "stale_drain_on_seek",
                            None,
                            0,
                            &[
                                ("old_serial", serde_json::Value::from(old_serial as i64)),
                                (
                                    "live_serial",
                                    serde_json::Value::from(cur_clock_serial as i64),
                                ),
                                (
                                    "dropped",
                                    serde_json::Value::from(drain_result.dropped as i64),
                                ),
                                (
                                    "deferred_serial",
                                    drain_result
                                        .deferred_serial
                                        .map(|s| serde_json::Value::from(s as i64))
                                        .unwrap_or(serde_json::Value::Null),
                                ),
                                (
                                    "remaining_rx_len",
                                    serde_json::Value::from(drain_result.remaining_rx_len as i64),
                                ),
                                ("hit_limit", serde_json::Value::from(drain_result.hit_limit)),
                            ],
                        );
                    }
                }
                continue;
            }
        }

        let preroll_now = clock.audio_preroll_suspended();
        if preroll_now {
            normalize_gain_ramp.snap_to_target(clock.normalize_gain() as f32);
            was_preroll_suspended = true;
            if let Ok(buf) = buffer.lock() {
                publish_buffer_secs(&buf, &clock);
            }
            continue;
        }
        if preroll_release_edge(was_preroll_suspended, preroll_now) {
            // preroll 解除エッジ: 測定確定 gain で即再生開始する (4 秒 ramp を避ける)。
            // ここで snap しておけば直後の apply_to_samples は target 一致で ramp を arm しない。
            normalize_gain_ramp.snap_to_target(clock.normalize_gain() as f32);
        }
        was_preroll_suspended = preroll_now;

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

            // ── Time stretch → normalize gain → VST process_block (mutex 解放中) ──
            let playback_speed = clock.playback_speed();
            let mut stretched =
                time_stretcher.process(&raw.samples, raw.duration_secs, playback_speed);
            // 音量ノーマライズの線形ゲイン (Phase 2-A): VST3 入力前に掛ける。
            // VST3 (Pro-L2 等) が「-14 LUFS に揃った入力」を見られるよう前段に置く。
            // 目標変更は dB 空間で ramp する。測定前待機からの解除時は上の
            // `snap_to_target` により、最初の可聴 chunk から仮 gain で始まる。
            let max_normalize_gain_in_block = normalize_gain_ramp
                .apply_to_samples(&mut stretched.samples, clock.normalize_gain() as f32);
            #[cfg(windows)]
            let (mut output_samples, mut current_pdc_latency_secs, vst_chain_active): (
                Vec<f32>,
                f64,
                bool,
            ) = if let Some(b) = &dsp_bridge {
                if b.is_enabled() && b.active_slot_count() > 0 {
                    fx_out.resize(stretched.samples.len(), 0.0);
                    let process_result = b.process_block(&stretched.samples, &mut fx_out);
                    let success = process_result.is_ok();
                    if success {
                        // 連続成功カウンタを進め、`HEALTHY_RESET` 回連続成功でようやく
                        // 失敗カウンタをリセット (= partial desync 中の偶発 Ok で counter が
                        // 不当にゼロ戻りするのを防ぐ)
                        vst3_consecutive_successes = vst3_consecutive_successes.saturating_add(1);
                        if vst3_consecutive_successes >= VST3_HEALTHY_RESET {
                            vst3_consecutive_failures = 0;
                        }
                        let total_lat_samples = b.total_latency_samples();
                        let lat_secs = if total_lat_samples > 0 {
                            total_lat_samples as f64 / sample_rate as f64
                        } else {
                            0.0
                        };
                        (fx_out.clone(), lat_secs, true)
                    } else {
                        // T07 (v0.9.0): VST3 process_block 失敗時の dry fallback
                        // を **PDC=0 + vst_chain_active=false** にする。旧コードは
                        // VST latency を残したまま dry サンプルを流していたので、
                        // dry にも PDC 補正が掛かって timing がずれていた。
                        // 連続失敗カウンタを進めて、閾値に達したら auto-disable。
                        vst3_consecutive_failures = vst3_consecutive_failures.saturating_add(1);
                        vst3_consecutive_successes = 0;
                        // ログ rate-limit: 最初の失敗と 10 回ごとに出す
                        if vst3_consecutive_failures == 1 || vst3_consecutive_failures % 10 == 0 {
                            crate::logger::log(format!(
                                "vst3 process_block failed (consecutive #{}): {}",
                                vst3_consecutive_failures,
                                process_result.unwrap_err()
                            ));
                        }
                        // T07 (v0.9.0) Codex P1 round 3 反映: 閾値 trigger は `>=` を使う +
                        // chain が現に enabled なときだけ disable を呼ぶ + 呼んだら counter を
                        // 0 にリセットする。
                        //
                        // `==` だけだと:
                        //   1. failures が threshold (3) に到達 → disable
                        //   2. ユーザーが GUI から re-enable
                        //   3. 次の失敗で counter が threshold+1 に → `==` を満たさず
                        //      二度と auto-disable が走らない
                        // という穴ができる。`>=` + counter reset で再 enable 後も正しく動く。
                        if vst3_consecutive_failures >= VST3_CONSECUTIVE_FAILURE_DISABLE
                            && b.is_enabled()
                        {
                            crate::logger::log(format!(
                                "vst3 process_block has failed {} consecutive times; \
                                 auto-disabling VST3 chain for this session",
                                vst3_consecutive_failures
                            ));
                            b.disable_with_reason(Some(format!(
                                "VST3 chain wedged after {} consecutive process_block failures; auto-disabled for this session",
                                vst3_consecutive_failures
                            )));
                            vst3_consecutive_failures = 0;
                        }
                        // 注 (Codex P1 round 3 限界事項): HEALTHY_RESET の hysteresis は
                        // 「N 回 Ok 連続 → 正常復帰」と heuristic 判定する。pipe-pairing drift
                        // (= tail of previous + head of current の偶発 Ok 連発) は構造的に
                        // 検出できない。完全な解決には bridge との discard/reset handshake が
                        // 必要で v0.10 で導入予定。drift 継続なら結局再 fail → 再 disable で
                        // 救う。
                        (stretched.samples.clone(), 0.0, false)
                    }
                } else {
                    // bridge は enable だが active slot が無い: dry 通過。
                    // active slot 不在のフレームは "成功でも失敗でもない"。counter は
                    // そのまま (= 一度 wedge 警告状態に入ったらユーザーが GUI で
                    // bypass を切る/再有効化するまで保つ)。
                    (stretched.samples.clone(), 0.0, false)
                }
            } else {
                // dsp_bridge ハンドル自体が無い: VST3 サポートが無効化されている。
                vst3_consecutive_failures = 0;
                vst3_consecutive_successes = 0;
                (stretched.samples.clone(), 0.0, false)
            };
            #[cfg(not(windows))]
            let (mut output_samples, mut current_pdc_latency_secs, vst_chain_active): (
                Vec<f32>,
                f64,
                bool,
            ) = (stretched.samples.clone(), 0.0, false);

            let pre_limiter_gain = clock.pre_limiter_gain();
            if pre_limiter_gain > 1.0 {
                for sample in &mut output_samples {
                    *sample *= pre_limiter_gain;
                }
            }
            // Phase 2-B: normalize gain が +側 (>1.0) のときも safety_limiter を通す。
            // VST3 無効 + 音量0dB以下 + normalize +20dB のケースで clip を防ぐ。
            // 下げ方向 (<1.0) は clip 不可なので limiter 不要 (5ms latency 節約)。
            let normalize_boost_active = max_normalize_gain_in_block > 1.0 + f32::EPSILON;
            let limiter_active =
                vst_chain_active || pre_limiter_gain > 1.0 || normalize_boost_active;
            if limiter_active {
                if safety_limiter.process_block(&mut output_samples) {
                    clock.mark_limiter_ceiling_hit();
                }
                current_pdc_latency_secs += safety_limiter.latency_secs();
            } else {
                safety_limiter.reset();
            }

            // ── chunk metadata 計算 ──
            // latency は output 秒で発生するため、source timeline に換算する。
            let duration_secs = output_samples.len() as f64 / samples_per_sec;
            let source_secs_per_output_sec = if duration_secs > 0.0 {
                stretched.source_secs_per_output_sec
            } else {
                playback_speed
            };
            current_pdc_latency_secs += stretched.stretcher_latency_output_secs;
            let (audible_pts_secs, current_latency_source_secs) = audible_pts_after_latency(
                raw.pts_secs,
                current_pdc_latency_secs,
                source_secs_per_output_sec,
            );

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
                    source_secs_per_output_sec,
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
                source_secs_per_output_sec,
                seek_serial: raw.seek_serial,
                pdc_latency_secs_at_process: current_latency_source_secs,
            };

            refresh_audio_tap(&audio_tap_command_rx, &mut active_audio_tap);
            // tap 接続時だけ samples を clone する。cpal callback と共有する AudioBuffer
            // mutex の外なので、この allocation が PC 側の drain を待たせることはない。
            let prepared_audio_tap = prepare_audio_tap_chunk(&active_audio_tap, &chunk);

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
            if (buf.pdc_latency_secs - current_latency_source_secs).abs() > 1e-6 {
                crate::logger::log(format!(
                    "Audio latency changed: {:.3}ms -> {:.3}ms source-time",
                    buf.pdc_latency_secs * 1000.0,
                    current_latency_source_secs * 1000.0
                ));
                buf.pdc_latency_secs = current_latency_source_secs;
            }
            publish_prepared_audio_tap_chunk(&mut active_audio_tap, prepared_audio_tap);
            buf.processed.push_back(chunk);
        }

        // ── publish_buffer_secs + BufferReady emit ──
        // BufferReady は **processed のみ** で判定 (Codex P1-2、raw を含めない)。
        // pts は **next_audible_pts** = `processed.front().audible_pts + drain_offset/sps`
        // を使う (Codex P1-1 反映、2026-05): seek 直後 next_pts_secs は input pts のまま
        // なので、PDC 適用後の audible 値を渡さないと engine の audio_anchor が
        // target-pdc に固定される。
        //
        // **Codex P1 修正 (2026-05-01)**: BufferReady の audio_anchor は
        // user-requested target を下限にする。seek 後の audio は target まで trim 済み
        // なので通常 audible ≈ target だが、PDC > 0 などで audible が target
        // より前に見えるケースを `pump_anchor_target_secs` との **max** で吸収する:
        //   - precise seek: audible ≈ target → max(audible, target) ≈ target
        //   - PDC 等で audible < target → max は target. anchor が target で固定
        //   - 失敗 / 初期 open: pump_anchor_target = None → audible 単独 (既存挙動)
        //   - BufferStarved (再 buffering): audible は再生位置 >> 旧 target → audible 採用
        let (processed_secs, cur_audible_pts, cur_serial) = {
            let buf = buffer.lock().unwrap();
            publish_buffer_secs(&buf, &clock);
            let secs: f64 = buf.processed.iter().map(|c| c.duration_secs).sum::<f64>()
                + remaining_first_chunk_secs(&buf);
            let audible = if let Some(first) = buf.processed.front() {
                first.audible_pts_secs
                    + (buf.drain_offset_in_first as f64 / buf.samples_per_sec)
                        * first.source_secs_per_output_sec
            } else {
                buf.next_pts_secs
            };
            (secs, audible, buf.pump_seek_serial)
        };
        // 音声実長が閾値未満のファイル (0.1 秒未満の SFX 等 / 極短音声トラックの動画) は
        // processed がこの閾値に永久に届かず、BufferReady が一度も emit されないまま
        // Buffering 固着する (再生開始不能)。demux がファイル全体を読み切っている
        // (`is_eof_reached`) ならこれ以上 processed が増える見込みは無いので、残量に
        // 関わらず readiness を通知する (review-v2.3.0 P2-6)。post-seek で末尾間際に
        // 到達した場合も同様 (残り実データが閾値未満でも開始してよい)。
        if processed_secs >= READY_THRESHOLD_SECS || clock.is_eof_reached() {
            // T15 (Codex R-VENG-001): BufferReady を **engine が待っている state でのみ** 送る。
            // 旧コードは Playing 中も pump loop ごと (audio frame rate ≈ 100-200Hz) に
            // BufferReady を try_send していた。engine 側はそれを epoch < current 早期 return
            // で no-op 処理するが、64-cap bounded lane を埋めてしまい、UI stall 中に
            // `SeekCompleted` / `FirstFrameReady` 等 one-shot critical event が
            // `try_send` → `Full` で **silently drop** される race を作っていた
            // (= state が Seeking で固着し audio mute する症状)。
            //
            // BufferReady は readiness latch を埋める edge-style gate。engine が **Buffering
            // または Loading** で待っているときだけ意味がある:
            //   - Loading: 起動直後 / 動画切替直後。FirstFrameReady と並行して latch を埋める
            //   - Buffering: post-seek の readiness 待ち、open 後の autoplay 等
            //   - Seeking: ❌ 含めない (Codex P1 2026-05-16)。`SeekCompleted` 受領で engine は
            //     latch を reset しつつ Buffering に遷移するため、Seeking 中の BufferReady は
            //     latch を「埋めて即 reset」される dead write。むしろ lane を浪費して
            //     SeekCompleted の前で詰まらせる原因になる
            //   - Playing/Paused/Eof: ❌ 含めない (latch 既セット or 不要、handler 早期 return)
            //
            // 再 buffering (`BufferStarved → Buffering`) は現状 production code で
            // BufferStarved 発火経路がないため (v0.9.0 時点で enum 定義 + handler + tests
            // のみ)、再 open 不要。将来 BufferStarved を実装する場合はその時点で本 gate も
            // 見直す。
            //
            // さらに stale-epoch check: `cur_serial != clock.current_seek_serial()` (= pump
            // が新世代 seek を観測する前に古い世代の processed buffer を見ている) のときは
            // 送らない。engine 側 `epoch < current_seek_epoch` で discard される dead event を
            // sender で先に弾く (lane の節約)。
            let engine_st = engine_state.load(Ordering::Acquire);
            let engine_waiting_for_ready =
                matches!(engine_st, state_code::LOADING | state_code::BUFFERING);
            let live_serial = clock.current_seek_serial();
            if engine_waiting_for_ready && cur_serial == live_serial {
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

        // ── A/V drift instrumentation: 1Hz snapshot + edge JSONL emit (Codex P1 ① 反映) ──
        // RT callback は atomic を書くだけ。実際の `perf::event` (= JSON 構築 + writer
        // mutex) は pump スレッドのここでまとめる。callback への影響ゼロ。
        if crate::perf::is_enabled() {
            let log_now = std::time::Instant::now();

            // (1) 1Hz snapshot: underrun 状態 / 直近 1 秒の silence ms / バッファ残量
            if log_now.duration_since(last_diag_log_at) >= std::time::Duration::from_secs(1) {
                let cur_underrun = diagnostics.audio_underrun_active.load(Ordering::Acquire);
                let silence_total = diagnostics
                    .audio_silence_samples_total
                    .load(Ordering::Acquire);
                let silence_delta_samples = silence_total.saturating_sub(last_silence_total_logged);
                last_silence_total_logged = silence_total;
                let silence_delta_ms = (silence_delta_samples as f64 / samples_per_sec) * 1000.0;
                crate::perf::event(
                    "audio_out",
                    "snapshot",
                    None,
                    0,
                    &[
                        ("underrun_active", serde_json::Value::from(cur_underrun)),
                        (
                            "silence_ms_last_sec",
                            serde_json::Value::from(silence_delta_ms),
                        ),
                        ("processed_secs", serde_json::Value::from(processed_secs)),
                        (
                            "audio_tx_queued_secs",
                            serde_json::Value::from(clock.audio_tx_queued_secs()),
                        ),
                    ],
                );
                last_diag_log_at = log_now;
            }

            // (2) underrun begin/end edge: callback 側が seq を bump しているので
            //     その変化を poll して即時 emit (50-200ms 解像度で取りたい)。
            let cur_begin_seq = diagnostics.audio_underrun_begin_seq.load(Ordering::Acquire);
            if cur_begin_seq != last_seen_underrun_begin_seq {
                last_seen_underrun_begin_seq = cur_begin_seq;
                let wall_ns = diagnostics
                    .audio_underrun_begin_wall_ns
                    .load(Ordering::Acquire);
                let edge_age_ms =
                    ((diagnostics.wall_ns_now().saturating_sub(wall_ns)) as f64) / 1.0e6;
                crate::perf::event(
                    "audio_out",
                    "underrun_begin",
                    None,
                    0,
                    &[
                        ("edge_wall_ns", serde_json::Value::from(wall_ns as i64)),
                        ("edge_age_ms", serde_json::Value::from(edge_age_ms)),
                    ],
                );
            }
            let cur_end_seq = diagnostics.audio_underrun_end_seq.load(Ordering::Acquire);
            if cur_end_seq != last_seen_underrun_end_seq {
                last_seen_underrun_end_seq = cur_end_seq;
                let wall_ns = diagnostics
                    .audio_underrun_end_wall_ns
                    .load(Ordering::Acquire);
                let edge_age_ms =
                    ((diagnostics.wall_ns_now().saturating_sub(wall_ns)) as f64) / 1.0e6;
                crate::perf::event(
                    "audio_out",
                    "underrun_end",
                    None,
                    0,
                    &[
                        ("edge_wall_ns", serde_json::Value::from(wall_ns as i64)),
                        ("edge_age_ms", serde_json::Value::from(edge_age_ms)),
                    ],
                );
            }

            // (3) audio_pts_jump: callback 側で閾値判定済みのものだけ seq が上がる。
            let cur_jump_seq = diagnostics.audio_pts_jump_seq.load(Ordering::Acquire);
            if cur_jump_seq != last_seen_pts_jump_seq {
                last_seen_pts_jump_seq = cur_jump_seq;
                let req = f64::from_bits(
                    diagnostics
                        .audio_pts_jump_requested_bits
                        .load(Ordering::Acquire),
                );
                let prev = f64::from_bits(
                    diagnostics
                        .audio_pts_jump_prev_now_bits
                        .load(Ordering::Acquire),
                );
                let after = f64::from_bits(
                    diagnostics
                        .audio_pts_jump_after_now_bits
                        .load(Ordering::Acquire),
                );
                let wall_ns = diagnostics.audio_pts_jump_wall_ns.load(Ordering::Acquire);
                let req_delta_ms = (req - prev) * 1000.0;
                let applied_delta_ms = (after - prev) * 1000.0;
                let edge_age_ms =
                    ((diagnostics.wall_ns_now().saturating_sub(wall_ns)) as f64) / 1.0e6;
                crate::perf::event(
                    "audio_out",
                    "audio_pts_jump",
                    None,
                    0,
                    &[
                        ("requested_pts", serde_json::Value::from(req)),
                        ("prev_now", serde_json::Value::from(prev)),
                        ("after_now", serde_json::Value::from(after)),
                        ("requested_delta_ms", serde_json::Value::from(req_delta_ms)),
                        (
                            "applied_delta_ms",
                            serde_json::Value::from(applied_delta_ms),
                        ),
                        ("edge_wall_ns", serde_json::Value::from(wall_ns as i64)),
                        ("edge_age_ms", serde_json::Value::from(edge_age_ms)),
                    ],
                );
            }
        }
    }
    // ── 終了時の silence flush (= 既存) ──
    // T20 (Codex P2 2026-05-16): cancel が立っているときは flush_silence をスキップする。
    // `flush_silence(480, 10)` は 10 反復で各反復 200ms timeout = 最大 2 秒ブロック。
    // 通常終了 (= 動画 EOF / 停止) では tail silence を吐き切る価値があるが、cancel 経由
    // (= 動画切替 / Drop) では即 exit したいので skip。
    #[cfg(windows)]
    if !cancel.load(Ordering::Acquire) {
        if let Some(b) = &dsp_bridge {
            if b.is_enabled() && b.active_slot_count() > 0 {
                b.flush_silence(480, 10);
            }
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
    output_duration_secs: f64,
    source_secs_per_output_sec: f64,
    target_secs: f64,
    samples_per_sec: f64,
    channels: usize,
) -> TrimResult {
    let source_rate = source_secs_per_output_sec.max(1.0e-6);
    let audible_end = audible_pts_secs + output_duration_secs * source_rate;
    if audible_end <= target_secs - 1e-6 {
        TrimResult::DropAll
    } else if audible_pts_secs >= target_secs - 1e-6 {
        TrimResult::KeepAll
    } else {
        let trim_source_secs = target_secs - audible_pts_secs;
        let trim_output_secs = trim_source_secs / source_rate;
        let trim_samples_raw = (trim_output_secs * samples_per_sec).round() as usize;
        // channel-aligned (= interleaved stereo は 2 単位)
        let trim_samples = (trim_samples_raw / channels) * channels;
        let new_audible_pts =
            audible_pts_secs + (trim_samples as f64 / samples_per_sec) * source_rate;
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

/// PLAYING drain path 出口で呼ぶ silence/underrun 集計ヘルパー (RT-safe)。
///
/// ## 適用範囲 (Codex 5 巡目 P2 ②)
/// **PLAYING かつ `clock.is_playing()` 通過後の drain path** の出口だけで呼ぶ。
/// `fill_output` 内の他の silence return 経路 (= stale clear / 非 PLAYING /
/// pause の意図 silence) は **underrun ではない**ので finalize は通さない。
/// さもないと意図無音を underrun と誤計測する。
///
/// ## 単位
/// `want` / `written` は **stereo interleaved の f32 sample 数** (= `want = 2 * frames`)。
/// silence ms は `want / samples_per_sec * 1000.0` (`samples_per_sec = sample_rate * 2.0`)。
/// JSONL emit は pump スレッドが atomic を読んで行う (callback では何も emit しない)。
pub(crate) fn finalize_fill_output(diagnostics: &AudioDiagnostics, want: usize, written: usize) {
    let silence_samples = want.saturating_sub(written);
    if silence_samples > 0 {
        diagnostics
            .audio_silence_samples_total
            .fetch_add(silence_samples as u64, Ordering::Release);
        let was = diagnostics
            .audio_underrun_active
            .swap(true, Ordering::AcqRel);
        if !was {
            // begin edge — begin 専用 atomic 群を更新
            diagnostics
                .audio_underrun_begin_wall_ns
                .store(diagnostics.wall_ns_now(), Ordering::Release);
            diagnostics
                .audio_underrun_begin_seq
                .fetch_add(1, Ordering::Release);
        }
    } else {
        let was = diagnostics
            .audio_underrun_active
            .swap(false, Ordering::AcqRel);
        if was {
            // end edge — end 専用 atomic 群を更新
            diagnostics
                .audio_underrun_end_wall_ns
                .store(diagnostics.wall_ns_now(), Ordering::Release);
            diagnostics
                .audio_underrun_end_seq
                .fetch_add(1, Ordering::Release);
        }
    }
}

fn fill_output(
    out: &mut [f32],
    buffer: &Arc<Mutex<AudioBuffer>>,
    clock: &Arc<AvClock>,
    engine_state: &Arc<AtomicU8>,
    diagnostics: &Arc<AudioDiagnostics>,
) {
    // ── pre-seek discard (= state gate より先、Codex P1-4) ──
    let clock_serial = clock.current_seek_serial();
    let mut buf = buffer.lock().unwrap();

    if buf.pump_seek_serial < clock_serial {
        let pump_serial = buf.pump_seek_serial;
        let processed_secs: f64 = buf.processed.iter().map(|c| c.duration_secs).sum::<f64>()
            + remaining_first_chunk_secs(&buf);
        let raw_pending_secs: f64 = buf.raw_pending.iter().map(|f| f.duration_secs).sum::<f64>();
        let should_log = buf.last_fill_stale_clear_logged_serial != clock_serial;
        if should_log {
            buf.last_fill_stale_clear_logged_serial = clock_serial;
        }
        buf.processed.clear();
        buf.drain_offset_in_first = 0;
        buf.raw_pending.clear();
        publish_buffer_secs(&buf, clock);
        drop(buf);
        if should_log {
            crate::logger::log(format!(
                "[audio-out] fill_output stale clear: pump_serial={} live_serial={} processed_secs={:.3} raw_pending_secs={:.3}",
                pump_serial, clock_serial, processed_secs, raw_pending_secs
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "audio_out",
                    "fill_output_stale_clear",
                    None,
                    0,
                    &[
                        ("pump_serial", serde_json::Value::from(pump_serial as i64)),
                        ("live_serial", serde_json::Value::from(clock_serial as i64)),
                        ("processed_secs", serde_json::Value::from(processed_secs)),
                        (
                            "raw_pending_secs",
                            serde_json::Value::from(raw_pending_secs),
                        ),
                    ],
                );
            }
        }
        out.fill(0.0);
        return;
    }

    if clock.audio_preroll_suspended() {
        publish_buffer_secs(&buf, clock);
        out.fill(0.0);
        return;
    }

    // ── EngineState gate (Codex P1-4): PLAYING 以外は silence + 非 drain ──
    //
    // PLAYING 以外で processed を drain しないため、上流が音声を作り続けると
    // raw_pending → processed → audio_tx → audio_pkt_tx の順に逆圧が連鎖する。
    // そのため decoder.rs の audio decode thread は PAUSED/EOF で park し、タイル
    // fast swap 中などの停止状態で audio 側だけが queue を満杯にしないようにする。
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

    let vol = clock.output_volume();

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
        let chunk_source_rate = first.source_secs_per_output_sec;
        chunk_pdc_latency_at_drain = Some(chunk_latency);

        for i in 0..take {
            out[written + i] = first.samples[buf.drain_offset_in_first + i] * vol;
        }
        written += take;
        real_consumed += take;
        buf.drain_offset_in_first += take;

        // chunk 内 drain 後の audible_pts を計算 (= 次に drain される予定の PTS)
        next_audible_pts = Some(
            chunk_audible_pts
                + (buf.drain_offset_in_first as f64 / samples_per_sec) * chunk_source_rate,
        );

        if buf.drain_offset_in_first >= buf.processed.front().map(|c| c.samples.len()).unwrap_or(0)
        {
            buf.processed.pop_front();
            buf.drain_offset_in_first = 0;
        }
    }

    // ── bookkeeping ──
    if real_consumed == 0 {
        publish_buffer_secs(&buf, clock);
        // PLAYING drain path の full underrun 出口。silence_samples = want。
        finalize_fill_output(diagnostics, want, written);
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
        // ── audio_pts_jump 計装 (Codex 5 巡目 P2 ① 反映、上書き対策) ──
        // requested vs applied を計測して、wall-rate cap や monotonic guard が
        // 効いた場合の差分を検出する。**大ジャンプ専用 atomic** に書くのは
        // `should_record_pts_jump` 閾値判定で true のときだけ (= 通常の小さい更新は
        // pump が読む前に上書きされない)。
        let prev_now = clock.now_secs();
        let requested = pts_for_video;
        if latency_jumped {
            clock.set_audio_pts_jump(pts_for_video);
        } else {
            clock.set_audio_pts(pts_for_video);
        }
        let after_now = clock.now_secs();
        let req_delta_ms = (requested - prev_now) * 1000.0;
        let applied_delta_ms = (after_now - prev_now) * 1000.0;

        // ── 体感ズレ検出用の連続メトリクス ──
        // Norm 経路で `clear_audio_output_buffer` が `raw_pending` 5 秒分を捨てた後、
        // 新しく届く audio frame の audible PTS は前回 clock より +5s 先になる。
        // 一方 wall-rate cap で master clock は 1.02x rate でしか追いつけないため、
        // この差は数分間そのまま残る。`av_drift_ms` (= video − master_clock) は両方が
        // 連動して進むので 0 近辺に張り付き、ユーザー体感の音映像差を捉えられない。
        // ここで `audio_audible_pts` と `audio_lead_ms` を毎 callback 更新することで
        // overlay と analyze_perf 側がこの状況を即座に把握できるようにする。
        //
        // Codex P2 ② 反映: `audio_lead_ms` は **post-apply residual** にする。
        //   旧版: `req_delta = requested − prev_now` (= 補正要求量)
        //   新版: `lead = requested − after_now` (= 補正後でも残っている乖離)
        // 旧版だと wall extrapolation 分で通常時にも +10ms 程度の偽 lead が見えた。
        // 新版は通常時 ≈ 0、Norm 経路バグ時のみ +5000ms 級が表示される。
        let post_apply_lead_ms = (requested - after_now) * 1000.0;
        diagnostics
            .audio_audible_pts_bits
            .store(requested.to_bits(), Ordering::Release);
        // bits 書き込み → valid=true の順 (= load 側は valid → bits の逆順で読むので、
        // この順だと「valid=true で旧 bits」の中間状態が見えない)
        diagnostics
            .audio_audible_pts_valid
            .store(true, Ordering::Release);
        diagnostics
            .audio_lead_ms_bits
            .store(post_apply_lead_ms.to_bits(), Ordering::Release);

        if AudioDiagnostics::should_record_pts_jump(req_delta_ms, applied_delta_ms) {
            diagnostics
                .audio_pts_jump_requested_bits
                .store(requested.to_bits(), Ordering::Release);
            diagnostics
                .audio_pts_jump_prev_now_bits
                .store(prev_now.to_bits(), Ordering::Release);
            diagnostics
                .audio_pts_jump_after_now_bits
                .store(after_now.to_bits(), Ordering::Release);
            diagnostics
                .audio_pts_jump_wall_ns
                .store(diagnostics.wall_ns_now(), Ordering::Release);
            diagnostics
                .audio_pts_jump_seq
                .fetch_add(1, Ordering::Release);
        }
        if (pts_for_video - after_now).abs() <= SEEK_TARGET_TOLERANCE_SECS {
            clock.clear_seek_target_override(pump_serial);
        }
    }

    // PLAYING drain path の正常終了出口。silence_samples = want - written
    // (= 0 なら recovery edge、>0 なら partial underrun)。
    finalize_fill_output(diagnostics, want, written);
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
            last_fill_stale_clear_logged_serial: 0,
            pdc_latency_secs: 0.0,
            pdc_latency_secs_applied: 0.0,
        }))
    }

    /// PLAYING engine state を test 用に作る。
    fn playing_state() -> Arc<AtomicU8> {
        Arc::new(AtomicU8::new(state_code::PLAYING))
    }

    /// テスト用 AudioDiagnostics (= atomic 全 0、started_at = now)。
    fn make_diag() -> Arc<AudioDiagnostics> {
        Arc::new(AudioDiagnostics::new(std::time::Instant::now()))
    }

    /// テスト用の processed chunk を作る。
    fn make_chunk(samples: Vec<f32>, pts_secs: f64, samples_per_sec: f64) -> ProcessedChunk {
        let duration_secs = samples.len() as f64 / samples_per_sec;
        ProcessedChunk {
            samples,
            audible_pts_secs: pts_secs,
            duration_secs,
            source_secs_per_output_sec: 1.0,
            seek_serial: 0,
            pdc_latency_secs_at_process: 0.0,
        }
    }

    #[test]
    fn audible_pts_uses_source_timeline_latency_and_clamps_at_zero() {
        let (audible, latency_source) = audible_pts_after_latency(10.0, 0.125, 1.0);
        assert!((audible - 9.875).abs() < 1.0e-12);
        assert!((latency_source - 0.125).abs() < 1.0e-12);

        let (audible_2x, latency_source_2x) = audible_pts_after_latency(10.0, 0.125, 2.0);
        assert!((audible_2x - 9.75).abs() < 1.0e-12);
        assert!((latency_source_2x - 0.25).abs() < 1.0e-12);

        let (clamped, _) = audible_pts_after_latency(0.05, 0.125, 1.0);
        assert_eq!(clamped, 0.0);
    }

    #[test]
    fn disconnected_tap_preserves_playback_chunk_bit_for_bit_and_allocation() {
        let (_command_tx, command_rx) = unbounded();
        let mut active = None;
        let samples = vec![0.0, -0.0, 0.25, -0.5, f32::from_bits(0x7fc0_0001)];
        let expected_bits = samples
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>();
        let chunk = make_chunk(samples, 3.25, 96_000.0);
        let samples_ptr = chunk.samples.as_ptr();
        let samples_capacity = chunk.samples.capacity();

        refresh_audio_tap(&command_rx, &mut active);
        let prepared = prepare_audio_tap_chunk(&active, &chunk);
        publish_prepared_audio_tap_chunk(&mut active, prepared);
        let mut playback = std::collections::VecDeque::with_capacity(1);
        playback.push_back(chunk);

        let output = playback.pop_front().unwrap();
        assert_eq!(output.samples.as_ptr(), samples_ptr);
        assert_eq!(output.samples.capacity(), samples_capacity);
        assert_eq!(
            output
                .samples
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            expected_bits
        );
    }

    #[test]
    fn remote_local_mute_does_not_change_post_dsp_tap_payload() {
        let clock = make_clock();
        let _mute = clock.acquire_remote_local_output_mute();
        assert_eq!(clock.output_volume(), 0.0);

        let expected = vec![0.25, -0.5, 0.75, -1.0];
        let chunk = make_chunk(expected.clone(), 2.0, 96_000.0);
        let (payload_tx, payload_rx) = bounded(1);
        let mut active = Some(ActiveAudioTap {
            owner_id: 1,
            payload_tx,
            dropped: Arc::new(AtomicU64::new(0)),
        });
        let prepared = prepare_audio_tap_chunk(&active, &chunk);
        publish_prepared_audio_tap_chunk(&mut active, prepared);

        assert_eq!(payload_rx.recv().unwrap().samples, expected);
        assert_eq!(chunk.samples, vec![0.25, -0.5, 0.75, -1.0]);
    }

    #[test]
    fn remote_local_mute_keeps_playing_callback_as_processed_consumer() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let _mute = clock.acquire_remote_local_output_mute();
        let samples_per_sec = buf.lock().unwrap().samples_per_sec;
        buf.lock()
            .unwrap()
            .processed
            .push_back(make_chunk(vec![0.5; 480], 0.0, samples_per_sec));

        let mut out = [1.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &playing_state(), &make_diag());

        assert!(buf.lock().unwrap().processed.is_empty());
        assert!(out.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn dropping_audio_tap_lease_detaches_and_restores_allocation_free_playback_path() {
        let (command_tx, command_rx) = unbounded();
        let controller = AudioTapController {
            command_tx,
            next_owner_id: Arc::new(AtomicU64::new(1)),
        };
        let (lease, payload_rx) = controller.attach(1).unwrap();
        let mut active = None;
        refresh_audio_tap(&command_rx, &mut active);
        drop(lease);
        refresh_audio_tap(&command_rx, &mut active);

        let chunk = make_chunk(vec![0.25, -0.25], 0.0, 96_000.0);
        assert!(matches!(
            prepare_audio_tap_chunk(&active, &chunk),
            PreparedAudioTapChunk::NotConnected
        ));
        assert!(matches!(
            payload_rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn full_tap_never_blocks_playback_and_counts_drop() {
        let (payload_tx, payload_rx) = bounded(1);
        payload_tx
            .try_send(make_chunk(vec![1.0, -1.0], 0.0, 96_000.0))
            .unwrap();
        let dropped = Arc::new(AtomicU64::new(0));
        let active = Some(ActiveAudioTap {
            owner_id: 7,
            payload_tx,
            dropped: Arc::clone(&dropped),
        });
        let (done_tx, done_rx) = bounded(1);

        std::thread::spawn(move || {
            let mut active = active;
            let chunk = make_chunk(vec![0.25, -0.25], 0.1, 96_000.0);
            let prepared = prepare_audio_tap_chunk(&active, &chunk);
            publish_prepared_audio_tap_chunk(&mut active, prepared);
            let mut playback = std::collections::VecDeque::new();
            playback.push_back(chunk);
            done_tx.send(playback.pop_front().unwrap()).unwrap();
        });

        let playback_chunk = done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("full tap path must not wait for receiver capacity");
        assert_eq!(playback_chunk.samples, vec![0.25, -0.25]);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(payload_rx.len(), 1);
    }

    fn make_audio_frame(seek_serial: u64) -> AudioFrame {
        AudioFrame {
            samples: vec![0.0; 2],
            pts_secs: seek_serial as f64,
            seek_serial,
            duration_secs: 0.01,
            queued_wall_secs: 0.01,
            audio_tx_accounting_epoch: 0,
            seek_target_secs: Some(seek_serial as f64),
        }
    }

    #[test]
    fn normalize_gain_ramp_interpolates_in_db_space() {
        let mut ramp = NormalizeGainRamp::new(10, 2);
        let target = 10.0_f32.powf(6.0 / 20.0);
        let mut first_quarter = vec![1.0_f32; 20]; // 10 stereo frames; full ramp = 40 frames.

        let max_gain = ramp.apply_to_samples(&mut first_quarter, target);

        assert!(first_quarter[0] > 1.0);
        assert!(first_quarter[18] < target);
        assert!(max_gain < target);
        assert_eq!(ramp.remaining_frames(), 30);

        let mut rest = vec![1.0_f32; 80];
        let max_gain = ramp.apply_to_samples(&mut rest, target);

        assert_eq!(ramp.remaining_frames(), 0);
        assert!((rest[78] - target).abs() < 1.0e-5);
        assert!((max_gain - target).abs() < 1.0e-5);
    }

    #[test]
    fn normalize_gain_ramp_can_snap_for_preroll() {
        let mut ramp = NormalizeGainRamp::new(48_000, 2);
        ramp.apply_to_samples(&mut [1.0_f32; 16], 2.0);
        ramp.snap_to_target(0.5);

        let mut samples = vec![1.0_f32; 8];
        let max_gain = ramp.apply_to_samples(&mut samples, 0.5);

        assert_eq!(ramp.remaining_frames(), 0);
        assert!((max_gain - 0.5).abs() < 1.0e-6);
        assert!(samples.iter().all(|s| (*s - 0.5).abs() < 1.0e-6));
    }

    #[test]
    fn preroll_release_edge_detects_only_true_to_false() {
        assert!(preroll_release_edge(true, false), "解除エッジ (snap する)");
        assert!(
            !preroll_release_edge(false, false),
            "通常再生 (ramp を活かす)"
        );
        assert!(!preroll_release_edge(true, true), "preroll 継続中");
        assert!(!preroll_release_edge(false, true), "preroll 開始エッジ");
    }

    /// 再生前スキャン (deferred normalize) の解除エッジで snap すれば、確定 gain で
    /// 最初のブロックから始まり 4 秒 ramp しない、という pump の不変条件を検証する。
    /// pump は preroll 中 old gain を snap し続け (race で確定 gain を捕まえ損ねうる)、
    /// 解除エッジで確定 gain を snap する。そのシーケンスを再現する。
    #[test]
    fn normalize_gain_snap_on_preroll_release_edge_avoids_ramp() {
        let mut ramp = NormalizeGainRamp::new(48_000, 2);
        ramp.snap_to_target(1.0); // pump 起動時の初期 snap
        // preroll 中: old gain (=1.0) を snap し続ける (確定前)。
        ramp.snap_to_target(1.0);
        // 解除エッジ: UI が set_normalize_gain(2.0) → preroll 解除。pump は解除ブロックで
        // 確定 gain=2.0 を snap する。
        ramp.snap_to_target(2.0);
        // 解除後の最初の可聴ブロック: target=2.0 を適用しても snap 済みなので ramp を arm しない。
        let mut samples = vec![1.0_f32; 32];
        let max_gain = ramp.apply_to_samples(&mut samples, 2.0);
        assert_eq!(
            ramp.remaining_frames(),
            0,
            "解除エッジ snap 後は ramp しない"
        );
        assert!((max_gain - 2.0).abs() < 1.0e-4);
        assert!(
            samples.iter().all(|s| (*s - 2.0).abs() < 1.0e-4),
            "最初のブロックから確定 gain で始まる (徐々に上がらない)"
        );
    }

    #[test]
    fn drain_stale_audio_rx_drops_old_and_defers_first_current() {
        let (tx, rx) = bounded(8);
        tx.send(make_audio_frame(1)).unwrap();
        tx.send(make_audio_frame(1)).unwrap();
        tx.send(make_audio_frame(2)).unwrap();
        tx.send(make_audio_frame(3)).unwrap();

        let mut deferred = None;
        let result = drain_stale_audio_rx(&rx, 2, &mut deferred);

        assert_eq!(result.dropped, 2);
        assert_eq!(result.deferred_serial, Some(2));
        assert_eq!(deferred.as_ref().map(|f| f.seek_serial), Some(2));
        assert_eq!(
            rx.len(),
            1,
            "frames after the deferred one must stay queued"
        );
        assert!(!result.disconnected);
    }

    #[test]
    fn drain_stale_audio_rx_drops_all_stale_without_deferred() {
        let (tx, rx) = bounded(8);
        tx.send(make_audio_frame(0)).unwrap();
        tx.send(make_audio_frame(1)).unwrap();

        let mut deferred = None;
        let result = drain_stale_audio_rx(&rx, 2, &mut deferred);

        assert_eq!(result.dropped, 2);
        assert_eq!(result.deferred_serial, None);
        assert!(deferred.is_none());
        assert_eq!(rx.len(), 0);
        assert!(!result.hit_limit);
    }

    #[test]
    fn safety_limiter_delays_audio_by_lookahead() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        assert_eq!(limiter.lookahead_frames, 5);

        let mut samples = vec![0.0_f32; 12];
        samples[0] = 0.5;
        samples[1] = -0.5;
        assert!(!limiter.process_block(&mut samples));

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

        assert!(limiter.process_block(&mut samples));

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
    fn safety_limiter_passes_signal_below_full_scale() {
        // -1 dBFS ~ 0 dBFS の素材は ceiling (0 dBFS) を超えないので素通し。
        // 旧 ceiling (-1 dBFS) では抑えられていたケースが、そのまま通ることを確認する。
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut samples = vec![0.0_f32; 18];
        samples[0] = 0.95; // ≈ -0.45 dBFS
        samples[1] = -0.95;
        assert!(!limiter.process_block(&mut samples));
        assert!((samples[10] - 0.95).abs() < 1e-6);
        assert!((samples[11] + 0.95).abs() < 1e-6);
    }

    #[test]
    fn safety_limiter_indicator_ignores_sub_threshold_overshoot() {
        // ceiling を ~0.5 dB だけ超えるピーク: リミッターは抑え込むが、ゲイン
        // リダクションが SAFETY_LIMITER_INDICATOR_GR_DB (1 dB) 未満なので
        // ピークランプは点かない (タイムストレッチの微小オーバー相当)。
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut samples = vec![0.0_f32; 18];
        samples[0] = 1.06;
        samples[1] = -1.06;
        assert!(!limiter.process_block(&mut samples));
        let max_abs = samples.iter().fold(0.0_f32, |acc, &v| acc.max(v.abs()));
        assert!(
            max_abs <= limiter.ceiling + 1e-6,
            "limiter still protects the output even below the indicator threshold"
        );
    }

    #[test]
    fn safety_limiter_indicator_fires_on_one_db_reduction() {
        // 1 dB 以上のゲインリダクションを要するピーク → ピークランプ点灯。
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut samples = vec![0.0_f32; 18];
        samples[0] = 1.5;
        samples[1] = -1.5;
        assert!(limiter.process_block(&mut samples));
    }

    #[test]
    fn safety_limiter_keeps_delay_across_blocks() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut first = vec![0.0_f32; 6];
        first[0] = 0.5;
        first[1] = -0.5;
        assert!(!limiter.process_block(&mut first));
        assert!(
            first.iter().all(|&v| v == 0.0),
            "first block is shorter than lookahead, so it should be delayed"
        );

        let mut second = vec![0.0_f32; 6];
        assert!(!limiter.process_block(&mut second));
        assert!((second[4] - 0.5).abs() < 1e-6);
        assert!((second[5] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn safety_limiter_releases_gain_across_blocks() {
        let mut limiter = SafetyLimiter::new(1_000, 2);
        let mut loud = vec![0.0_f32; 14];
        loud[0] = 2.0;
        loud[1] = -2.0;
        assert!(limiter.process_block(&mut loud));
        let gain_after_peak = limiter.gain;
        assert!(
            gain_after_peak < 0.6,
            "loud peak should force immediate gain reduction"
        );

        let mut quiet = vec![0.0_f32; 200];
        assert!(!limiter.process_block(&mut quiet));
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
        assert!(!limiter.process_block(&mut samples));
        limiter.reset();

        let mut silence = vec![0.0_f32; 12];
        assert!(!limiter.process_block(&mut silence));
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
        fill_output(&mut out, &buf, &clock, &playing_state(), &make_diag());

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
        fill_output(&mut out, &buf, &clock, &playing_state(), &make_diag());

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
        fill_output(&mut out, &buf, &clock, &playing_state(), &make_diag());

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
        fill_output(&mut out, &buf, &clock, &buffering_state, &make_diag());

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

    /// PAUSED 中は drain せず、PLAYING へ戻したら同じ buffer を消費し始める。
    #[test]
    fn fill_output_paused_then_playing_starts_drain() {
        let buf = make_buffer(48_000);
        let clock = make_clock();
        let samples_per_sec = buf.lock().unwrap().samples_per_sec;
        let state = Arc::new(AtomicU8::new(state_code::PAUSED));

        {
            let mut b = buf.lock().unwrap();
            b.processed
                .push_back(make_chunk(vec![0.5; 480], 0.0, samples_per_sec));
        }
        let pts_before = buf.lock().unwrap().next_pts_secs;

        let mut out = [1.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &state, &make_diag());
        assert!(
            out.iter().all(|&s| s == 0.0),
            "Paused: output must be silence"
        );
        assert_eq!(
            buf.lock().unwrap().processed.front().unwrap().samples.len(),
            480,
            "Paused: processed chunk must remain queued"
        );
        assert_eq!(
            buf.lock().unwrap().next_pts_secs,
            pts_before,
            "Paused: pts must not advance"
        );

        state.store(state_code::PLAYING, Ordering::Release);
        fill_output(&mut out, &buf, &clock, &state, &make_diag());
        assert!(
            out.iter().all(|&s| (s - 0.3).abs() < 1e-6),
            "Playing: buffered samples should be drained with volume applied"
        );
        assert!(
            buf.lock().unwrap().processed.is_empty(),
            "Playing: processed chunk should be consumed"
        );
    }

    /// PDC pre-target trim: chunk 全体が target 前なら DropAll。
    #[test]
    fn pre_target_trim_drops_chunk_fully_before_target() {
        // PDC = 1.0 sec、input pts = 5.0、duration = 0.023 (= 1 frame)
        // audible_pts = 5.0 - 1.0 = 4.0、audible_end = 4.023
        // target = 5.0 → audible_end < target → DropAll
        let result = pre_target_trim_decision(4.0, 0.023, 1.0, 5.0, 96000.0, 2);
        assert!(matches!(result, TrimResult::DropAll));
    }

    /// PDC pre-target trim: chunk 全体が target 以降なら KeepAll。
    #[test]
    fn pre_target_trim_keeps_chunk_fully_after_target() {
        // input pts = 5.5、PDC = 0.0、audible_pts = 5.5
        // target = 5.0 → audible_pts >= target → KeepAll
        let result = pre_target_trim_decision(5.5, 0.023, 1.0, 5.0, 96000.0, 2);
        assert!(matches!(result, TrimResult::KeepAll));
    }

    /// PDC pre-target trim: chunk が target を跨ぐ → TrimFront。
    #[test]
    fn pre_target_trim_splits_chunk_crossing_target() {
        // PDC = 0.5、input pts = 5.5、duration = 1.0、audible_pts = 5.0
        // target = 5.5 → 跨ぐ → 先頭 0.5 sec trim、new_audible = 5.5
        // sample_per_sec = 96000、trim = 0.5 * 96000 = 48000 samples (channel-aligned)
        let result = pre_target_trim_decision(5.0, 1.0, 1.0, 5.5, 96000.0, 2);
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

    /// PDC pre-target trim: 2.0x では output 0.25s が source 0.5s に相当する。
    #[test]
    fn pre_target_trim_handles_double_speed() {
        // audible_pts=4.0、output_duration=0.5、source_rate=2.0 → audible_end=5.0
        // target=4.5 → source 0.5s 分だけ trim。output では 0.25s。
        let result = pre_target_trim_decision(4.0, 0.5, 2.0, 4.5, 96000.0, 2);
        match result {
            TrimResult::TrimFront {
                trim_samples,
                new_audible_pts,
            } => {
                assert_eq!(trim_samples, 24000);
                assert!((new_audible_pts - 4.5).abs() < 1e-6);
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
            let result = pre_target_trim_decision(audible_pts, frame_duration, 1.0, target, sps, 2);
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
                source_secs_per_output_sec: 1.0,
                seek_serial: 0,
                pdc_latency_secs_at_process: 1.0,
            };
            b.processed.push_back(chunk);
        }

        let mut out = [0.0_f32; 480];
        fill_output(&mut out, &buf, &clock, &playing_state(), &make_diag());

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
        fill_output(&mut out, &buf, &clock, &playing_state(), &make_diag());

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

    // ── finalize_fill_output: PLAYING drain path 出口の silence/underrun 集計 ──
    // (Codex 5 巡目 P3 ③ 反映)。
    // - want / written は **stereo interleaved sample 数**。silence_samples = want - written。
    // - silence_samples > 0 → underrun begin edge (false → true) で begin_seq +1。
    // - silence_samples == 0 → underrun end edge (true → false) で end_seq +1。
    // - 連続呼び出しで edge にならない場合 (= 同じ状態のまま) は seq を上げない。

    #[test]
    fn finalize_fill_output_full_underrun_writes_silence_total_and_begin_edge() {
        let diag = make_diag();
        // 初期: active=false, totals=0
        assert!(!diag.audio_underrun_active.load(Ordering::Acquire));
        assert_eq!(diag.audio_silence_samples_total.load(Ordering::Acquire), 0);

        // want=1024, written=0 → silence_samples=1024 (full underrun)
        finalize_fill_output(&diag, 1024, 0);

        assert!(diag.audio_underrun_active.load(Ordering::Acquire));
        assert_eq!(
            diag.audio_silence_samples_total.load(Ordering::Acquire),
            1024
        );
        assert_eq!(diag.audio_underrun_begin_seq.load(Ordering::Acquire), 1);
        assert_eq!(diag.audio_underrun_end_seq.load(Ordering::Acquire), 0);
    }

    #[test]
    fn finalize_fill_output_partial_underrun_increments_silence() {
        let diag = make_diag();

        // want=1024, written=512 → silence_samples=512 (partial underrun)
        finalize_fill_output(&diag, 1024, 512);

        assert!(diag.audio_underrun_active.load(Ordering::Acquire));
        assert_eq!(
            diag.audio_silence_samples_total.load(Ordering::Acquire),
            512
        );
        assert_eq!(diag.audio_underrun_begin_seq.load(Ordering::Acquire), 1);
    }

    #[test]
    fn finalize_fill_output_recovery_emits_end_edge() {
        let diag = make_diag();

        // 1) full underrun → begin edge
        finalize_fill_output(&diag, 1024, 0);
        assert!(diag.audio_underrun_active.load(Ordering::Acquire));
        assert_eq!(diag.audio_underrun_begin_seq.load(Ordering::Acquire), 1);

        // 2) full drain → end edge
        finalize_fill_output(&diag, 1024, 1024);
        assert!(!diag.audio_underrun_active.load(Ordering::Acquire));
        assert_eq!(diag.audio_underrun_end_seq.load(Ordering::Acquire), 1);
        // silence_samples_total は変わらない (= 1 回目の 1024 のまま)
        assert_eq!(
            diag.audio_silence_samples_total.load(Ordering::Acquire),
            1024
        );
    }

    #[test]
    fn finalize_fill_output_no_edge_when_state_unchanged() {
        let diag = make_diag();

        // 連続 full drain (= 通常運転)
        finalize_fill_output(&diag, 1024, 1024);
        finalize_fill_output(&diag, 1024, 1024);
        finalize_fill_output(&diag, 1024, 1024);
        // begin/end 共に変化なし
        assert_eq!(diag.audio_underrun_begin_seq.load(Ordering::Acquire), 0);
        assert_eq!(diag.audio_underrun_end_seq.load(Ordering::Acquire), 0);

        // 連続 underrun (= begin edge は最初の 1 回だけ、その後は seq 不変)
        finalize_fill_output(&diag, 1024, 0);
        finalize_fill_output(&diag, 1024, 0);
        finalize_fill_output(&diag, 1024, 0);
        assert_eq!(diag.audio_underrun_begin_seq.load(Ordering::Acquire), 1);
        // 累積 silence は毎回 +1024 加算
        assert_eq!(
            diag.audio_silence_samples_total.load(Ordering::Acquire),
            1024 * 3
        );
    }

    #[test]
    fn finalize_fill_output_silence_ms_conversion_via_samples_per_sec() {
        // silence_samples を ms に換算するロジックを再現:
        // silence_ms = silence_samples / samples_per_sec * 1000.0
        // (samples_per_sec = sample_rate * 2.0 for stereo interleaved)
        let diag = make_diag();
        let sample_rate = 48_000_u32;
        let samples_per_sec = sample_rate as f64 * 2.0;

        // 50ms 相当: 50ms * 48kHz * 2ch = 4800 samples
        finalize_fill_output(&diag, 4800, 0);
        let silence_total = diag.audio_silence_samples_total.load(Ordering::Acquire);
        let silence_ms = (silence_total as f64 / samples_per_sec) * 1000.0;
        assert!(
            (silence_ms - 50.0).abs() < 0.01,
            "expected 50ms, got {silence_ms}ms"
        );
    }

    #[test]
    fn finalize_fill_output_full_drain_from_clean_state_no_edge() {
        // edge は **状態変化** のときだけ。クリーンな状態 (active=false) で full drain
        // (silence=0) を呼んでも end_seq は上がらない (= false → false なら no-op)。
        let diag = make_diag();
        finalize_fill_output(&diag, 1024, 1024);
        assert_eq!(diag.audio_underrun_end_seq.load(Ordering::Acquire), 0);
        assert_eq!(diag.audio_underrun_begin_seq.load(Ordering::Acquire), 0);
        assert!(!diag.audio_underrun_active.load(Ordering::Acquire));
    }
}
