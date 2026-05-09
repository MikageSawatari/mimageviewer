//! 動画・音声デコーダ worker。
//!
//! 1 動画につき 1 スレッド。`std::thread::spawn` で起動して bounded mpsc にフレームを流す。
//! UI スレッドは [`crate::video::VideoPlayer`] 経由でフレームを取り出す。
//!
//! ## 設計
//! - **動画**: `swscale` で BGRA に変換し [`VideoFrame`] として送出。
//! - **音声**: `swresample` で f32 stereo / 48kHz に変換し [`AudioFrame`] として送出。
//! - **クロック**: [`crate::video::clock::AvClock`] を共有。音声出力スレッドが
//!   `set_audio_pts` を呼び、ここでは `take_seek_request` を見るだけ。
//! - **シーク**: `AvClock::take_seek_request` で要求を pull し、`av_seek_frame` →
//!   デコーダ flush → 目標 PTS まで進めた最初のフレームに `seek_serial` を載せて送出。
//!   UI 側は古い serial のフレームを破棄する。
//! - **キャンセル**: `cancel: Arc<AtomicBool>` を毎ループ確認。Drop 時に true。
//!
//! ## ffmpeg-the-third のバージョン依存性
//! このファイルは `ffmpeg-the-third = "3"` 系を想定して書いている。
//! 2 系・4 系では `ChannelLayout` 周りの API が変わっているので、
//! `cargo check` で型エラーが出たらそこを最初に疑うこと。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded};

use super::clock::AvClock;

const AUDIO_PACKET_QUEUE_CAP: usize = 256;
const VIDEO_PACKET_QUEUE_CAP: usize = 256;
const VIDEO_PACKET_OVERFLOW_MAX_BYTES: usize = 64 * 1024 * 1024;
const DEMUX_PACKET_SEND_WAIT_WARN_MS: f64 = 20.0;
const DEMUX_PACKET_SEND_TIMEOUT_MS: u64 = 2;
const VIDEO_PACING_SLEEP_MS: u64 = 1;

fn sleep_video_pacing() {
    std::thread::sleep(std::time::Duration::from_millis(VIDEO_PACING_SLEEP_MS));
}

const EAGAIN_ERRNO: i32 = 11;

fn engine_state_code_name(code: u8) -> &'static str {
    match code {
        crate::video::engine::actor::state_code::IDLE => "Idle",
        crate::video::engine::actor::state_code::LOADING => "Loading",
        crate::video::engine::actor::state_code::BUFFERING => "Buffering",
        crate::video::engine::actor::state_code::PLAYING => "Playing",
        crate::video::engine::actor::state_code::PAUSED => "Paused",
        crate::video::engine::actor::state_code::SEEKING => "Seeking",
        crate::video::engine::actor::state_code::EOF => "Eof",
        _ => "Unknown",
    }
}

fn engine_state_parks_decode(code: u8) -> bool {
    code == crate::video::engine::actor::state_code::PAUSED
        || code == crate::video::engine::actor::state_code::EOF
}

// Preserve bounded-channel back-pressure during normal playback while still
// letting fullscreen close/cancel cut through a full demux packet queue quickly.
fn send_demux_msg_cancel_aware<T>(
    tx: &Sender<T>,
    mut msg: T,
    cancel: &AtomicBool,
    stream: &'static str,
    msg_kind: &'static str,
    queue_cap: usize,
) -> bool {
    let wait_started = std::time::Instant::now();
    let mut last_wait_log = wait_started;
    loop {
        if cancel.load(Ordering::Acquire) {
            return false;
        }
        match tx.send_timeout(
            msg,
            std::time::Duration::from_millis(DEMUX_PACKET_SEND_TIMEOUT_MS),
        ) {
            Ok(()) => return true,
            Err(SendTimeoutError::Disconnected(_)) => return false,
            Err(SendTimeoutError::Timeout(returned)) => {
                msg = returned;
                let now = std::time::Instant::now();
                let waited = now.duration_since(wait_started);
                if waited >= std::time::Duration::from_millis(250)
                    && now.duration_since(last_wait_log) >= std::time::Duration::from_millis(250)
                {
                    last_wait_log = now;
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "demux",
                            "control_send_waiting",
                            None,
                            0,
                            &[
                                ("stream", serde_json::Value::from(stream)),
                                ("message", serde_json::Value::from(msg_kind)),
                                (
                                    "wait_ms",
                                    serde_json::Value::from(waited.as_secs_f64() * 1000.0),
                                ),
                                ("queue_len", serde_json::Value::from(tx.len() as i64)),
                                ("queue_cap", serde_json::Value::from(queue_cap as i64)),
                            ],
                        );
                    }
                    if waited >= std::time::Duration::from_secs(1) {
                        crate::logger::log(format!(
                            "[demux] {stream} {msg_kind} send still waiting {:.1}ms queue_len={}/{}",
                            waited.as_secs_f64() * 1000.0,
                            tx.len(),
                            queue_cap
                        ));
                    }
                }
            }
        }
    }
}

enum DemuxPacketSend {
    Sent,
    Cancelled,
    SeekPending,
}

// Packet sends are back-pressure points, but seek/flush is control traffic.
// If a seek arrives while a packet queue is full, dropping the old packet and
// returning to the demux loop lets the Flush marker reach decoders promptly.
fn send_demux_packet_seek_aware<T>(
    tx: &Sender<T>,
    mut msg: T,
    clock: &AvClock,
    cancel: &AtomicBool,
    stream: &'static str,
    queue_cap: usize,
) -> DemuxPacketSend {
    let wait_started = std::time::Instant::now();
    let mut last_wait_log = wait_started;
    loop {
        if cancel.load(Ordering::Acquire) {
            return DemuxPacketSend::Cancelled;
        }
        if clock.peek_seek_request_pending() {
            return DemuxPacketSend::SeekPending;
        }
        match tx.send_timeout(
            msg,
            std::time::Duration::from_millis(DEMUX_PACKET_SEND_TIMEOUT_MS),
        ) {
            Ok(()) => return DemuxPacketSend::Sent,
            Err(SendTimeoutError::Disconnected(_)) => return DemuxPacketSend::Cancelled,
            Err(SendTimeoutError::Timeout(returned)) => {
                msg = returned;
                let now = std::time::Instant::now();
                let waited = now.duration_since(wait_started);
                if waited >= std::time::Duration::from_millis(250)
                    && now.duration_since(last_wait_log) >= std::time::Duration::from_millis(250)
                {
                    last_wait_log = now;
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "demux",
                            "packet_send_waiting",
                            None,
                            0,
                            &[
                                ("stream", serde_json::Value::from(stream)),
                                (
                                    "wait_ms",
                                    serde_json::Value::from(waited.as_secs_f64() * 1000.0),
                                ),
                                ("queue_len", serde_json::Value::from(tx.len() as i64)),
                                ("queue_cap", serde_json::Value::from(queue_cap as i64)),
                                (
                                    "seek_serial",
                                    serde_json::Value::from(clock.current_seek_serial() as i64),
                                ),
                            ],
                        );
                    }
                    if waited >= std::time::Duration::from_secs(1) {
                        crate::logger::log(format!(
                            "[demux] {stream} packet send still waiting {:.1}ms queue_len={}/{} seek_serial={}",
                            waited.as_secs_f64() * 1000.0,
                            tx.len(),
                            queue_cap,
                            clock.current_seek_serial()
                        ));
                    }
                }
            }
        }
    }
}

// Audio packet back-pressure must not strand already-demuxed video packets.
// During precise seek preroll the demux thread may have enough video packets in
// pending_video_packets to reach the target, while the next audio packet send is
// blocked by the frozen Buffering audio pipeline. Drain the video overflow on
// each audio send timeout so FirstFrameReady can still be produced.
fn send_audio_packet_with_video_drain(
    audio_pkt_tx: &Sender<AudioWorkerMsg>,
    mut msg: AudioWorkerMsg,
    clock: &AvClock,
    cancel: &AtomicBool,
    pending_video_packets: &mut VecDeque<QueuedVideoPacket>,
    pending_video_packet_bytes: &mut usize,
    video_pkt_tx: &Sender<VideoWorkerMsg>,
) -> DemuxPacketSend {
    let wait_started = std::time::Instant::now();
    let mut last_wait_log = wait_started;
    loop {
        if cancel.load(Ordering::Acquire) {
            return DemuxPacketSend::Cancelled;
        }
        if clock.peek_seek_request_pending() {
            return DemuxPacketSend::SeekPending;
        }
        match audio_pkt_tx.send_timeout(
            msg,
            std::time::Duration::from_millis(DEMUX_PACKET_SEND_TIMEOUT_MS),
        ) {
            Ok(()) => return DemuxPacketSend::Sent,
            Err(SendTimeoutError::Disconnected(_)) => return DemuxPacketSend::Cancelled,
            Err(SendTimeoutError::Timeout(returned)) => {
                msg = returned;
                if !pending_video_packets.is_empty() {
                    let before_packets = pending_video_packets.len();
                    let before_bytes = *pending_video_packet_bytes;
                    if !drain_pending_video_packets(
                        pending_video_packets,
                        pending_video_packet_bytes,
                        video_pkt_tx,
                        clock,
                        cancel,
                    ) {
                        return DemuxPacketSend::Cancelled;
                    }
                    let after_packets = pending_video_packets.len();
                    if after_packets < before_packets && crate::perf::is_enabled() {
                        crate::perf::event(
                            "demux",
                            "audio_wait_video_drain",
                            None,
                            0,
                            &[
                                (
                                    "drained_packets",
                                    serde_json::Value::from(
                                        (before_packets - after_packets) as i64,
                                    ),
                                ),
                                (
                                    "before_packets",
                                    serde_json::Value::from(before_packets as i64),
                                ),
                                (
                                    "after_packets",
                                    serde_json::Value::from(after_packets as i64),
                                ),
                                ("before_bytes", serde_json::Value::from(before_bytes as i64)),
                                (
                                    "after_bytes",
                                    serde_json::Value::from(*pending_video_packet_bytes as i64),
                                ),
                                (
                                    "video_pkt_tx_len",
                                    serde_json::Value::from(video_pkt_tx.len() as i64),
                                ),
                                (
                                    "seek_serial",
                                    serde_json::Value::from(clock.current_seek_serial() as i64),
                                ),
                            ],
                        );
                    }
                }

                let now = std::time::Instant::now();
                let waited = now.duration_since(wait_started);
                if waited >= std::time::Duration::from_millis(250)
                    && now.duration_since(last_wait_log) >= std::time::Duration::from_millis(250)
                {
                    last_wait_log = now;
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "demux",
                            "packet_send_waiting",
                            None,
                            0,
                            &[
                                ("stream", serde_json::Value::from("audio")),
                                (
                                    "wait_ms",
                                    serde_json::Value::from(waited.as_secs_f64() * 1000.0),
                                ),
                                (
                                    "queue_len",
                                    serde_json::Value::from(audio_pkt_tx.len() as i64),
                                ),
                                (
                                    "queue_cap",
                                    serde_json::Value::from(AUDIO_PACKET_QUEUE_CAP as i64),
                                ),
                                (
                                    "seek_serial",
                                    serde_json::Value::from(clock.current_seek_serial() as i64),
                                ),
                            ],
                        );
                    }
                    if waited >= std::time::Duration::from_secs(1) {
                        crate::logger::log(format!(
                            "[demux] audio packet send still waiting {:.1}ms queue_len={}/{} seek_serial={}",
                            waited.as_secs_f64() * 1000.0,
                            audio_pkt_tx.len(),
                            AUDIO_PACKET_QUEUE_CAP,
                            clock.current_seek_serial()
                        ));
                    }
                }
            }
        }
    }
}

fn frame_best_effort_timestamp(raw: *const ffmpeg_the_third::ffi::AVFrame) -> Option<i64> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: callers pass a raw pointer obtained from a live FFmpeg frame.
    // We only read the timestamp field and do not retain the pointer.
    let ts = unsafe { (*raw).best_effort_timestamp };
    if ts == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
        None
    } else {
        Some(ts)
    }
}

fn video_frame_timestamp(frame: &ffmpeg_the_third::util::frame::Video) -> Option<i64> {
    // SAFETY: ffmpeg-the-third exposes the raw AVFrame pointer through an
    // unsafe method; the frame is borrowed and alive for this call.
    let raw = unsafe { frame.as_ptr() };
    frame_best_effort_timestamp(raw).or_else(|| frame.pts())
}

fn audio_frame_timestamp(frame: &ffmpeg_the_third::util::frame::audio::Audio) -> Option<i64> {
    // SAFETY: ffmpeg-the-third exposes the raw AVFrame pointer through an
    // unsafe method; the frame is borrowed and alive for this call.
    let raw = unsafe { frame.as_ptr() };
    frame_best_effort_timestamp(raw).or_else(|| frame.pts())
}

fn sane_video_rate(rate: ffmpeg_the_third::Rational) -> Option<(i32, i32)> {
    if rate.numerator() > 0 && rate.denominator() > 0 {
        Some((rate.numerator(), rate.denominator()))
    } else {
        None
    }
}

fn selected_video_rate(
    avg_rate: ffmpeg_the_third::Rational,
    stream_rate: ffmpeg_the_third::Rational,
) -> Option<(i32, i32)> {
    sane_video_rate(avg_rate).or_else(|| sane_video_rate(stream_rate))
}

fn packet_timestamp(packet: &ffmpeg_the_third::Packet) -> Option<i64> {
    packet.pts().or_else(|| packet.dts())
}

/// Phase B (3-thread split): demux thread から video decode thread に流すメッセージ。
///
/// Phase A で audio decode を分離した後も、demux と video decode は同じスレッドで
/// 動いていた。video decode 中に I/O bound な demux が止まると、(1) 音声 packet も
/// 流れず audio decode 待機が広がる、(2) HDD random read のスパイクが video decode
/// 経路の中で吸収されない、という二次的なストールが残る。Phase B は demux も独立
/// スレッドにして video decode と並行動作させる。
///
/// `Flush` と `Eof` は `AudioWorkerMsg` と同じセマンティクスで、順序保証は channel
/// が担保する。`Packet` には `seek_serial` を含める。video decode thread は `Flush`
/// 受領でローカル serial を更新しつつ、clock の live serial が先に進んだ場合も古い
/// packet を捨てる。direct queue は shallow に保ち、Flush を古い packet の後ろへ
/// 深く埋めない設計にしているため、live serial drop でも大きな seek gap を作らない。
enum VideoWorkerMsg {
    /// avformat から取り出した未デコード動画 packet。video decode thread が
    /// `send_packet` → `receive_frame` → (GPU blit / swscale) → pacing →
    /// `video_tx.try_send` を行う。
    Packet {
        serial: u64,
        packet: ffmpeg_the_third::Packet,
    },
    /// シーク完了通知。video decode thread はこれを受けて自分の avcodec デコーダ
    /// を `flush()` し、`current_seek_serial` を更新する。
    ///
    /// **2 つの target を分離管理** (Codex P1 助言、2026-05-01):
    /// - `seek_target_secs`: ユーザー要求 seek 位置 (= timeline 表示位置の意図)。
    ///   Fast モードでは keyframe pts ではなく target を保つことで、Buffering→Playing
    ///   入場時の anchor が target に維持される (Fast でも timeline は target 固定)。
    ///   `None` のときは seek 失敗 / 非 seek flush。
    /// - `trim_before_secs`: post-seek preroll trim 用。`Some(t)` で受信側は
    ///   `drop_before_secs = Some(t)` を設定し target ぴったりに着地。`None` で
    ///   trim をスキップ (= Fast backward の即時再生 / seek 失敗)。
    ///
    /// 旧版は `target_secs: Option<f64>` 単一フィールドで両方の役割を兼ねていたが、
    /// Fast モードで「trim 不要だが target 情報は必要」を表現できなかったため分離。
    Flush {
        serial: u64,
        /// video decode thread では現状未参照 (= pump 経由で audio 側が消費)。
        /// 将来 video 側でも post-seek 表示同期に使う可能性があるためフィールドとして保持。
        #[allow(dead_code)]
        seek_target_secs: Option<f64>,
        trim_before_secs: Option<f64>,
    },
    /// EOF 到達通知。video decode thread は何もせずに次の `Packet` か `Flush`
    /// か channel disconnect を待つ (旧 `run_decoder` の挙動と同じく、内部
    /// 残フレームは EOF で失われる)。
    Eof,
}

struct QueuedVideoPacket {
    serial: u64,
    packet: ffmpeg_the_third::Packet,
    pts_secs: Option<f64>,
    size_bytes: usize,
}

fn emit_demux_packet_send_wait(
    stream: &'static str,
    wait_ms: f64,
    queue_len_before: usize,
    queue_cap: usize,
    packet_pts: Option<f64>,
    seek_serial: u64,
) {
    crate::logger::log(format!(
        "[demux] {stream} packet send waited {wait_ms:.1}ms queue_len_before={queue_len_before}/{queue_cap} pts={}",
        packet_pts
            .map(|pts| format!("{pts:.3}"))
            .unwrap_or_else(|| "-".to_string())
    ));
    if crate::perf::is_enabled() {
        crate::perf::event(
            "demux",
            "packet_send_wait",
            None,
            0,
            &[
                ("stream", serde_json::Value::from(stream)),
                ("wait_ms", serde_json::Value::from(wait_ms)),
                (
                    "queue_len_before",
                    serde_json::Value::from(queue_len_before as i64),
                ),
                ("queue_cap", serde_json::Value::from(queue_cap as i64)),
                (
                    "packet_pts",
                    packet_pts
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("seek_serial", serde_json::Value::from(seek_serial as i64)),
            ],
        );
    }
}

fn emit_video_packet_overflow_queued(
    pending_packets: usize,
    pending_bytes: usize,
    queue_len_before: usize,
    packet_pts: Option<f64>,
    seek_serial: u64,
) {
    crate::logger::log(format!(
        "[demux] video packet overflow queued pending={pending_packets} bytes={pending_bytes} queue_len_before={queue_len_before}/{VIDEO_PACKET_QUEUE_CAP} pts={}",
        packet_pts
            .map(|pts| format!("{pts:.3}"))
            .unwrap_or_else(|| "-".to_string())
    ));
    if crate::perf::is_enabled() {
        crate::perf::event(
            "demux",
            "video_packet_overflow_queued",
            None,
            0,
            &[
                (
                    "pending_packets",
                    serde_json::Value::from(pending_packets as i64),
                ),
                (
                    "pending_bytes",
                    serde_json::Value::from(pending_bytes as i64),
                ),
                (
                    "queue_len_before",
                    serde_json::Value::from(queue_len_before as i64),
                ),
                (
                    "queue_cap",
                    serde_json::Value::from(VIDEO_PACKET_QUEUE_CAP as i64),
                ),
                (
                    "packet_pts",
                    packet_pts
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("seek_serial", serde_json::Value::from(seek_serial as i64)),
            ],
        );
    }
}

// HwDevice (= AVBufferRef のラッパー) を別スレッドに move するため Send を実装する。
// FFmpeg の av_buffer_ref / av_buffer_unref は内部で atomic refcount を使っているので、
// 異なるスレッドからの ref/unref は安全 (Sync は不要 = 1 thread が排他所有する形で使う)。
unsafe impl Send for HwDevice {}

fn pending_video_pts_span_ms(pending_video_packets: &VecDeque<QueuedVideoPacket>) -> Option<f64> {
    let first = pending_video_packets.front()?.pts_secs?;
    let last = pending_video_packets.back()?.pts_secs?;
    let span_ms = (last - first) * 1000.0;
    if span_ms.is_finite() && span_ms >= 0.0 {
        Some(span_ms)
    } else {
        None
    }
}

fn emit_demux_drain_full_hit(
    pending_video_packets: &VecDeque<QueuedVideoPacket>,
    pending_video_packet_bytes: usize,
    video_pkt_tx_len: usize,
    seek_serial: u64,
) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            "demux",
            "drain_full_hit",
            None,
            0,
            &[
                (
                    "pending_video_packets",
                    serde_json::Value::from(pending_video_packets.len() as i64),
                ),
                (
                    "pending_video_bytes",
                    serde_json::Value::from(pending_video_packet_bytes as i64),
                ),
                (
                    "video_pkt_tx_len",
                    serde_json::Value::from(video_pkt_tx_len as i64),
                ),
                (
                    "pending_video_pts_span_ms",
                    pending_video_pts_span_ms(pending_video_packets)
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("seek_serial", serde_json::Value::from(seek_serial as i64)),
            ],
        );
    }
}

fn emit_demux_queue_state(
    pending_video_packets: &VecDeque<QueuedVideoPacket>,
    pending_video_packet_bytes: usize,
    pending_video_peak_packets: usize,
    pending_video_peak_bytes: usize,
    video_pkt_tx: &Sender<VideoWorkerMsg>,
    audio_pkt_tx: &Sender<AudioWorkerMsg>,
) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            "demux",
            "queue_state",
            None,
            0,
            &[
                (
                    "pending_video_packets",
                    serde_json::Value::from(pending_video_packets.len() as i64),
                ),
                (
                    "pending_video_bytes",
                    serde_json::Value::from(pending_video_packet_bytes as i64),
                ),
                (
                    "pending_video_peak_packets",
                    serde_json::Value::from(pending_video_peak_packets as i64),
                ),
                (
                    "pending_video_peak_bytes",
                    serde_json::Value::from(pending_video_peak_bytes as i64),
                ),
                (
                    "video_pkt_tx_len",
                    serde_json::Value::from(video_pkt_tx.len() as i64),
                ),
                (
                    "audio_pkt_tx_len",
                    serde_json::Value::from(audio_pkt_tx.len() as i64),
                ),
                (
                    "pending_video_pts_span_ms",
                    pending_video_pts_span_ms(pending_video_packets)
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                ),
            ],
        );
    }
}

struct BwdifFilter {
    graph: ffmpeg_the_third::filter::Graph,
    key: BwdifFilterKey,
    force_all_frames: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BwdifFilterKey {
    pix_fmt: ffmpeg_the_third::format::Pixel,
    width: u32,
    height: u32,
    sar_num: i32,
    sar_den: i32,
    time_base_num: i32,
    time_base_den: i32,
}

impl BwdifFilter {
    fn new(key: BwdifFilterKey, force_all_frames: bool) -> Result<Self, String> {
        use ffmpeg_the_third as ffmpeg;

        let buffer = ffmpeg::filter::find("buffer").ok_or("FFmpeg filter 'buffer' not found")?;
        let buffersink =
            ffmpeg::filter::find("buffersink").ok_or("FFmpeg filter 'buffersink' not found")?;
        let pix_fmt = Into::<ffmpeg::ffi::AVPixelFormat>::into(key.pix_fmt) as i32;
        let args = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
            key.width,
            key.height,
            pix_fmt,
            key.time_base_num.max(1),
            key.time_base_den.max(1),
            key.sar_num.max(1),
            key.sar_den.max(1)
        );

        let mut graph = ffmpeg::filter::Graph::new();
        graph
            .add(&buffer, "in", &args)
            .map_err(|e| format!("bwdif buffer init: {e}"))?;
        {
            let mut sink = graph
                .add(&buffersink, "out", "")
                .map_err(|e| format!("bwdif buffersink init: {e}"))?;
            sink.set_pixel_format(key.pix_fmt);
        }

        let deint = if force_all_frames {
            "bwdif=mode=send_frame:parity=auto:deint=all"
        } else {
            "bwdif=mode=send_frame:parity=auto:deint=interlaced"
        };
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(deint))
            .map_err(|e| format!("bwdif graph parse: {e}"))?;
        graph
            .validate()
            .map_err(|e| format!("bwdif graph validate: {e}"))?;

        Ok(Self {
            graph,
            key,
            force_all_frames,
        })
    }

    fn matches(&self, key: BwdifFilterKey, force_all_frames: bool) -> bool {
        self.key == key && self.force_all_frames == force_all_frames
    }

    fn filter_one(
        &mut self,
        frame: &ffmpeg_the_third::util::frame::video::Video,
    ) -> Result<Option<ffmpeg_the_third::util::frame::video::Video>, String> {
        use ffmpeg_the_third as ffmpeg;

        let input = frame.clone();
        {
            let mut src_ctx = self
                .graph
                .get("in")
                .ok_or_else(|| "bwdif source context missing".to_string())?;
            src_ctx
                .source()
                .add(&input)
                .map_err(|e| format!("bwdif source add: {e}"))?;
        }
        drop(input);

        let mut output = ffmpeg::util::frame::video::Video::empty();
        {
            let mut sink_ctx = self
                .graph
                .get("out")
                .ok_or_else(|| "bwdif sink context missing".to_string())?;
            match sink_ctx.sink().frame(&mut output) {
                Ok(()) => Ok(Some(output)),
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN_ERRNO => Ok(None),
                Err(ffmpeg::Error::Eof) => Ok(None),
                Err(e) => Err(format!("bwdif sink frame: {e}")),
            }
        }
    }
}

fn bwdif_filter_key(
    frame: &ffmpeg_the_third::util::frame::video::Video,
    time_base_num: i32,
    time_base_den: i32,
) -> BwdifFilterKey {
    let sar = frame.aspect_ratio();
    let sar_num = sar.numerator();
    let sar_den = sar.denominator();
    BwdifFilterKey {
        pix_fmt: frame.format(),
        width: frame.width(),
        height: frame.height(),
        sar_num: if sar_num > 0 { sar_num } else { 1 },
        sar_den: if sar_den > 0 { sar_den } else { 1 },
        time_base_num: time_base_num.max(1),
        time_base_den: time_base_den.max(1),
    }
}

fn field_order_is_interlaced(field_order: ffmpeg_the_third::FieldOrder) -> bool {
    matches!(
        field_order,
        ffmpeg_the_third::FieldOrder::TT
            | ffmpeg_the_third::FieldOrder::BB
            | ffmpeg_the_third::FieldOrder::TB
            | ffmpeg_the_third::FieldOrder::BT
    )
}

fn should_try_deinterlace(
    mode: crate::settings::VideoDeinterlaceMode,
    frame_interlaced: bool,
    stream_interlaced: bool,
    failure_logged: bool,
) -> bool {
    mode.is_enabled()
        && !failure_logged
        && (mode.force_all_frames() || frame_interlaced || stream_interlaced)
}

fn bwdif_force_all_frames(
    mode: crate::settings::VideoDeinterlaceMode,
    frame_interlaced: bool,
    stream_interlaced: bool,
) -> bool {
    mode.force_all_frames()
        || (mode == crate::settings::VideoDeinterlaceMode::Auto
            && stream_interlaced
            && !frame_interlaced)
}

fn drain_pending_video_packets(
    pending_video_packets: &mut VecDeque<QueuedVideoPacket>,
    pending_video_packet_bytes: &mut usize,
    video_pkt_tx: &Sender<VideoWorkerMsg>,
    clock: &AvClock,
    cancel: &AtomicBool,
) -> bool {
    use crossbeam_channel::TrySendError;

    while let Some(queued) = pending_video_packets.pop_front() {
        if cancel.load(Ordering::Acquire) {
            pending_video_packets.push_front(queued);
            return false;
        }

        let QueuedVideoPacket {
            serial,
            packet,
            pts_secs,
            size_bytes,
        } = queued;

        let live_serial = clock.current_seek_serial();
        if serial != live_serial {
            *pending_video_packet_bytes = pending_video_packet_bytes.saturating_sub(size_bytes);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "demux",
                    "video_packet_overflow_stale_discarded",
                    None,
                    0,
                    &[
                        ("packet_serial", serde_json::Value::from(serial as i64)),
                        ("live_serial", serde_json::Value::from(live_serial as i64)),
                        (
                            "pending_packets",
                            serde_json::Value::from(pending_video_packets.len() as i64),
                        ),
                    ],
                );
            }
            continue;
        }

        match video_pkt_tx.try_send(VideoWorkerMsg::Packet { serial, packet }) {
            Ok(()) => {
                *pending_video_packet_bytes = pending_video_packet_bytes.saturating_sub(size_bytes);
            }
            Err(TrySendError::Full(VideoWorkerMsg::Packet {
                serial: ret_serial,
                packet: ret_packet,
            })) => {
                pending_video_packets.push_front(QueuedVideoPacket {
                    serial: ret_serial,
                    packet: ret_packet,
                    pts_secs,
                    size_bytes,
                });
                emit_demux_drain_full_hit(
                    pending_video_packets,
                    *pending_video_packet_bytes,
                    video_pkt_tx.len(),
                    ret_serial,
                );
                break;
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    true
}

/// Phase A (3-thread split): demux + video decode thread から audio decode thread に
/// 流すメッセージ。
///
/// 旧構造では `run_decoder` が音声 packet を受け取った時点で `send_packet` →
/// `receive_frame` → `swresample` → `audio_tx.send` まで実行していたが、
/// `audio_tx` (bounded=32) が満杯のときに `audio_tx.send` がブロックすると、
/// 同じスレッドで動いている動画 demux/decode も停止してしまい、
/// `video_tx` (bounded=24) が空 → UI 側 `buf 0/24` が頻発していた。
///
/// Phase A では音声 decode を独立スレッドに切り出し、demux 側は `Packet` を
/// `audio_pkt_tx` に enqueue するだけにする。`audio_pkt_tx` (small bounded queue) が満杯に
/// なっても demux スレッドが一時停止するだけで、video decode は別経路でそのまま
/// 進行する (Phase B で demux も video decode から分離予定)。
///
/// `Flush` / `Eof` は順序保証のため packet と同じ channel に enqueue する
/// (Mutex + 別チャネルだと「Flush 通知より後に届いた packet が前世代として
/// decode される」race が起きる)。
///
/// ただし seek 要求後、Flush marker が bounded queue の奥に滞留している間に旧世代の
/// audio packet を再生してしまうと、master clock 自体が旧位置で進む。audio worker は
/// packet/frame 送出時に live seek serial も確認し、Flush 到達前でも旧世代の音を捨てる。
enum AudioWorkerMsg {
    /// avformat から取り出した未デコード音声 packet。audio decode thread が
    /// `send_packet` → `receive_frame` → resample → `audio_tx.send` を行う。
    Packet {
        serial: u64,
        packet: ffmpeg_the_third::Packet,
    },
    /// シーク完了通知。audio decode thread はこれを受けたら自分の avcodec デコーダ
    /// を `flush()` する。
    ///
    /// **2 つの target を分離管理** (= [`VideoWorkerMsg::Flush`] と同じ理由):
    /// - `seek_target_secs`: pump 経由で BufferReady の audio_anchor pts に伝搬。
    ///   Fast モードでは keyframe pts ではなく target を維持する。
    /// - `trim_before_secs`: `drop_before_secs` (= preroll 切り捨て下限) として保持。
    ///   `None` で trim をスキップ。
    Flush {
        serial: u64,
        seek_target_secs: Option<f64>,
        trim_before_secs: Option<f64>,
    },
    /// EOF 到達通知。audio decode thread は内部 decoder を flush して残フレームを
    /// drain (= 末尾の音声を出し切る)、その後次の `Flush` か `Packet` か
    /// channel disconnect を待つ。
    Eof,
}

/// 1 動画フレーム。CPU readback (旧経路) と GPU 共有テクスチャ (新経路) の二択。
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: VideoFrameData,
    /// 提示時刻 (秒)。AvClock との比較に使う。
    pub pts_secs: f64,
    /// シーク世代。これが現行の AvClock seek_serial と異なれば UI は捨てる。
    pub seek_serial: u64,
}

/// 動画フレームのピクセルデータ。
pub enum VideoFrameData {
    /// CPU 上の RGBA8 (CPU 経路: SW デコード or HW + av_hwframe_transfer_data + swscale)。
    /// `Vec<u8>` は `width * height * 4` バイト。
    Cpu(Vec<u8>),
    /// GPU 上の D3D11 NT 共有テクスチャ (GPU 経路: HW + VideoProcessorBlt → RGBA shared)。
    /// UI は `import_shared_d3d11_texture` で wgpu::Texture に import して描画する。
    /// テクスチャの寿命管理は `D3d11Frame` が `Drop` で `CloseHandle` する責務を持つ。
    #[cfg(windows)]
    Gpu(crate::video::gpu_renderer::D3d11Frame),
}

impl VideoFrameData {
    /// 旧来の CPU bgra アクセス互換 (call sites の段階的移行用)。
    /// GPU フレームでは `None` を返す。
    #[allow(dead_code)]
    pub fn cpu_bgra(&self) -> Option<&[u8]> {
        match self {
            Self::Cpu(b) => Some(b.as_slice()),
            #[cfg(windows)]
            Self::Gpu(_) => None,
        }
    }
}

/// 1 音声フレーム (interleaved stereo f32、48kHz)。
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub pts_secs: f64,
    pub seek_serial: u64,
    /// このフレーム分の元音声の再生時間 (source timeline 秒)。
    pub duration_secs: f64,
    /// decoder→pump 間の tx queue 会計に使う wall 秒。enqueue 時の playback speed で
    /// 固定し、pump 受信時も同じ値を減算する。
    pub queued_wall_secs: f64,
    /// `queued_wall_secs` を加算した時点の会計世代。速度変更で世代が進んだ後の
    /// 旧 frame は音声としては使うが、tx queue 会計からは除外する。
    pub audio_tx_accounting_epoch: u64,
    /// この `seek_serial` におけるユーザー要求 seek 位置 (秒)。
    ///
    /// **背景 (Codex P1 修正、2026-05-01)**: pump 側で BufferReady の audio_anchor を
    /// target ベースで報告するため、demux Flush 経由で受け取った target 値を audio
    /// decode thread が **同じ seek_serial の全 frame に焼き付けて** 伝搬する
    /// (= 1-shot ではなく persistent: pump がどのタイミングで frame を観測しても
    /// target を取り出せる)。
    ///
    /// **2 巡目 fix で audio は Fast でも target まで trim** するようになったため、
    /// `pts_secs` ≈ target で frame が emit される (= 旧設計の「keyframe pts と target
    /// が乖離」状態は audio 側では発生しない)。それでも `audible_pts.max(target)` の
    /// max 演算は安全側として保持している (= PDC > 0 の場合 audible_pts < target が
    /// 発生しうる、初期 open の audio anchor 等のケース)。
    ///
    /// 値の意味:
    /// - `Some(target)`: seek が走った世代の frame。pump は BufferReady pts の下限として
    ///   max 演算する (= audible_pts.max(target))。
    /// - `None`: 非 seek flush (= 失敗 or 初期 open)。pump は audible_pts を素朴に使う。
    pub seek_target_secs: Option<f64>,
}

/// デコード開始時に分かる動画情報。UI の HUD で利用。
#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    /// Stream codec id name (h264 / hevc / av1 / vp9 ...).
    pub video_codec: String,
    /// FFmpeg decoder selected by avcodec_find_decoder/open_as.
    pub video_decoder: String,
    /// Whether the selected/default decoder advertises D3D11VA output.
    pub d3d11va_supported: bool,
    /// Compact debug summary of the decoder's D3D11VA-capable HW configs.
    pub d3d11va_config: String,
    pub audio_codec: Option<String>,
    pub has_audio: bool,
    /// HW デコードが実際に有効化されたか (sw / hw_d3d11va)。
    pub hw_decode_active: bool,
    /// GPU 経路 (D3D11 video processor blit) が利用可能か。
    /// 第 1 フレーム到着時に確定。`false` の間は CPU readback 経路。
    pub gpu_path_active: bool,
    /// 動画ファイルに埋め込まれた標準メタデータ (Phase 5.4)。
    /// FFmpeg avformat が解釈できる形式 (Matroska tags / MP4 udta / ffmetadata) を
    /// 想定。値が無いキーは None。
    pub title: Option<String>,
    pub artist: Option<String>,
    /// Original webpage URL embedded by external tools (`PURL`,
    /// `webpage_url`, etc.). Only HTTP(S) URLs are surfaced to the UI.
    pub original_url: Option<String>,
    pub description: Option<String>,
    /// 平均フレームレート (Phase 5.4 の右パネル表示用)。
    pub avg_fps: f64,
    /// 平均ビットレート (bps、Phase 6 の上ホバーバー表示用)。0 のときは未知。
    /// `AVFormatContext.bit_rate` をそのまま流す。
    pub bit_rate_bps: i64,
    /// 埋め込みチャプター (Phase 5.4)。`AVChapter*` 配列を時間秒単位で 1 度だけ
    /// 抽出して保持する。空配列ならチャプターは無し。
    pub chapters: Vec<Chapter>,
}

/// 埋め込みチャプター 1 件分。`AVChapter` の `start`/`end` を `time_base` で秒に
/// 変換済の値を持つ。
#[derive(Clone, Debug)]
pub struct Chapter {
    pub start_secs: f64,
    pub end_secs: f64,
    pub title: Option<String>,
}

pub struct DecodeHandles {
    /// 動画フレーム受信。容量 4 (UI 1 フレーム/リフレッシュで十分)。
    pub video_rx: crossbeam_channel::Receiver<VideoFrame>,
    /// 音声フレーム受信。容量 32 (~1 秒分のバッファ余地)。
    pub audio_rx: crossbeam_channel::Receiver<AudioFrame>,
    /// 動画情報の単発通知 (open 完了時)。
    pub info_rx: crossbeam_channel::Receiver<Result<VideoInfo, String>>,
}

/// デコーダワーカーを起動する。ファイルオープン (`avformat_open_input`) も worker
/// thread 内で行うので、UI スレッドはこの関数を呼ぶだけで即座に返る。
///
/// `target_audio_sample_rate` は音声出力デバイスのサンプルレート (cpal 側で決定)。
/// 通常 48000。
///
/// `hw_decode` が true なら D3D11VA HW デコードを試行する。コーデック非対応 / デバイス
/// 初期化失敗 / get_format で SW 形式が返った場合は **黙って SW にフォールバック**
/// する (perf log の `video.open.decode_path` で実際の経路を確認可能)。
/// `engine_state` は `EngineActor::published_state_handle()` で取得した
/// `Arc<AtomicU8>` (Phase 3d)。pacing loop で `state == Playing` のときだけ
/// audio_buf escape を有効化する。
/// `engine_event_tx` (Phase 3e) は decoder thread から SeekCompleted を engine に
/// 通知するために使う。これがないと runtime seek 後 engine が永久 Seeking 状態に
/// 張り付き、pacing escape も解除されない。
pub fn spawn(
    path: PathBuf,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    target_audio_sample_rate: u32,
    hw_decode: bool,
    deinterlace: crate::settings::VideoDeinterlaceMode,
    #[cfg(windows)] gpu_video_device: Option<
        std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
    >,
    engine_state: Arc<std::sync::atomic::AtomicU8>,
    engine_event_tx: crossbeam_channel::Sender<crate::video::engine::EngineEvent>,
    skipped_frame_count: Arc<std::sync::atomic::AtomicU64>,
) -> DecodeHandles {
    // 60fps 1080p で 8 フレーム = 約 130ms のバッファ。decoder pacing の閾値
    // (100ms) と組み合わせて「pacing 直前に 1-2 フレーム余裕がある」状態を
    // 維持し、vsync 1 周期で取り損ねた分を次周期に displayable な状態で
    // 取れるようにする。bounded(4) では 60fps で常に Full → drop に陥る。
    // Phase 8.J: 8 → 24 に増やす。
    // - GPU 経路: 1 frame = HANDLE+メタのみ、メモリコスト無視
    // - CPU 経路: 1080p RGBA × 24 = 192MB (= 許容範囲)
    // 30fps で 800ms 分 buffer できるので、Phase 8.G で残った micro-burst の
    // 350-400ms stall + HDD random read 100-300ms スパイクの両方を吸収。
    let (video_tx, video_rx) = bounded::<VideoFrame>(24);
    let (audio_tx, audio_rx) = bounded::<AudioFrame>(32);
    let (info_tx, info_rx) = bounded::<Result<VideoInfo, String>>(1);

    std::thread::Builder::new()
        .name("video-demux".into())
        .spawn(move || {
            run_decoder(
                path,
                clock,
                cancel,
                target_audio_sample_rate,
                hw_decode,
                deinterlace,
                #[cfg(windows)]
                gpu_video_device,
                engine_state,
                engine_event_tx,
                video_tx,
                audio_tx,
                info_tx,
                skipped_frame_count,
            );
        })
        .expect("spawn video-decode thread");

    DecodeHandles {
        video_rx,
        audio_rx,
        info_rx,
    }
}

fn run_decoder(
    path: PathBuf,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    target_audio_sample_rate: u32,
    hw_decode_requested: bool,
    deinterlace: crate::settings::VideoDeinterlaceMode,
    #[cfg(windows)] gpu_video_device: Option<
        std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
    >,
    engine_state: Arc<std::sync::atomic::AtomicU8>,
    engine_event_tx: crossbeam_channel::Sender<crate::video::engine::EngineEvent>,
    video_tx: Sender<VideoFrame>,
    audio_tx: Sender<AudioFrame>,
    info_tx: Sender<Result<VideoInfo, String>>,
    skipped_frame_count: Arc<std::sync::atomic::AtomicU64>,
) {
    use ffmpeg_the_third as ffmpeg;
    // Phase B: Pixel / ScaleContext / ScaleFlags / Video は run_video_decode に移管。
    // run_decoder = demux thread はもう video frame を直接触らない。
    use ffmpeg::format::sample::{Sample, Type as SampleType};
    use ffmpeg::media::Type as MediaType;
    use ffmpeg::software::resampling::Context as ResampleContext;

    // ── FFmpeg ライブラリ初期化 ──
    if let Err(e) = ffmpeg::init() {
        let _ = info_tx.send(Err(format!("ffmpeg::init failed: {e}")));
        return;
    }

    // ── ファイルを開く ──
    let mut input = match ffmpeg::format::input(&path) {
        Ok(i) => i,
        Err(e) => {
            let _ = info_tx.send(Err(format!("open input: {e}")));
            return;
        }
    };

    // ── 動画ストリーム選択 ──
    let video_stream = match input.streams().best(MediaType::Video) {
        Some(s) => s,
        None => {
            let _ = info_tx.send(Err("動画ストリームが見つかりません".into()));
            return;
        }
    };
    let video_stream_idx = video_stream.index();
    let video_time_base = video_stream.time_base();
    let video_params = video_stream.parameters();
    let (video_fps_num, video_fps_den) =
        selected_video_rate(video_stream.avg_frame_rate(), video_stream.rate())
            .map(|(n, d)| (n as u32, d as u32))
            .unwrap_or((0u32, 0u32));
    let video_avg_fps = if video_fps_num == 0 || video_fps_den == 0 {
        0.0
    } else {
        video_fps_num as f64 / video_fps_den as f64
    };
    // VPP ContentDesc に渡す raw 分数 (= num/den のまま渡すことで丸め誤差を排除)。
    // 0 の場合は VPP 側で 60/1 にフォールバックされる。
    let video_params_owned = match clone_codec_parameters(&video_params) {
        Ok(p) => p,
        Err(e) => {
            let _ = info_tx.send(Err(format!("video codec parameters clone: {e}")));
            return;
        }
    };
    let video_field_order = video_params_owned.field_order();
    let video_stream_interlaced = field_order_is_interlaced(video_field_order);
    // ── HW デコード初期化 (D3D11VA) ──
    // 失敗時は黙って SW デコードに落ちる。`hw_device` を _hw_device で持って Drop 時に
    // unref されるようにし、AVCodecContext は内部でさらに ref を取るので競合しない。
    //
    // gpu_video_device が利用可能な場合は **mIV 側で作成した D3D11 デバイス** を
    // FFmpeg に渡して共有する (= HW デコーダの NV12 出力と ID3D11VideoProcessor が
    // 同じデバイス上で動き、CreateVideoProcessorInputView で受け渡せる)。
    // gpu_video_device 不在 / 失敗時は従来通り FFmpeg が新デバイスを作成し、
    // 出力は av_hwframe_transfer_data で CPU readback する旧経路。
    let codec_id = video_params_owned.id();
    let stream_codec_name = codec_id.name().to_string();
    let effective_hw_decode_requested = hw_decode_requested;
    let opened_video_result = if effective_hw_decode_requested {
        #[cfg(windows)]
        {
            open_video_decoder_with_candidates(
                &video_params_owned,
                codec_id,
                true,
                gpu_video_device.as_ref(),
            )
        }
        #[cfg(not(windows))]
        {
            open_video_decoder_with_candidates(&video_params_owned, codec_id, true)
        }
    } else {
        #[cfg(windows)]
        {
            open_video_decoder_with_candidates(
                &video_params_owned,
                codec_id,
                false,
                gpu_video_device.as_ref(),
            )
        }
        #[cfg(not(windows))]
        {
            open_video_decoder_with_candidates(&video_params_owned, codec_id, false)
        }
    };
    let opened_video = match opened_video_result {
        Ok(v) => v,
        Err(e) => {
            let _ = info_tx.send(Err(format!("video decoder open: {e}")));
            return;
        }
    };
    let video_decoder = opened_video.decoder;
    let video_decoder_name = opened_video.decoder_name;
    let hw_probe = opened_video.hw_probe;
    let hw_active_initially = opened_video.hw_device.is_some();
    let _hw_device = opened_video.hw_device;
    let src_w = video_decoder.width();
    let src_h = video_decoder.height();

    // 出力サイズは GPU テクスチャ上限に合わせて縮める
    let max_dim = crate::app::MAX_TEXTURE_DIM as u32;
    let (dst_w, dst_h) = clamp_dims(src_w, src_h, max_dim);

    // **scaler は lazy 構築**。
    // HW デコード時は `frame.format()` が `AV_PIX_FMT_D3D11` で、av_hwframe_transfer_data
    // で SW 取り出した結果は通常 NV12 (10-bit HEVC なら P010)。SW デコードでも `format()`
    // は decoder の出力フォーマットに依存する (yuv420p / yuvj420p / 等)。最初の 1 フレームを
    // 受け取った時点の **実際の入力フォーマット + 寸法** で初期化する。
    // (key に width/height を含めるのは、HW のサーフェス内部寸法と display 寸法が
    // 異なる場合や mid-stream で resolution change が起きた場合に
    // ScaleContext::run が `InputChanged` で全 frame skip に陥るのを防ぐため。)
    // Phase B: scaler / scaler_key / first_frame_logged はすべて run_video_decode の
    // ローカル変数として所有される (= デコーダ + GPU パスは別 thread)。

    // ── 音声ストリーム選択 (任意) ──
    let audio_setup = match input.streams().best(MediaType::Audio) {
        Some(audio_stream) => {
            let idx = audio_stream.index();
            let tb = audio_stream.time_base();
            let params = audio_stream.parameters();
            match ffmpeg::codec::context::Context::from_parameters(params) {
                Ok(mut ctx) => {
                    let audio_codec_id = ctx.id();
                    let audio_codec_name = audio_codec_id.name().to_string();
                    let (container_channels, container_layout_desc) =
                        audio_context_layout_summary(&ctx);
                    let stereo_request_sent = request_stereo_audio_decoder_output(
                        &mut ctx,
                        &audio_codec_name,
                        container_channels,
                        &container_layout_desc,
                    );
                    match ctx.decoder().audio() {
                        Ok(mut dec) => {
                            let in_fmt = dec.format();
                            let in_rate = dec.rate();
                            // FFmpeg 7.x API: channel_layout → ch_layout, get → get2
                            let (in_layout, guessed_stereo) = {
                                let raw_in_layout = dec.ch_layout();
                                normalize_audio_input_layout(raw_in_layout)
                            };
                            if guessed_stereo {
                                crate::logger::log(
                                "audio channel layout unspecified for 2ch stream; guessing stereo"
                                    .to_string(),
                            );
                                dec.set_ch_layout(in_layout.clone());
                            }
                            // 出力は f32 packed stereo / target_audio_sample_rate
                            let out_fmt = Sample::F32(SampleType::Packed);
                            let out_rate = target_audio_sample_rate;
                            let out_layout = ffmpeg::ChannelLayout::STEREO;
                            let input_channels = in_layout.channels();
                            let input_layout_desc = in_layout.description();
                            let input_format = format!("{in_fmt:?}");
                            let decoder_stereo_effective = input_channels <= 2;
                            let fast_downmix =
                                FastDownmixToStereo::new(in_fmt, &in_layout, in_rate, out_rate);
                            let fast_downmix_enabled = fast_downmix.is_some();
                            crate::logger::log(format!(
                                "audio setup: codec={audio_codec_name} container_layout=\"{container_layout_desc}\" container_channels={container_channels} decoder_layout=\"{input_layout_desc}\" decoder_channels={input_channels} in_fmt={input_format} in_rate={in_rate} out_layout=stereo out_rate={out_rate} request_stereo={stereo_request_sent} request_effective={decoder_stereo_effective} fast_downmix={fast_downmix_enabled}"
                            ));
                            match ResampleContext::get2(
                                in_fmt, in_layout, in_rate, out_fmt, out_layout, out_rate,
                            ) {
                                Ok(rs) => Some(AudioSetup {
                                    stream_idx: idx,
                                    out_rate,
                                    input_rate: in_rate,
                                    input_channels,
                                    input_layout_desc,
                                    input_format,
                                    output_channels: 2,
                                    decoder_stereo_requested: stereo_request_sent,
                                    decoder_stereo_effective,
                                    fast_downmix,
                                    time_base_num: tb.numerator() as f64,
                                    time_base_den: tb.denominator() as f64,
                                    decoder: dec,
                                    resampler: rs,
                                    codec_name: audio_codec_name,
                                }),
                                Err(e) => {
                                    crate::logger::log(format!(
                                        "audio resampler init failed: {e} (再生は映像のみ)"
                                    ));
                                    None
                                }
                            }
                        }
                        Err(e) => {
                            crate::logger::log(format!("audio decoder open failed: {e}"));
                            None
                        }
                    }
                }
                Err(e) => {
                    crate::logger::log(format!("audio codec context failed: {e}"));
                    None
                }
            }
        }
        None => None,
    };

    let has_audio = audio_setup.is_some();
    if !has_audio {
        // 音声無し動画: 最初から fallback wall clock を使う
        clock.mark_audio_inactive();
    }

    // ── 動画情報を通知 ──
    let duration_secs = duration_to_secs(input.duration());
    #[cfg(windows)]
    let gpu_path_active = gpu_video_device.is_some();
    #[cfg(not(windows))]
    let gpu_path_active = false;

    // ── 埋め込みメタデータ + チャプター (Phase 5.4) ──
    // 標準キーを拾う。Matroska / MP4 / ffmetadata で共通する小文字キー名を探し、
    // 大文字違いも順に試す。値が空なら None。
    let title = read_metadata_value(&input, &["title", "TITLE"]);
    let artist = read_metadata_value(&input, &["artist", "ARTIST", "author"]);
    let original_url = read_metadata_http_url(
        &input,
        &[
            "purl",
            "PURL",
            "url",
            "URL",
            "webpage_url",
            "WEBPAGE_URL",
            "source_url",
            "SOURCE_URL",
            "original_url",
            "ORIGINAL_URL",
            "comment",
            "COMMENT",
        ],
    );
    let description = read_metadata_value(
        &input,
        &["description", "DESCRIPTION", "comment", "COMMENT"],
    );
    let chapters: Vec<Chapter> = input
        .chapters()
        .map(|c| {
            // chapter time は AVChapter::time_base 単位の整数で、秒換算は
            // start * (num/den)。time_base が 0/0 なら 0.0 にフォールバック。
            let tb = c.time_base();
            let tb_num = tb.numerator() as f64;
            let tb_den = tb.denominator() as f64;
            let to_secs = |t: i64| -> f64 {
                if tb_den > 0.0 {
                    t as f64 * tb_num / tb_den
                } else {
                    0.0
                }
            };
            let title = {
                let md = c.metadata();
                md.get("title")
                    .or_else(|| md.get("TITLE"))
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
            };
            Chapter {
                start_secs: to_secs(c.start()),
                end_secs: to_secs(c.end()),
                title,
            }
        })
        .collect();

    let bit_rate_bps = input.bit_rate();
    let info = VideoInfo {
        width: src_w,
        height: src_h,
        duration_secs,
        video_codec: stream_codec_name.clone(),
        video_decoder: video_decoder_name.clone(),
        d3d11va_supported: hw_probe.d3d11va_supported,
        d3d11va_config: hw_probe.d3d11va_config.clone(),
        audio_codec: audio_setup.as_ref().map(|a| a.codec_name.clone()),
        has_audio,
        hw_decode_active: hw_active_initially,
        gpu_path_active,
        title,
        artist,
        original_url,
        description,
        avg_fps: video_avg_fps,
        bit_rate_bps,
        chapters,
    };
    let _ = info_tx.send(Ok(info));

    crate::logger::log(format!(
        "video decoder: codec={stream_codec_name} decoder={video_decoder_name} hw_requested={hw_decode_requested} hw_effective={effective_hw_decode_requested} d3d11va_supported={} hw_active_initially={hw_active_initially} gpu_path={gpu_path_active} field_order={video_field_order:?} stream_interlaced={video_stream_interlaced} d3d11va_config={}",
        hw_probe.d3d11va_supported, hw_probe.d3d11va_config
    ));

    // perf: 動画特性を 1 行に記録 (解析時の最初の手がかり)。
    if crate::perf::is_enabled() {
        let pix_fmt = format!("{:?}", video_decoder.format());
        let decode_path = if hw_active_initially {
            "hw_d3d11va"
        } else {
            "sw"
        };
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file_size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(-1);
        let (
            audio_codec,
            audio_rate,
            audio_ch,
            audio_input_rate,
            audio_input_ch,
            audio_input_layout,
            audio_input_format,
            audio_decoder_stereo_requested,
            audio_decoder_stereo_effective,
            audio_fast_downmix,
        ) = audio_setup
            .as_ref()
            .map(|a| {
                (
                    a.codec_name.clone(),
                    a.out_rate as i64,
                    a.output_channels as i64,
                    a.input_rate as i64,
                    a.input_channels as i64,
                    a.input_layout_desc.clone(),
                    a.input_format.clone(),
                    a.decoder_stereo_requested,
                    a.decoder_stereo_effective,
                    a.fast_downmix.is_some(),
                )
            })
            .unwrap_or_else(|| {
                (
                    "none".to_string(),
                    0,
                    0,
                    0,
                    0,
                    "none".to_string(),
                    "none".to_string(),
                    false,
                    false,
                    false,
                )
            });
        crate::perf::event(
            "video",
            "open",
            None,
            0,
            &[
                ("file", serde_json::Value::from(file_name)),
                ("file_size", serde_json::Value::from(file_size)),
                ("width", serde_json::Value::from(src_w as i64)),
                ("height", serde_json::Value::from(src_h as i64)),
                ("dst_w", serde_json::Value::from(dst_w as i64)),
                ("dst_h", serde_json::Value::from(dst_h as i64)),
                ("pix_fmt", serde_json::Value::from(pix_fmt)),
                ("decode_path", serde_json::Value::from(decode_path)),
                ("video_codec", serde_json::Value::from(stream_codec_name)),
                ("video_decoder", serde_json::Value::from(video_decoder_name)),
                (
                    "hw_decode_requested",
                    serde_json::Value::from(hw_decode_requested),
                ),
                (
                    "d3d11va_supported",
                    serde_json::Value::from(hw_probe.d3d11va_supported),
                ),
                (
                    "d3d11va_config",
                    serde_json::Value::from(hw_probe.d3d11va_config),
                ),
                (
                    "field_order",
                    serde_json::Value::from(format!("{video_field_order:?}")),
                ),
                (
                    "stream_interlaced",
                    serde_json::Value::from(video_stream_interlaced),
                ),
                ("avg_fps", serde_json::Value::from(video_avg_fps)),
                ("duration_secs", serde_json::Value::from(duration_secs)),
                ("audio_codec", serde_json::Value::from(audio_codec)),
                ("audio_rate", serde_json::Value::from(audio_rate)),
                ("audio_channels", serde_json::Value::from(audio_ch)),
                (
                    "audio_input_rate",
                    serde_json::Value::from(audio_input_rate),
                ),
                (
                    "audio_input_channels",
                    serde_json::Value::from(audio_input_ch),
                ),
                (
                    "audio_input_layout",
                    serde_json::Value::from(audio_input_layout),
                ),
                (
                    "audio_input_format",
                    serde_json::Value::from(audio_input_format),
                ),
                (
                    "audio_decoder_stereo_requested",
                    serde_json::Value::from(audio_decoder_stereo_requested),
                ),
                (
                    "audio_decoder_stereo_effective",
                    serde_json::Value::from(audio_decoder_stereo_effective),
                ),
                (
                    "audio_fast_downmix",
                    serde_json::Value::from(audio_fast_downmix),
                ),
            ],
        );
    }

    // ── Phase A (3-thread split): audio decode を独立スレッドに切り出す ──
    // 音声 packet 検出時は decode せず `audio_pkt_tx` に enqueue するだけにする。
    // これにより `audio_tx` (bounded=32) が満杯でも demux/video decode は止まらず、
    // `video_tx` (bounded=24) が枯渇しなくなる (旧構造の "buf 0/24" 振動の解消)。
    //
    // Keep this packet queue shallow. Audio prefill belongs in AudioBuffer
    // raw_pending; a deep packet queue delays ordered Flush markers after
    // resume/seek and can let old compressed audio run before the new timeline.
    let audio_stream_idx_for_demux: Option<usize> = audio_setup.as_ref().map(|a| a.stream_idx);
    let audio_time_base_for_demux: Option<(f64, f64)> = audio_setup
        .as_ref()
        .map(|a| (a.time_base_num, a.time_base_den));
    let (audio_pkt_tx, audio_pkt_rx) = bounded::<AudioWorkerMsg>(AUDIO_PACKET_QUEUE_CAP);
    // audio decode thread の JoinHandle。run_decoder 終了時に
    // `drop(audio_pkt_tx)` → channel disconnect → audio thread exit を経由して join する。
    // `audio_setup` は ここで consume される (= 以降 demux からは触らない)。
    let audio_decode_handle: Option<std::thread::JoinHandle<()>> = if let Some(setup) = audio_setup
    {
        let clock_a = clock.clone();
        let cancel_a = cancel.clone();
        let engine_state_a = engine_state.clone();
        // audio_tx の所有を audio decode thread に move。run_decoder 側は
        // 以降 audio_tx を直接触らない (audio_pkt_tx 経由で間接的に流す)。
        let audio_tx_for_thread = audio_tx;
        Some(
            std::thread::Builder::new()
                .name("video-audio-decode".into())
                .spawn(move || {
                    run_audio_decode(
                        setup,
                        audio_pkt_rx,
                        audio_tx_for_thread,
                        clock_a,
                        cancel_a,
                        engine_state_a,
                    );
                })
                .expect("spawn video-audio-decode thread"),
        )
    } else {
        // 音声無し動画: audio_tx と audio_pkt_rx を即 close する。AudioOutput pump は
        // 元から起動しないか、起動しても channel disconnect で即終了する。
        drop(audio_tx);
        drop(audio_pkt_rx);
        None
    };

    // ── Phase B (3-thread split): video decode も独立スレッドに切り出す ──
    // 旧構造では demux と video decode が同居しており、4K HEVC SW デコードや
    // HDD random read 100-300ms スパイクで video decode が止まると demux も止まり、
    // 音声 packet の `audio_pkt_tx` への流量も途絶えていた (= audio decode thread が
    // 飢餓状態に)。Phase B では demux thread が `input.packets()` ループを単独で
    // 回し、video packet を `video_pkt_tx` に enqueue するだけに専念する。
    //
    // Keep the direct video channel shallow so seek Flush markers reach the
    // decoder quickly. Sustained compressed-video burst absorption is handled
    // by the demux-side bounded overflow queue.
    let video_tb_num = video_time_base.numerator() as f64;
    let video_tb_den = video_time_base.denominator() as f64;
    let (video_pkt_tx, video_pkt_rx) = bounded::<VideoWorkerMsg>(VIDEO_PACKET_QUEUE_CAP);
    let video_decode_handle: std::thread::JoinHandle<()> = {
        let clock_v = clock.clone();
        let cancel_v = cancel.clone();
        let engine_state_v = engine_state.clone();
        let skipped_frame_count_v = skipped_frame_count.clone();
        // video_tx の所有を video decode thread に move。run_decoder (= demux) 側は
        // 以降 video_tx を直接触らない (video_pkt_tx 経由で間接的に流す)。
        let video_tx_for_thread = video_tx;
        // gpu_video_device は cfg(windows) のみ存在する Option<Arc<...>>。
        // Arc は move で thread 跨ぎ OK。
        #[cfg(windows)]
        let gpu_video_device_v = gpu_video_device;
        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                run_video_decode(
                    video_decoder,
                    _hw_device,
                    #[cfg(windows)]
                    gpu_video_device_v,
                    video_pkt_rx,
                    video_tx_for_thread,
                    clock_v,
                    cancel_v,
                    engine_state_v,
                    skipped_frame_count_v,
                    dst_w,
                    dst_h,
                    video_time_base.numerator(),
                    video_time_base.denominator(),
                    video_tb_num,
                    video_tb_den,
                    video_fps_num,
                    video_fps_den,
                    hw_active_initially,
                    deinterlace,
                    video_stream_interlaced,
                )
            })
            .expect("spawn video-decode thread")
    };
    // この時点で run_decoder = demux thread として再構成される。video_decoder /
    // _hw_device / gpu_video_device / video_tx はすべて video decode thread が所有。
    // 以下のループは demux + seek 調停 + EOF idle wait に専念する。

    // ── デコードループ (demux thread) ──

    let mut pending_video_packets: VecDeque<QueuedVideoPacket> = VecDeque::new();
    let mut pending_video_packet_bytes: usize = 0;
    let mut pending_video_peak_packets: usize = 0;
    let mut pending_video_peak_bytes: usize = 0;
    let mut next_video_overflow_log_bytes: usize = 0;
    let mut last_demux_queue_state_at = std::time::Instant::now();

    'outer: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }

        if !pending_video_packets.is_empty()
            && !drain_pending_video_packets(
                &mut pending_video_packets,
                &mut pending_video_packet_bytes,
                &video_pkt_tx,
                &clock,
                &cancel,
            )
        {
            break 'outer;
        }
        if pending_video_packets.is_empty() {
            next_video_overflow_log_bytes = 0;
        }
        if crate::perf::is_enabled()
            && last_demux_queue_state_at.elapsed() >= std::time::Duration::from_secs(1)
        {
            emit_demux_queue_state(
                &pending_video_packets,
                pending_video_packet_bytes,
                pending_video_peak_packets,
                pending_video_peak_bytes,
                &video_pkt_tx,
                &audio_pkt_tx,
            );
            last_demux_queue_state_at = std::time::Instant::now();
        }

        // シーク要求を確認
        if let Some(req) = clock.take_seek_request() {
            let super::clock::SeekRequest {
                target_secs,
                serial,
                kind,
            } = req;
            // Phase B: post_seek_frame_sent / drop_before_secs / current_seek_serial は
            // すべて video decode thread のローカル変数として所有される。demux thread は
            // 「seek 要求を受け取り → input.seek() を実行 → 両 decode thread に Flush
            // marker を送る」までを担当。
            // タイムスタンプは AV_TIME_BASE_Q (1/1_000_000 秒、マイクロ秒) 単位。
            let target_pts = (target_secs * 1_000_000.0) as i64;

            // 後方 / 絶対シークは `av_seek_frame + AVSEEK_FLAG_BACKWARD` を使う。
            // `avformat_seek_file` (= `Input::seek`) は AVSEEK_FLAG_BACKWARD を
            // 無視するため、デマクサが target を跨いだ前後どちらの keyframe を
            // 選ぶか不定。raw FFI 経由で確実に target 以前へ飛ばす。
            let backward =
                |input: &mut ffmpeg::format::context::Input| -> Result<(), ffmpeg::Error> {
                    use ffmpeg_the_third::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
                    let ret = unsafe {
                        av_seek_frame(
                            input.as_mut_ptr(),
                            -1,
                            target_pts,
                            AVSEEK_FLAG_BACKWARD as i32,
                        )
                    };
                    if ret >= 0 {
                        Ok(())
                    } else {
                        Err(ffmpeg::Error::from(ret))
                    }
                };

            // Phase 9.F (2026-04-30): 前方/後方/絶対に関係なく **常に backward seek**
            // (= `av_seek_frame + AVSEEK_FLAG_BACKWARD`) を使う。旧コードは前方相対で
            // `input.seek(target..)` (= avformat_seek_file with min_ts=target、target 以降
            // の keyframe に着地) を使い、preroll なしで「最寄り keyframe から再生」する
            // 設計だったが、forward seek は target+0.5〜2 秒の keyframe に着地し video
            // 1 枚目 pts >> target になる。一方 mp4 の音声 packet は keyframe pts より
            // 少し前から届くため、anchor.pts (= audio_pts) < video frame pts となり、
            // UI tick の `pts <= now + lead_tol` 判定で video frame が future 扱い →
            // 表示停止、音声だけ進む現象が起きていた。
            //
            // backward seek なら video/audio 両方が **target 直前の keyframe** で始まる
            // ので、anchor を target に書く [`SeekKind::Precise`] / [`SeekKind::Fast`]
            // どちらでも video frame は anchor より過去 = `pts <= now + lead_tol` で即時
            // 表示されるため UI 停止しない。差分は preroll trim (= drop_before_secs) を
            // 行うかどうかのみ:
            //
            // - **Precise**: drop_before_secs = Some(target)。keyframe → target を decode
            //   + drop して target ぴったりに着地 (= シークバー / ブックマークの精度)。
            // - **Fast**: drop_before_secs = None。preroll decode を完全にスキップ。
            //   keyframe pts (= target - 0〜3 秒) で即時再生開始 (= ←→ キーの体感速度)。
            //   audio monotonic guard が anchor 後退を防ぐので timeline 表示は target で
            //   固定、視聴コンテンツが GOP 1 個分先行する形になる。0〜3 秒の wall 経過で
            //   audio が target に追いつき完全同期する。
            let mut seek_result = backward(&mut input);
            // backward が失敗したら forward を retry (= EOF 近傍など、target 以前に
            // keyframe が無い場合)。
            // forward retry が走ったかを追跡: Fast モードは backward 成功時のみ preroll
            // trim を省略する。forward retry は keyframe ≥ target に着地するため、
            // trim 無しで放流すると Phase 9.F regression (audio_pts < video_pts → video
            // future stall) を再発させうる (Codex P2 助言、2026-05-01)。
            let mut used_forward_retry = false;
            if seek_result.is_err() {
                crate::logger::log(format!(
                    "backward seek failed at {target_secs:.3}s, retry as forward"
                ));
                seek_result = input.seek(target_pts, target_pts..);
                used_forward_retry = true;
            }
            let kind_str = match kind {
                super::clock::SeekKind::Precise => "precise",
                super::clock::SeekKind::Fast => "fast",
            };
            crate::logger::log(format!(
                "seek: target={target_secs:.3}s serial={serial} kind={kind_str} result={seek_result:?}"
            ));
            if seek_result.is_err() {
                // 完全失敗: override を明示解除しないと pace_now が target 固定で
                // UI が hang する。clock 経由で wall extrapolation に切替える。
                clock.clear_seek_target_override(serial);
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "video",
                        "seek_failure_abort",
                        None,
                        0,
                        &[
                            ("target", serde_json::Value::from(target_secs)),
                            ("serial", serde_json::Value::from(serial as i64)),
                        ],
                    );
                }
            }
            // Phase B: video / audio どちらの decoder も別 thread が所有しているので、
            // Flush は channel 経由で送る。順序保証 channel なので、Flush 後に enqueue
            // される packet は前世代として処理されない。
            //
            // **video / audio で trim 下限を分けて送る** (Codex P1 助言、2026-05-01):
            //
            // - `seek_target_for_flush` (= ユーザー要求 seek 位置): 成功時は常に
            //   `Some(target_secs)`。pump が BufferReady の audio_anchor pts に使う。
            //   これにより Fast モードでも Buffering→Playing 入場時の anchor が target
            //   に維持され、timeline 表示が target 固定になる。失敗時のみ `None`。
            // - `video_trim_before` (= video 用 preroll trim 下限):
            //   - Precise 成功: Some(target) → keyframe → target を decode + drop し
            //     target ぴったりに着地 (post_seek_frame_sent=false で 1 枚目を待機)
            //   - Fast backward 成功: None → preroll trim 無し、keyframe pts から即時再生
            //   - Fast forward retry 成功: Some(target) → retry は keyframe ≥ target に
            //     着地するため、trim 無しだと forward-seek regression (Phase 9.F) を
            //     再発させる (Codex P2 助言)。安全側に Precise 同等の trim を強制
            //   - 失敗: None → trim せず通常 pacing
            // - `audio_trim_before` (= audio 用 preroll trim 下限):
            //   - **Fast でも常に Some(target)** が正しい (Codex 2 巡目 P1 助言、
            //     2026-05-01)。理由:
            //     audio が trim なしで keyframe から鳴り始めると、`set_audio_pts`
            //     monotonic guard により clock anchor が target で凍結し、audio が物理
            //     的に target に追いつくまで (= 数秒) clock が進まず video pacing も
            //     凍結する (= 6-7 秒の動画フリーズ regression)。Fast では video のみ
            //     trim を省略して即時 keyframe 再生し、audio は target からスタートさせて
            //     target 直後から clock を 1x で進める。視覚は GOP 分先行する scrub に
            //     なるが、audio と clock は target 起点で同期するため freeze は発生しない。
            //   - Precise / forward retry / Fast: Some(target)
            //   - 失敗: None
            let seek_target_for_flush = if seek_result.is_ok() {
                Some(target_secs)
            } else {
                None
            };
            let video_trim_before = if seek_result.is_ok() {
                match kind {
                    super::clock::SeekKind::Precise => Some(target_secs),
                    super::clock::SeekKind::Fast => {
                        if used_forward_retry {
                            Some(target_secs) // 安全側に Precise 同等
                        } else {
                            None
                        }
                    }
                }
            } else {
                None
            };
            // audio は Fast でも常に target まで trim する (= clock freeze 回避)。
            let audio_trim_before = if seek_result.is_ok() {
                Some(target_secs)
            } else {
                None
            };
            // video_pkt_tx は drop されない (video decode thread が生きている間ずっと
            // 受信可能)。send は blocking なので順序保証されるが、cancel 中は
            // disconnect になる可能性がある (= video decode thread 終了後)。
            if !pending_video_packets.is_empty() {
                crate::logger::log(format!(
                    "[demux] discarded {} queued video packets ({} bytes) for seek serial={serial}",
                    pending_video_packets.len(),
                    pending_video_packet_bytes
                ));
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "demux",
                        "video_packet_overflow_discarded",
                        None,
                        0,
                        &[
                            (
                                "pending_packets",
                                serde_json::Value::from(pending_video_packets.len() as i64),
                            ),
                            (
                                "pending_bytes",
                                serde_json::Value::from(pending_video_packet_bytes as i64),
                            ),
                            ("seek_serial", serde_json::Value::from(serial as i64)),
                        ],
                    );
                }
                pending_video_packets.clear();
                pending_video_packet_bytes = 0;
                next_video_overflow_log_bytes = 0;
            }
            if !send_demux_msg_cancel_aware(
                &video_pkt_tx,
                VideoWorkerMsg::Flush {
                    serial,
                    seek_target_secs: seek_target_for_flush,
                    trim_before_secs: video_trim_before,
                },
                &cancel,
                "video",
                "flush",
                VIDEO_PACKET_QUEUE_CAP,
            ) {
                break 'outer;
            }
            if audio_stream_idx_for_demux.is_some() {
                let _ = send_demux_msg_cancel_aware(
                    &audio_pkt_tx,
                    AudioWorkerMsg::Flush {
                        serial,
                        seek_target_secs: seek_target_for_flush,
                        trim_before_secs: audio_trim_before,
                    },
                    &cancel,
                    "audio",
                    "flush",
                    AUDIO_PACKET_QUEUE_CAP,
                );
            }
            // 成功時のみ anchor を target に進める。失敗時に target を anchor すると
            // demux 位置とクロックが食い違い、anchor < frame_pts な audio set_audio_pts
            // 経路が monotonic-clamp で進まなくなる。
            // 失敗経路は anchor 据え置き + 音声会計だけリセットし、後続の最初の有効
            // frame / sample が set_audio_pts / set_fallback_anchor 経由で自然に
            // 現在位置にアンカーし直す。
            if seek_result.is_ok() {
                clock.notify_seek_completed(target_secs);
            } else {
                clock.reset_audio_bookkeeping_only();
            }
            // Phase 3e: engine にも SeekCompleted を通知 (= Seeking → Buffering 遷移)。
            // これがないと engine は永久 Seeking 状態に張り付き、pacing escape が
            // 解除されない。
            let _ = engine_event_tx.try_send(crate::video::engine::EngineEvent::Decoder(
                crate::video::engine::state::DecoderEvent::SeekCompleted {
                    epoch: serial,
                    actual_pts: target_secs,
                },
            ));
        }

        // 1 パケット読み込み
        if !pending_video_packets.is_empty()
            && !drain_pending_video_packets(
                &mut pending_video_packets,
                &mut pending_video_packet_bytes,
                &video_pkt_tx,
                &clock,
                &cancel,
            )
        {
            break 'outer;
        }

        let packet_iter = input.packets();
        // ※ packets() は &mut input を取るので毎ループ作り直す形になる。
        //    ffmpeg-the-third 3.x では packets() のアイテムが Result<(Stream, Packet), Error>
        //    に変わったので unwrap してから分解する。
        let mut got_packet = false;
        for item in packet_iter {
            let (stream, packet) = match item {
                Ok(sp) => sp,
                Err(e) => {
                    crate::logger::log(format!("packet read error: {e}"));
                    break;
                }
            };
            got_packet = true;
            if cancel.load(Ordering::Acquire) {
                break 'outer;
            }
            if stream.index() == video_stream_idx {
                // Phase B: video packet は decode せず video decode thread に転送する。
                // pre-decode preroll trim (= drop_before_secs check) と pacing logic は
                // すべて video decode thread 側に移管。
                //
                // `send` (blocking) を使う理由: 順序保証 channel に enqueue するので、
                // 直前の Flush marker と packet の到着順が逆転しない。bounded queue が
                // 満杯なら demux 側を一時 stall させて逆圧をかけるのが正しい。
                let packet_pts =
                    packet_timestamp(&packet).map(|pts| (pts as f64) * video_tb_num / video_tb_den);
                let packet_size = packet.size();
                let seek_serial = clock.current_seek_serial();
                let queue_len_before = video_pkt_tx.len();
                if !pending_video_packets.is_empty()
                    && !drain_pending_video_packets(
                        &mut pending_video_packets,
                        &mut pending_video_packet_bytes,
                        &video_pkt_tx,
                        &clock,
                        &cancel,
                    )
                {
                    break 'outer;
                }
                if !pending_video_packets.is_empty() || video_pkt_tx.is_full() {
                    while pending_video_packet_bytes.saturating_add(packet_size)
                        > VIDEO_PACKET_OVERFLOW_MAX_BYTES
                    {
                        let Some(queued) = pending_video_packets.pop_front() else {
                            break;
                        };
                        let QueuedVideoPacket {
                            serial,
                            packet,
                            pts_secs,
                            size_bytes,
                        } = queued;
                        let send_t0 = std::time::Instant::now();
                        match send_demux_packet_seek_aware(
                            &video_pkt_tx,
                            VideoWorkerMsg::Packet { serial, packet },
                            &clock,
                            &cancel,
                            "video",
                            VIDEO_PACKET_QUEUE_CAP,
                        ) {
                            DemuxPacketSend::Sent => {}
                            DemuxPacketSend::Cancelled => break 'outer,
                            DemuxPacketSend::SeekPending => {
                                pending_video_packet_bytes =
                                    pending_video_packet_bytes.saturating_sub(size_bytes);
                                continue 'outer;
                            }
                        }
                        pending_video_packet_bytes =
                            pending_video_packet_bytes.saturating_sub(size_bytes);
                        let wait_ms = send_t0.elapsed().as_secs_f64() * 1000.0;
                        if wait_ms >= DEMUX_PACKET_SEND_WAIT_WARN_MS {
                            emit_demux_packet_send_wait(
                                "video",
                                wait_ms,
                                queue_len_before,
                                VIDEO_PACKET_QUEUE_CAP,
                                pts_secs,
                                serial,
                            );
                        }
                    }
                    let queued_bytes = pending_video_packet_bytes.saturating_add(packet_size);
                    if queued_bytes <= VIDEO_PACKET_OVERFLOW_MAX_BYTES {
                        let should_log = pending_video_packets.is_empty()
                            || queued_bytes >= next_video_overflow_log_bytes;
                        pending_video_packet_bytes = queued_bytes;
                        pending_video_packets.push_back(QueuedVideoPacket {
                            serial: seek_serial,
                            packet,
                            pts_secs: packet_pts,
                            size_bytes: packet_size,
                        });
                        pending_video_peak_packets =
                            pending_video_peak_packets.max(pending_video_packets.len());
                        pending_video_peak_bytes =
                            pending_video_peak_bytes.max(pending_video_packet_bytes);
                        if should_log {
                            emit_video_packet_overflow_queued(
                                pending_video_packets.len(),
                                pending_video_packet_bytes,
                                queue_len_before,
                                packet_pts,
                                seek_serial,
                            );
                            next_video_overflow_log_bytes =
                                pending_video_packet_bytes.saturating_add(8 * 1024 * 1024);
                        }
                        break;
                    }
                }
                let send_t0 = std::time::Instant::now();
                match send_demux_packet_seek_aware(
                    &video_pkt_tx,
                    VideoWorkerMsg::Packet {
                        serial: seek_serial,
                        packet,
                    },
                    &clock,
                    &cancel,
                    "video",
                    VIDEO_PACKET_QUEUE_CAP,
                ) {
                    DemuxPacketSend::Sent => {}
                    // video decode thread が既に終了している → 自分も exit。
                    DemuxPacketSend::Cancelled => break 'outer,
                    DemuxPacketSend::SeekPending => continue 'outer,
                }
                let wait_ms = send_t0.elapsed().as_secs_f64() * 1000.0;
                if wait_ms >= DEMUX_PACKET_SEND_WAIT_WARN_MS {
                    emit_demux_packet_send_wait(
                        "video",
                        wait_ms,
                        queue_len_before,
                        VIDEO_PACKET_QUEUE_CAP,
                        packet_pts,
                        seek_serial,
                    );
                }
                break; // 1 パケット消費したらループ先頭でシークチェック
            } else if let Some(audio_idx) = audio_stream_idx_for_demux {
                if stream.index() == audio_idx {
                    // Phase A: 音声 packet は decode せず audio decode thread に転送する。
                    // packet 段階の pre-decode preroll trim と sample-level trim は両方
                    // audio decode thread 側に移管した (= AudioWorkerMsg::Flush で渡した
                    // `trim_before_secs` を audio thread が `drop_before_secs` として保持)。
                    //
                    // `send` (blocking) を使う理由: 順序保証 channel に enqueue するので、
                    // 直前の Flush marker と packet の到着順が逆転しない。bounded queue が
                    // 満杯なら demux 側を一時 stall させて逆圧をかけるのが正しい
                    // (audio_pkt_rx 側が止まっているのに packet を取り続けると memory が
                    // 無制限に膨らむ)。ただし stall 中も pending video overflow は drain
                    // する。precise seek では video 側の FirstFrameReady が Buffering を
                    // 抜ける条件なので、既に読めている video packet を audio back-pressure
                    // の後ろに取り残さない。
                    let packet_pts = audio_time_base_for_demux.and_then(|(tb_num, tb_den)| {
                        packet_timestamp(&packet).map(|pts| (pts as f64) * tb_num / tb_den)
                    });
                    let seek_serial = clock.current_seek_serial();
                    let queue_len_before = audio_pkt_tx.len();
                    let send_t0 = std::time::Instant::now();
                    match send_audio_packet_with_video_drain(
                        &audio_pkt_tx,
                        AudioWorkerMsg::Packet {
                            serial: seek_serial,
                            packet,
                        },
                        &clock,
                        &cancel,
                        &mut pending_video_packets,
                        &mut pending_video_packet_bytes,
                        &video_pkt_tx,
                    ) {
                        DemuxPacketSend::Sent => {}
                        // audio decode thread が既に終了している (= disconnect)。
                        // VideoPlayer の shutdown 経路 → 自分も exit。
                        DemuxPacketSend::Cancelled => break 'outer,
                        DemuxPacketSend::SeekPending => continue 'outer,
                    }
                    let wait_ms = send_t0.elapsed().as_secs_f64() * 1000.0;
                    if wait_ms >= DEMUX_PACKET_SEND_WAIT_WARN_MS {
                        emit_demux_packet_send_wait(
                            "audio",
                            wait_ms,
                            queue_len_before,
                            AUDIO_PACKET_QUEUE_CAP,
                            packet_pts,
                            seek_serial,
                        );
                    }
                    break; // 1 パケット消費したらループ先頭でシークチェック
                }
            }
        }
        if !got_packet {
            if !pending_video_packets.is_empty() {
                sleep_video_pacing();
                continue;
            }
            // EOF or demux stall。先に seek 要求をチェック (race で EOF flag 立てる
            // 前に新シークが来ていれば即通常ループに戻る)。
            if clock.peek_seek_request_pending() {
                continue;
            }
            // EOF 確定。スレッドは終わらせず、cancel か新しい seek 要求が来るまで
            // idle ループで待つ。これで末尾停止後の re-seek / replay が
            // decoder 再生成なしで動作する。
            clock.notify_eof_reached();
            // Phase A: audio decode thread にも Eof を通知して残フレームを drain させる。
            // (= 末尾の音声を確実に出し切る。drain しないと数十 ms の音声が抜ける。)
            if audio_stream_idx_for_demux.is_some() {
                let _ = send_demux_msg_cancel_aware(
                    &audio_pkt_tx,
                    AudioWorkerMsg::Eof,
                    &cancel,
                    "audio",
                    "eof",
                    AUDIO_PACKET_QUEUE_CAP,
                );
            }
            // Phase B: video decode thread にも Eof を通知。動画は内部残フレームを
            // 失っても許容なので drain しないが、Eof 自体は送って状態を伝える。
            let _ = send_demux_msg_cancel_aware(
                &video_pkt_tx,
                VideoWorkerMsg::Eof,
                &cancel,
                "video",
                "eof",
                VIDEO_PACKET_QUEUE_CAP,
            );
            loop {
                if cancel.load(Ordering::Acquire) {
                    crate::logger::log(format!("video decoder finished: {}", path.display()));
                    break 'outer;
                }
                if clock.peek_seek_request_pending() {
                    clock.clear_eof_reached();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    // Phase A/B: 終了時の cleanup。各 *_pkt_tx を drop → channel disconnect →
    // 各 decode thread の recv() が Err で抜け → exit → join。
    // 順序: audio を先に join (= cpal stream の bookkeeping を Drop より前に
    // 完了させたい)、次に video。
    drop(audio_pkt_tx);
    if let Some(handle) = audio_decode_handle {
        if let Err(e) = handle.join() {
            crate::logger::log(format!("video-audio-decode thread panicked: {e:?}"));
        }
    }
    drop(video_pkt_tx);
    if let Err(e) = video_decode_handle.join() {
        crate::logger::log(format!("video-decode thread panicked: {e:?}"));
    }
}

/// Phase B: 動画 decode + (HW D3D11VA GPU blit / SW swscale) + pacing + VideoFrame
/// 送出を担う独立スレッド。
///
/// 旧 `run_decoder` の動画 packet 処理ブロック (~450 行) を、`video_pkt_rx` から
/// 受け取った [`VideoWorkerMsg`] を処理する形に再構成したもの。
///
/// 呼び出し元 (= demux thread) は動画 packet を `video_pkt_tx` に enqueue する
/// だけで、`video_tx` (bounded=24) が満杯のときも自スレッドはブロックしない
/// (= drop してカウンタ加算)。`video_pkt_rx` (small bounded queue) は demux ↔ video decode
/// の逆圧経路として機能する。
///
/// シーク時は demux 側が `VideoWorkerMsg::Flush { serial, seek_target_secs,
/// trim_before_secs }` を送る。この thread は `Flush` 受領で内部 decoder を
/// `flush()` し、`current_seek_serial` / `drop_before_secs` / `post_seek_frame_sent`
/// をリセットする。`drop_before_secs` には `trim_before_secs` が入る (=
/// `seek_target_secs` は video 側では使わず、pump 側 BufferReady 用に audio チェーン
/// が持つ)。`trim_before_secs.is_none()` (= Fast backward 成功 or seek 失敗) の場合は
/// preroll trim せず通常 pacing に戻す (= post_seek_frame_sent を直ちに true)。
///
/// EOF 時は `VideoWorkerMsg::Eof` を受け取るが、動画は内部残フレームを失っても
/// 許容なので drain せず何もしない (旧 `run_decoder` の挙動と同じ)。
#[allow(clippy::too_many_arguments)]
fn run_video_decode(
    mut video_decoder: ffmpeg_the_third::decoder::Video,
    _hw_device: Option<HwDevice>,
    #[cfg(windows)] gpu_video_device: Option<
        std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
    >,
    video_pkt_rx: Receiver<VideoWorkerMsg>,
    video_tx: Sender<VideoFrame>,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    engine_state: Arc<std::sync::atomic::AtomicU8>,
    skipped_frame_count: Arc<std::sync::atomic::AtomicU64>,
    dst_w: u32,
    dst_h: u32,
    video_tb_num_i32: i32,
    video_tb_den_i32: i32,
    video_tb_num: f64,
    video_tb_den: f64,
    video_fps_num: u32,
    video_fps_den: u32,
    hw_active_initially: bool,
    deinterlace: crate::settings::VideoDeinterlaceMode,
    stream_interlaced: bool,
) {
    use ffmpeg::format::Pixel;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::video::Video;
    use ffmpeg_the_third as ffmpeg;

    let mut scaler: Option<ScaleContext> = None;
    let mut scaler_key: Option<(Pixel, u32, u32)> = None;
    let mut deinterlacer: Option<BwdifFilter> = None;
    let mut deinterlace_failure_logged = false;
    let mut deinterlace_cpu_fallback_logged = false;
    let mut first_frame_logged = false;
    let mut current_seek_serial: u64 = 0;
    let mut drop_before_secs: Option<f64> = None;
    let mut post_seek_frame_sent: bool = true;
    let mut last_enqueued_pts: f64 = 0.0;
    let mut stale_drop_burst_count: u64 = 0;
    let mut first_packet_logged_for_serial: Option<u64> = None;
    let mut preroll_drop_count: u64 = 0;
    let mut preroll_reached_logged_for_serial: Option<u64> = None;
    let mut pause_park_last_log: Option<std::time::Instant> = None;
    let mut post_seek_tx_full_last_log: Option<std::time::Instant> = None;

    'outer: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let msg = match video_pkt_rx.recv() {
            Ok(m) => m,
            Err(_) => break, // demux thread exited → channel disconnect
        };
        let packet = match msg {
            VideoWorkerMsg::Flush {
                serial,
                seek_target_secs: _,
                trim_before_secs,
            } => {
                if stale_drop_burst_count > 0 {
                    crate::logger::log(format!(
                        "[video-decode] stale packet burst drained before flush: count={stale_drop_burst_count} next_serial={serial} live_serial={}",
                        clock.current_seek_serial()
                    ));
                    stale_drop_burst_count = 0;
                }
                let prev_serial = current_seek_serial;
                let engine_st = engine_state.load(Ordering::Acquire);
                crate::logger::log(format!(
                    "[video-decode] flush received: serial={serial} prev_serial={prev_serial} trim_before={trim_before_secs:?} pkt_rx_len={} video_tx_len={} engine_state={}",
                    video_pkt_rx.len(),
                    video_tx.len(),
                    engine_state_code_name(engine_st)
                ));
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "video",
                        "flush_received",
                        None,
                        0,
                        &[
                            ("serial", serde_json::Value::from(serial as i64)),
                            ("prev_serial", serde_json::Value::from(prev_serial as i64)),
                            (
                                "trim_before",
                                trim_before_secs
                                    .map(serde_json::Value::from)
                                    .unwrap_or(serde_json::Value::Null),
                            ),
                            (
                                "pkt_rx_len",
                                serde_json::Value::from(video_pkt_rx.len() as i64),
                            ),
                            (
                                "video_tx_len",
                                serde_json::Value::from(video_tx.len() as i64),
                            ),
                            (
                                "engine_state",
                                serde_json::Value::from(engine_state_code_name(engine_st)),
                            ),
                        ],
                    );
                }
                video_decoder.flush();
                current_seek_serial = serial;
                // bwdif keeps a tiny temporal window. Drop it on seek so the
                // first post-seek frame cannot be compared with pre-seek data.
                deinterlacer = None;
                drop_before_secs = trim_before_secs;
                first_packet_logged_for_serial = None;
                preroll_drop_count = 0;
                preroll_reached_logged_for_serial = None;
                pause_park_last_log = None;
                post_seek_tx_full_last_log = None;
                // trim_before あり (Precise / Fast forward retry / 安全側) → post-seek
                // 1 枚目を待つので false。trim_before なし (Fast backward / 失敗) →
                // 通常 pacing に戻すので true。
                // seek_target_secs は video decode thread では使わない (= pump 側の
                // BufferReady audio_anchor 用)。
                post_seek_frame_sent = trim_before_secs.is_none();
                continue;
            }
            VideoWorkerMsg::Eof => {
                // 動画は EOF で内部 frame を失っても許容 (旧 run_decoder と同じ挙動)。
                // 何もせず次の Packet/Flush/disconnect を待つ。
                crate::logger::log(format!(
                    "[video-decode] eof received: serial={current_seek_serial} pkt_rx_len={} video_tx_len={} engine_state={}",
                    video_pkt_rx.len(),
                    video_tx.len(),
                    engine_state_code_name(engine_state.load(Ordering::Acquire))
                ));
                continue;
            }
            VideoWorkerMsg::Packet { serial, packet } => {
                let live_seek_serial = clock.current_seek_serial();
                if serial != current_seek_serial || serial != live_seek_serial {
                    stale_drop_burst_count = stale_drop_burst_count.saturating_add(1);
                    if crate::perf::is_enabled() {
                        let reason = if serial != live_seek_serial {
                            "live_seek_advanced"
                        } else {
                            "decoder_serial_mismatch"
                        };
                        crate::perf::event(
                            "video",
                            "stale_packet_drop",
                            None,
                            0,
                            &[
                                ("reason", serde_json::Value::from(reason)),
                                ("packet_serial", serde_json::Value::from(serial as i64)),
                                (
                                    "decoder_serial",
                                    serde_json::Value::from(current_seek_serial as i64),
                                ),
                                (
                                    "live_serial",
                                    serde_json::Value::from(live_seek_serial as i64),
                                ),
                            ],
                        );
                    }
                    continue;
                }
                if stale_drop_burst_count > 0 {
                    crate::logger::log(format!(
                        "[video-decode] stale packet burst ended: count={stale_drop_burst_count} serial={serial} live_serial={live_seek_serial} pkt_rx_len={}",
                        video_pkt_rx.len()
                    ));
                    stale_drop_burst_count = 0;
                }
                if first_packet_logged_for_serial != Some(serial) {
                    let packet_pts = packet_timestamp(&packet)
                        .map(|pts| (pts as f64) * video_tb_num / video_tb_den);
                    crate::logger::log(format!(
                        "[video-decode] first packet for serial={serial}: pts={packet_pts:?} pkt_rx_len={} video_tx_len={}",
                        video_pkt_rx.len(),
                        video_tx.len()
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "video",
                            "first_packet_for_serial",
                            None,
                            0,
                            &[
                                ("serial", serde_json::Value::from(serial as i64)),
                                (
                                    "packet_pts",
                                    packet_pts
                                        .map(serde_json::Value::from)
                                        .unwrap_or(serde_json::Value::Null),
                                ),
                                (
                                    "pkt_rx_len",
                                    serde_json::Value::from(video_pkt_rx.len() as i64),
                                ),
                                (
                                    "video_tx_len",
                                    serde_json::Value::from(video_tx.len() as i64),
                                ),
                            ],
                        );
                    }
                    first_packet_logged_for_serial = Some(serial);
                }
                packet
            }
        };

        let send_t0 = std::time::Instant::now();
        if let Err(e) = video_decoder.send_packet(&packet) {
            crate::logger::log(format!("video send_packet: {e}"));
            continue;
        }
        let mut frame = Video::empty();
        while video_decoder.receive_frame(&mut frame).is_ok() {
            if cancel.load(Ordering::Acquire) {
                break 'outer;
            }
            let decode_ms = send_t0.elapsed().as_secs_f64() * 1000.0;
            let pts = video_frame_timestamp(&frame).unwrap_or(0);
            let pts_secs = (pts as f64) * video_tb_num / video_tb_den;
            // post-seek preroll: target 前のフレームは描画しない
            if let Some(min) = drop_before_secs {
                if pts_secs + 0.005 < min {
                    preroll_drop_count = preroll_drop_count.saturating_add(1);
                    if crate::perf::is_enabled()
                        && (preroll_drop_count == 1 || preroll_drop_count % 30 == 0)
                    {
                        crate::perf::event(
                            "video",
                            "preroll_drop",
                            None,
                            0,
                            &[
                                (
                                    "serial",
                                    serde_json::Value::from(current_seek_serial as i64),
                                ),
                                ("count", serde_json::Value::from(preroll_drop_count as i64)),
                                ("frame_pts", serde_json::Value::from(pts_secs)),
                                ("trim_before", serde_json::Value::from(min)),
                                (
                                    "pkt_rx_len",
                                    serde_json::Value::from(video_pkt_rx.len() as i64),
                                ),
                            ],
                        );
                    }
                    continue;
                } else {
                    if preroll_reached_logged_for_serial != Some(current_seek_serial) {
                        crate::logger::log(format!(
                            "[video-decode] preroll reached target: serial={current_seek_serial} dropped={preroll_drop_count} first_pts={pts_secs:.3} trim_before={min:.3} pkt_rx_len={} video_tx_len={}",
                            video_pkt_rx.len(),
                            video_tx.len()
                        ));
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "video",
                                "preroll_reached",
                                None,
                                0,
                                &[
                                    (
                                        "serial",
                                        serde_json::Value::from(current_seek_serial as i64),
                                    ),
                                    (
                                        "dropped",
                                        serde_json::Value::from(preroll_drop_count as i64),
                                    ),
                                    ("first_pts", serde_json::Value::from(pts_secs)),
                                    ("trim_before", serde_json::Value::from(min)),
                                    (
                                        "pkt_rx_len",
                                        serde_json::Value::from(video_pkt_rx.len() as i64),
                                    ),
                                    (
                                        "video_tx_len",
                                        serde_json::Value::from(video_tx.len() as i64),
                                    ),
                                ],
                            );
                        }
                        preroll_reached_logged_for_serial = Some(current_seek_serial);
                    }
                    // target に到達した → preroll guard 解除 (動画側のみ)
                    // 音声側はまだ trim 必要なので drop_before_secs は audio thread
                    // が独自に管理する (= ここでは触らない)。
                }
            }

            // ── GPU 経路 (HW デコード + 共有 D3D11 device) ──
            // frame.format() == AV_PIX_FMT_D3D11 かつ mIV 側で GpuVideoDevice が
            // 利用可能な場合、av_hwframe_transfer_data + swscale を **完全に
            // スキップ** して、ID3D11VideoProcessor で直接 NV12→RGBA blit する。
            // 出力は NT 共有 ID3D11Texture2D で wgpu (egui) 側から sample される。
            #[cfg(windows)]
            if matches!(frame.format(), Pixel::D3D11) {
                let deinterlace_wants_cpu = should_try_deinterlace(
                    deinterlace,
                    frame.is_interlaced(),
                    stream_interlaced,
                    deinterlace_failure_logged,
                );
                if deinterlace_wants_cpu {
                    if !deinterlace_cpu_fallback_logged {
                        crate::logger::log(
                            "video deinterlace: using CPU bwdif path for D3D11VA frames"
                                .to_string(),
                        );
                        deinterlace_cpu_fallback_logged = true;
                    }
                } else if let Some(gpu_dev) = gpu_video_device.as_ref() {
                    match try_gpu_blit_path(
                        gpu_dev,
                        &frame,
                        dst_w,
                        dst_h,
                        pts_secs,
                        current_seek_serial,
                        &mut first_frame_logged,
                        hw_active_initially,
                        video_fps_num,
                        video_fps_den,
                    ) {
                        Ok(gpu_frame_out) => {
                            // ── デコーダのペーシング (GPU 経路) ──
                            // (詳細コメントは旧 run_decoder GPU 経路コメント参照、
                            //  Phase 8.K の PACE_LEAD=0.30 / post_seek_frame_sent /
                            //  SEEK_BURST_LEAD_MAX_SECS / generation race check 等)。
                            //
                            // **PDC-aware pacing** (2026-05-01, Codex 助言):
                            // VST3 plugin が PDC latency=N を報告したとき、video clock は
                            // N 秒遅れて進行する。pace_lead に `pdc_latency` を加えて
                            // VIDEO_QUEUE_LEAD_CAP_SECS で cap (= queue 過剰生産防止)。
                            //
                            // audio_buf は **actual buffer のみ** (= PDC は別 metric)。
                            // これにより cpal underrun 時 (= AudioBuffer 空) には audio_escape
                            // が確実に発動する。
                            //
                            // **微小 frame drop 修正** (2026-05-01, Codex 助言、追加修正):
                            // 旧: `audio_buf < AUDIO_CRITICAL_LO` で video frame の pace_lead
                            //     bypass を許可していた → PDC 大時に audio_buf が
                            //     構造的に低水位 (= 70-80ms) になり、bypass が常時発動して
                            //     queue を future frame で満杯にし、UI 表示 gap (= 微小スキップ)
                            //     を引き起こしていた
                            // 新: `pace_lead` bypass 条件は `ahead < SEEK_BURST_LEAD_MAX_SECS`
                            //     のみ (= seek/burst 直後の小ahead) に限定。actual audio low は
                            //     video frame の過剰生産で解決しない (= demux/audio decode は
                            //     別 thread)。
                            // 同時に AUDIO_SAFE_LO/HI を AudioBuffer cap (300ms) 前提の値に縮小
                            // (= VST 有効時の steady state 70-80ms で常時 escape にならないため)。
                            const PACE_LEAD_SECS: f64 = 0.30;
                            const AUDIO_SAFE_LO: f64 = 0.10; // 旧 0.25 (= cap 1.5s 時代)
                            const AUDIO_SAFE_HI: f64 = 0.20; // 旧 0.75 (= cap 300ms に整合)
                            const SEEK_BURST_LEAD_MAX_SECS: f64 = 0.20;
                            const AUDIO_CRITICAL_LO: f64 = 0.03; // 旧 0.08 (= 将来 audio 専用 emergency 用に保持)
                            let mut in_audio_escape = false;
                            let mut new_seek_pending = false;
                            // Phase 9.C (2026-04-30): pause-park。旧コードは
                            // `while !cancel && clock.is_playing()` で pause 時に loop が
                            // 抜けて try_send に落ち、decoder が HW デコード上限速度で
                            // バーストして video_tx (cap=24) を溢れさせていた
                            // (実測: 261 dropped_full / セッション)。
                            //
                            // Phase 9.D (2026-04-30 fixup): park 条件を
                            // `engine_state in [PAUSED, EOF]` に変更。
                            // 旧 9.C 版は `!clock.is_playing()` で park していたが、
                            // 動画 open 直後 (= autoplay=false で Loading 状態) も
                            // is_playing()=false で park してしまい、post-seek frame が
                            // 生成されず「動画を準備中…」のまま停止する regression があった。
                            // EngineState::parks_decoder() が true を返す state
                            // (= Paused/Eof) のみで park、Loading/Buffering/Seeking 中は
                            // pacing logic に進む (= pace_lead=0 で 1 frame ずつ処理)。
                            loop {
                                if cancel.load(Ordering::Acquire) {
                                    break;
                                }
                                // ⚠️ seek_serial check は **park sleep より先**:
                                // paused 状態のまま seek 要求が来たケース (将来 pause 維持
                                // seek を入れたとき) でも、park sleep の 50ms を待たずに
                                // 即 break して新世代を処理できるようにする。
                                if clock.current_seek_serial() != current_seek_serial {
                                    new_seek_pending = true;
                                    break;
                                }
                                let engine_st = engine_state.load(Ordering::Acquire);
                                if engine_st == crate::video::engine::actor::state_code::PAUSED
                                    || engine_st == crate::video::engine::actor::state_code::EOF
                                {
                                    let now = std::time::Instant::now();
                                    if pause_park_last_log.is_none_or(|last| {
                                        now.duration_since(last)
                                            >= std::time::Duration::from_secs(2)
                                    }) {
                                        crate::logger::log(format!(
                                            "[video-decode] pause park: serial={current_seek_serial} frame_pts={pts_secs:.3} state={} pkt_rx_len={} video_tx_len={} clock_playing={} clock_seeking={}",
                                            engine_state_code_name(engine_st),
                                            video_pkt_rx.len(),
                                            video_tx.len(),
                                            clock.is_playing(),
                                            clock.is_seeking()
                                        ));
                                        if crate::perf::is_enabled() {
                                            crate::perf::event(
                                                "video",
                                                "pause_park",
                                                None,
                                                0,
                                                &[
                                                    (
                                                        "serial",
                                                        serde_json::Value::from(
                                                            current_seek_serial as i64,
                                                        ),
                                                    ),
                                                    (
                                                        "frame_pts",
                                                        serde_json::Value::from(pts_secs),
                                                    ),
                                                    (
                                                        "engine_state",
                                                        serde_json::Value::from(
                                                            engine_state_code_name(engine_st),
                                                        ),
                                                    ),
                                                    (
                                                        "pkt_rx_len",
                                                        serde_json::Value::from(
                                                            video_pkt_rx.len() as i64
                                                        ),
                                                    ),
                                                    (
                                                        "video_tx_len",
                                                        serde_json::Value::from(
                                                            video_tx.len() as i64
                                                        ),
                                                    ),
                                                    (
                                                        "clock_playing",
                                                        serde_json::Value::from(clock.is_playing()),
                                                    ),
                                                    (
                                                        "clock_seeking",
                                                        serde_json::Value::from(clock.is_seeking()),
                                                    ),
                                                ],
                                            );
                                        }
                                        pause_park_last_log = Some(now);
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                    continue;
                                }
                                if pause_park_last_log.take().is_some() {
                                    crate::logger::log(format!(
                                        "[video-decode] pause park exit: serial={current_seek_serial} frame_pts={pts_secs:.3} state={} pkt_rx_len={} video_tx_len={} clock_playing={} clock_seeking={}",
                                        engine_state_code_name(engine_st),
                                        video_pkt_rx.len(),
                                        video_tx.len(),
                                        clock.is_playing(),
                                        clock.is_seeking()
                                    ));
                                }
                                let audio_buf = clock.total_audio_buffer_secs();
                                let audio_active = clock.is_audio_active();
                                // Phase 9.D (2026-04-30): Buffering 中も PACE_LEAD で
                                // lookahead を許可する。旧 (Phase 9.C 以前): pace_lead=0
                                // で 1 frame ずつ生産して engine が Buffering→Playing に
                                // 遷移するまで stall していたため、Playing 遷移時に
                                // future_frames がほぼ空 → UI 消費に追いつかず frame
                                // batching → silent dropped_past。
                                //
                                // 新挙動: Buffering でも 0.30s の lookahead を許可
                                // (= 60fps で 18 frames、30fps で 9 frames)。pace_now は
                                // Frozen のまま (= clock 進行なし)、ahead が PACE_LEAD に
                                // 達するまで decoder は frame を生産。これにより
                                // Buffering→Playing 遷移時には buffer がほぼ満杯
                                // (ユーザー要望: 「バッファが半分まで埋まったら開始」)。
                                let engine_playing =
                                    engine_st == crate::video::engine::actor::state_code::PLAYING;
                                let allow_pace_lead = engine_playing
                                    || engine_st
                                        == crate::video::engine::actor::state_code::BUFFERING;
                                if audio_active {
                                    if audio_buf < AUDIO_SAFE_LO {
                                        in_audio_escape = true;
                                    } else if audio_buf >= AUDIO_SAFE_HI {
                                        in_audio_escape = false;
                                    }
                                }
                                let ahead = pts_secs - clock.video_pacing_now_secs();
                                if !post_seek_frame_sent && video_tx.is_full() {
                                    let now = std::time::Instant::now();
                                    if post_seek_tx_full_last_log.is_none_or(|last| {
                                        now.duration_since(last)
                                            >= std::time::Duration::from_millis(500)
                                    }) {
                                        let engine_st = engine_state.load(Ordering::Acquire);
                                        crate::logger::log(format!(
                                            "[video-decode] post-seek first frame waiting for video_tx space: serial={current_seek_serial} frame_pts={pts_secs:.3} video_tx_len={} pkt_rx_len={} engine_state={} clock_seeking={}",
                                            video_tx.len(),
                                            video_pkt_rx.len(),
                                            engine_state_code_name(engine_st),
                                            clock.is_seeking()
                                        ));
                                        if crate::perf::is_enabled() {
                                            crate::perf::event(
                                                "video",
                                                "post_seek_video_tx_full_wait",
                                                None,
                                                0,
                                                &[
                                                    (
                                                        "serial",
                                                        serde_json::Value::from(
                                                            current_seek_serial as i64,
                                                        ),
                                                    ),
                                                    (
                                                        "frame_pts",
                                                        serde_json::Value::from(pts_secs),
                                                    ),
                                                    (
                                                        "video_tx_len",
                                                        serde_json::Value::from(
                                                            video_tx.len() as i64
                                                        ),
                                                    ),
                                                    (
                                                        "pkt_rx_len",
                                                        serde_json::Value::from(
                                                            video_pkt_rx.len() as i64
                                                        ),
                                                    ),
                                                    (
                                                        "engine_state",
                                                        serde_json::Value::from(
                                                            engine_state_code_name(engine_st),
                                                        ),
                                                    ),
                                                    (
                                                        "clock_seeking",
                                                        serde_json::Value::from(clock.is_seeking()),
                                                    ),
                                                ],
                                            );
                                        }
                                        post_seek_tx_full_last_log = Some(now);
                                    }
                                    sleep_video_pacing();
                                    continue;
                                }
                                post_seek_tx_full_last_log = None;
                                // Phase 9.E (2026-04-30 fixup): post-seek 1 枚目は
                                // **audio_buf に関係なく必ず送出** (= override clear に必須)。
                                // 旧コード (Phase 8.F-9.D) は seek_burst 全体を
                                // `audio_buf < AUDIO_SAFE_HI` で gate していたが、forward
                                // seek (= avformat が target 後の keyframe に着地) で
                                // ahead が 0.5-2 秒になり、かつ audio buffer が満杯
                                // (= 一時停止後など) のとき、`!post_seek_frame_sent` 経路が
                                // 発火せず 1 枚目が送出できない → override が永久残留 →
                                // deadlock になっていた。
                                const VIDEO_QUEUE_LEAD_CAP_SECS: f64 = 0.60;
                                let playback_speed = clock.playback_speed();
                                let seek_burst_lead = (SEEK_BURST_LEAD_MAX_SECS * playback_speed)
                                    .min(VIDEO_QUEUE_LEAD_CAP_SECS);
                                if clock.is_seeking() && !post_seek_frame_sent {
                                    break;
                                }
                                if clock.is_seeking()
                                    && !in_audio_escape
                                    && audio_buf < AUDIO_SAFE_HI
                                {
                                    if ahead < seek_burst_lead {
                                        break;
                                    }
                                    if audio_active && audio_buf < AUDIO_SAFE_LO {
                                        break;
                                    }
                                }
                                // PDC-aware pace_lead (Codex 助言、2026-05-01 改訂):
                                // VST3 plugin の構造的遅延分だけ先読み許可量を増やす
                                // (= plugin に未来 input を供給するため demux/decode を
                                // 進める)。ただし decoded video frame queue 容量
                                // (= video_tx 24 + future_frames 24 = 48 frames ≒ 800ms@60fps)
                                // を超えて先読みすると queue が常時 full → dropped_full 連発 →
                                // queue 先頭 PTS が `now + (pace_lead - queue_span)` 先行で
                                // 表示不可、という構造的スタッターを起こす。
                                //
                                // VIDEO_QUEUE_LEAD_CAP_SECS で安全側 0.60s に cap する
                                // (= 60fps で 36 frames 相当)。compressed-packet bursts are
                                // absorbed by the demux-side overflow queue rather than by this
                                // direct control/packet channel.
                                // 非 Windows では VST3 機能なし → pdc_latency=0 = 既存動作。
                                #[cfg(windows)]
                                let pdc_latency = clock
                                    .vst3_pdc_latency_secs()
                                    .min(crate::video::dsp::MAX_PDC_LATENCY_SECS);
                                #[cfg(not(windows))]
                                let pdc_latency: f64 = 0.0;
                                let pace_lead = if allow_pace_lead {
                                    (PACE_LEAD_SECS * playback_speed + pdc_latency)
                                        .min(VIDEO_QUEUE_LEAD_CAP_SECS)
                                } else {
                                    0.0
                                };
                                if ahead <= pace_lead {
                                    break;
                                }
                                // audio_escape bypass: actual audio が低水位かつ ahead が小さいとき
                                // (= seek/burst 直後の post-seek 1 枚目補填) のみ pace_lead を超えた
                                // 送出を許可。`audio_buf < AUDIO_CRITICAL_LO` 単独 bypass は撤去
                                // (= Codex 助言、2026-05-01。video frame の過剰生産は audio を救わない)。
                                let _ = AUDIO_CRITICAL_LO; // 将来 audio 専用 emergency 用に定数保持
                                if in_audio_escape && ahead < seek_burst_lead {
                                    break;
                                }
                                sleep_video_pacing();
                            }
                            if new_seek_pending {
                                // Phase B: 新世代 seek 受信時は次の recv() に戻る。
                                // 以降のフレームも generation race check で同様に
                                // skip され、Flush に到達したら video_decoder.flush()
                                // で残バッファごと clean up される。
                                continue 'outer;
                            }
                            use crossbeam_channel::TrySendError;
                            let pts_gap = pts_secs - last_enqueued_pts;
                            let mut dropped_full = false;
                            let mut send_disconnected = false;
                            match video_tx.try_send(gpu_frame_out) {
                                Ok(()) => {
                                    last_enqueued_pts = pts_secs;
                                    post_seek_frame_sent = true;
                                }
                                Err(TrySendError::Full(mut frame_out)) => {
                                    dropped_full = true;
                                    if let VideoFrameData::Gpu(gpu) = &mut frame_out.data {
                                        gpu.reset_unpresented_shared_output();
                                    }
                                    skipped_frame_count
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                                Err(TrySendError::Disconnected(_)) => {
                                    send_disconnected = true;
                                }
                            }
                            if crate::perf::is_enabled() {
                                crate::perf::event(
                                    "video",
                                    "decode",
                                    None,
                                    0,
                                    &[
                                        ("pts", serde_json::Value::from(pts_secs)),
                                        ("path", serde_json::Value::from("gpu_blit")),
                                        ("dropped_full", serde_json::Value::from(dropped_full)),
                                        ("pts_gap_ms", serde_json::Value::from(pts_gap * 1000.0)),
                                        (
                                            "audio_buf_secs",
                                            serde_json::Value::from(
                                                clock.total_audio_buffer_secs(),
                                            ),
                                        ),
                                        // 診断用: raw/processed/tx を分離して記録
                                        // (Codex 助言、2026-05-01)
                                        (
                                            "audio_processed_secs",
                                            serde_json::Value::from(clock.audio_processed_secs()),
                                        ),
                                        (
                                            "audio_raw_pending_secs",
                                            serde_json::Value::from(clock.audio_raw_pending_secs()),
                                        ),
                                        (
                                            "audio_tx_queued_secs",
                                            serde_json::Value::from(clock.audio_tx_queued_secs()),
                                        ),
                                        (
                                            "pace_now",
                                            serde_json::Value::from(clock.video_pacing_now_secs()),
                                        ),
                                    ],
                                );
                            }
                            if send_disconnected {
                                break 'outer;
                            }
                            continue;
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "GPU path failed, fallback to CPU readback: {e}"
                            ));
                            // フォールスルーして既存の CPU 経路に進む。
                        }
                    }
                }
            }

            // HW デコードの場合、`frame.format()` は `AV_PIX_FMT_D3D11`。
            // av_hwframe_transfer_data で GPU → CPU メモリに NV12 (10-bit なら P010)
            // として落とし、その SW フレームを scaler に食わせる。
            let mut sw_owned: Option<Video> = None;
            let frame_for_scaler: &Video = {
                let fmt = frame.format();
                if matches!(fmt, Pixel::D3D11) {
                    let mut sw = Video::empty();
                    unsafe {
                        use ffmpeg_the_third::ffi::av_hwframe_transfer_data;
                        let ret = av_hwframe_transfer_data(sw.as_mut_ptr(), frame.as_ptr(), 0);
                        if ret < 0 {
                            crate::logger::log(format!("av_hwframe_transfer_data failed: {ret}"));
                            continue;
                        }
                    }
                    sw_owned = Some(sw);
                    sw_owned.as_ref().unwrap()
                } else {
                    &frame
                }
            };

            let mut filtered_owned: Option<Video> = None;
            let mut pts_secs_for_output = pts_secs;
            let frame_interlaced = frame_for_scaler.is_interlaced();
            if should_try_deinterlace(
                deinterlace,
                frame_interlaced,
                stream_interlaced,
                deinterlace_failure_logged,
            ) {
                let key = bwdif_filter_key(frame_for_scaler, video_tb_num_i32, video_tb_den_i32);
                let force_all_frames =
                    bwdif_force_all_frames(deinterlace, frame_interlaced, stream_interlaced);
                let needs_new_graph = deinterlacer
                    .as_ref()
                    .is_none_or(|f| !f.matches(key, force_all_frames));
                if needs_new_graph {
                    match BwdifFilter::new(key, force_all_frames) {
                        Ok(f) => {
                            crate::logger::log(format!(
                                "video deinterlace: bwdif enabled mode={} fmt={:?} size={}x{}",
                                if force_all_frames {
                                    "all"
                                } else {
                                    "interlaced"
                                },
                                key.pix_fmt,
                                key.width,
                                key.height
                            ));
                            deinterlacer = Some(f);
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "video deinterlace: bwdif init failed, continuing without deinterlace: {e}"
                            ));
                            deinterlace_failure_logged = true;
                        }
                    }
                }
                if let Some(filter) = deinterlacer.as_mut() {
                    match filter.filter_one(frame_for_scaler) {
                        Ok(Some(filtered)) => {
                            if let Some(filtered_pts) = video_frame_timestamp(&filtered) {
                                pts_secs_for_output =
                                    (filtered_pts as f64) * video_tb_num / video_tb_den;
                            }
                            filtered_owned = Some(filtered);
                        }
                        Ok(None) => {
                            continue;
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "video deinterlace: bwdif failed, continuing without deinterlace: {e}"
                            ));
                            deinterlace_failure_logged = true;
                        }
                    }
                }
            }
            let frame_for_scaler = filtered_owned.as_ref().unwrap_or(frame_for_scaler);
            let pts_secs = pts_secs_for_output;

            // scaler の lazy 構築 / 入力 (フォーマット|寸法) 変化時の再構築。
            let cur_fmt = frame_for_scaler.format();
            let cur_w = frame_for_scaler.width();
            let cur_h = frame_for_scaler.height();
            let cur_key = (cur_fmt, cur_w, cur_h);
            if !first_frame_logged {
                let actual_path = if matches!(frame.format(), Pixel::D3D11) {
                    "hw_d3d11va"
                } else if hw_active_initially {
                    "sw_fallback_after_hw_init"
                } else {
                    "sw"
                };
                if actual_path == "sw_fallback_after_hw_init" {
                    crate::logger::log(format!(
                        "video decode_path: HW init succeeded but first frame is SW (pix_fmt={cur_fmt:?})"
                    ));
                }
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "video",
                        "first_frame",
                        None,
                        0,
                        &[
                            ("decode_path", serde_json::Value::from(actual_path)),
                            (
                                "frame_pix_fmt",
                                serde_json::Value::from(format!("{cur_fmt:?}")),
                            ),
                            ("frame_w", serde_json::Value::from(cur_w as i64)),
                            ("frame_h", serde_json::Value::from(cur_h as i64)),
                        ],
                    );
                }
                first_frame_logged = true;
            }
            if scaler.is_none() || scaler_key != Some(cur_key) {
                match ScaleContext::get(
                    cur_fmt,
                    cur_w,
                    cur_h,
                    Pixel::RGBA,
                    dst_w,
                    dst_h,
                    ScaleFlags::BILINEAR,
                ) {
                    Ok(s) => {
                        scaler = Some(s);
                        scaler_key = Some(cur_key);
                    }
                    Err(e) => {
                        crate::logger::log(format!("sws_scale init: {e}"));
                        continue;
                    }
                }
            }
            let scaler_ref = scaler.as_mut().expect("scaler initialized above");
            let mut rgba = Video::empty();
            if let Err(e) = scaler_ref.run(frame_for_scaler, &mut rgba) {
                crate::logger::log(format!("sws_scale: {e}"));
                continue;
            }
            drop(sw_owned);
            // Plane 0 から bytes を取り出し (RGBA はパッキング 1 plane)
            let stride = rgba.stride(0);
            let needed_stride = (dst_w * 4) as usize;
            let plane = rgba.data(0);
            let bgra: Vec<u8> = if stride == needed_stride {
                plane.to_vec()
            } else {
                // パディング除去
                let mut out = Vec::with_capacity(needed_stride * dst_h as usize);
                for row in 0..dst_h as usize {
                    let start = row * stride;
                    out.extend_from_slice(&plane[start..start + needed_stride]);
                }
                out
            };
            let frame_out = VideoFrame {
                width: dst_w,
                height: dst_h,
                data: VideoFrameData::Cpu(bgra),
                pts_secs,
                seek_serial: current_seek_serial,
            };
            let scale_ms = send_t0.elapsed().as_secs_f64() * 1000.0 - decode_ms;

            // ── デコーダのペーシング (CPU 経路) ──
            // (詳細コメントは旧 run_decoder CPU 経路コメント + GPU 経路コメント参照)。
            // 微小 frame drop 修正 (2026-05-01、Codex 助言): 詳細は GPU 経路コメント参照。
            const PACE_LEAD_SECS: f64 = 0.30;
            const AUDIO_SAFE_LO: f64 = 0.10; // 旧 0.25 (= 300ms cap 整合)
            const AUDIO_SAFE_HI: f64 = 0.20; // 旧 0.75
            const SEEK_BURST_LEAD_MAX_SECS: f64 = 0.20;
            const AUDIO_CRITICAL_LO: f64 = 0.03; // 旧 0.08 (= 将来 audio 専用 emergency 用に保持)
            let mut in_audio_escape = false;
            let mut new_seek_pending = false;
            // Phase 9.C/D: pause-park (詳細は GPU 経路の同コメント参照)。
            // seek_serial check は park sleep より先。
            loop {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                if clock.current_seek_serial() != current_seek_serial {
                    new_seek_pending = true;
                    break;
                }
                let engine_st = engine_state.load(Ordering::Acquire);
                if engine_st == crate::video::engine::actor::state_code::PAUSED
                    || engine_st == crate::video::engine::actor::state_code::EOF
                {
                    let now = std::time::Instant::now();
                    if pause_park_last_log.map_or(true, |last| {
                        now.duration_since(last) >= std::time::Duration::from_secs(2)
                    }) {
                        crate::logger::log(format!(
                            "[video-decode] pause park: serial={current_seek_serial} frame_pts={pts_secs:.3} state={} pkt_rx_len={} video_tx_len={} clock_playing={} clock_seeking={}",
                            engine_state_code_name(engine_st),
                            video_pkt_rx.len(),
                            video_tx.len(),
                            clock.is_playing(),
                            clock.is_seeking()
                        ));
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "video",
                                "pause_park",
                                None,
                                0,
                                &[
                                    (
                                        "serial",
                                        serde_json::Value::from(current_seek_serial as i64),
                                    ),
                                    ("frame_pts", serde_json::Value::from(pts_secs)),
                                    (
                                        "engine_state",
                                        serde_json::Value::from(engine_state_code_name(engine_st)),
                                    ),
                                    (
                                        "pkt_rx_len",
                                        serde_json::Value::from(video_pkt_rx.len() as i64),
                                    ),
                                    (
                                        "video_tx_len",
                                        serde_json::Value::from(video_tx.len() as i64),
                                    ),
                                    ("clock_playing", serde_json::Value::from(clock.is_playing())),
                                    ("clock_seeking", serde_json::Value::from(clock.is_seeking())),
                                ],
                            );
                        }
                        pause_park_last_log = Some(now);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                if pause_park_last_log.take().is_some() {
                    crate::logger::log(format!(
                        "[video-decode] pause park exit: serial={current_seek_serial} frame_pts={pts_secs:.3} state={} pkt_rx_len={} video_tx_len={} clock_playing={} clock_seeking={}",
                        engine_state_code_name(engine_st),
                        video_pkt_rx.len(),
                        video_tx.len(),
                        clock.is_playing(),
                        clock.is_seeking()
                    ));
                }
                let audio_buf = clock.total_audio_buffer_secs();
                let audio_active = clock.is_audio_active();
                // Phase 9.D (2026-04-30): Buffering 中も PACE_LEAD で lookahead 許可
                // (詳細は GPU 経路の同コメント参照)。
                let engine_playing = engine_st == crate::video::engine::actor::state_code::PLAYING;
                let allow_pace_lead = engine_playing
                    || engine_st == crate::video::engine::actor::state_code::BUFFERING;
                if audio_active {
                    if audio_buf < AUDIO_SAFE_LO {
                        in_audio_escape = true;
                    } else if audio_buf >= AUDIO_SAFE_HI {
                        in_audio_escape = false;
                    }
                }
                let ahead = pts_secs - clock.video_pacing_now_secs();
                if !post_seek_frame_sent && video_tx.is_full() {
                    let now = std::time::Instant::now();
                    if post_seek_tx_full_last_log.map_or(true, |last| {
                        now.duration_since(last) >= std::time::Duration::from_millis(500)
                    }) {
                        let engine_st = engine_state.load(Ordering::Acquire);
                        crate::logger::log(format!(
                            "[video-decode] post-seek first frame waiting for video_tx space: serial={current_seek_serial} frame_pts={pts_secs:.3} video_tx_len={} pkt_rx_len={} engine_state={} clock_seeking={}",
                            video_tx.len(),
                            video_pkt_rx.len(),
                            engine_state_code_name(engine_st),
                            clock.is_seeking()
                        ));
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "video",
                                "post_seek_video_tx_full_wait",
                                None,
                                0,
                                &[
                                    (
                                        "serial",
                                        serde_json::Value::from(current_seek_serial as i64),
                                    ),
                                    ("frame_pts", serde_json::Value::from(pts_secs)),
                                    (
                                        "video_tx_len",
                                        serde_json::Value::from(video_tx.len() as i64),
                                    ),
                                    (
                                        "pkt_rx_len",
                                        serde_json::Value::from(video_pkt_rx.len() as i64),
                                    ),
                                    (
                                        "engine_state",
                                        serde_json::Value::from(engine_state_code_name(engine_st)),
                                    ),
                                    ("clock_seeking", serde_json::Value::from(clock.is_seeking())),
                                ],
                            );
                        }
                        post_seek_tx_full_last_log = Some(now);
                    }
                    sleep_video_pacing();
                    continue;
                }
                post_seek_tx_full_last_log = None;
                // Phase 9.E: post-seek 1 枚目は audio_buf 不問で必ず送出
                // (詳細は GPU 経路の同コメント参照、forward seek deadlock 修正)。
                if clock.is_seeking() && !post_seek_frame_sent {
                    break;
                }
                const VIDEO_QUEUE_LEAD_CAP_SECS: f64 = 0.60;
                let playback_speed = clock.playback_speed();
                let seek_burst_lead =
                    (SEEK_BURST_LEAD_MAX_SECS * playback_speed).min(VIDEO_QUEUE_LEAD_CAP_SECS);
                if clock.is_seeking() && !in_audio_escape && audio_buf < AUDIO_SAFE_HI {
                    if ahead < seek_burst_lead {
                        break;
                    }
                    if audio_active && audio_buf < AUDIO_SAFE_LO {
                        break;
                    }
                }
                // PDC-aware pace_lead with queue-cap (= GPU 経路と同じ理屈、Codex 助言改訂版)。
                // 詳細コメントは GPU 経路を参照。
                #[cfg(windows)]
                let pdc_latency = clock
                    .vst3_pdc_latency_secs()
                    .min(crate::video::dsp::MAX_PDC_LATENCY_SECS);
                #[cfg(not(windows))]
                let pdc_latency: f64 = 0.0;
                let pace_lead = if allow_pace_lead {
                    (PACE_LEAD_SECS * playback_speed + pdc_latency).min(VIDEO_QUEUE_LEAD_CAP_SECS)
                } else {
                    0.0
                };
                if ahead <= pace_lead {
                    break;
                }
                // audio_escape bypass: GPU 経路と同じ理屈で `audio_buf < CRITICAL` 単独 bypass を撤去。
                let _ = AUDIO_CRITICAL_LO; // 将来 audio 専用 emergency 用に定数保持
                if in_audio_escape && ahead < seek_burst_lead {
                    break;
                }
                sleep_video_pacing();
            }
            if new_seek_pending {
                continue 'outer;
            }

            let pts_gap = pts_secs - last_enqueued_pts;
            use crossbeam_channel::TrySendError;
            let send_result = video_tx.try_send(frame_out);
            let dropped_full = matches!(&send_result, Err(TrySendError::Full(_)));
            if !dropped_full && send_result.is_ok() {
                last_enqueued_pts = pts_secs;
                post_seek_frame_sent = true;
            }
            if dropped_full {
                skipped_frame_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "video",
                    "decode",
                    None,
                    0,
                    &[
                        ("pts", serde_json::Value::from(pts_secs)),
                        (
                            "decode_ms",
                            serde_json::Value::from(decode_ms.round() / 1.0),
                        ),
                        ("scale_ms", serde_json::Value::from(scale_ms.round() / 1.0)),
                        ("dropped_full", serde_json::Value::from(dropped_full)),
                        ("pts_gap_ms", serde_json::Value::from(pts_gap * 1000.0)),
                        (
                            "audio_buf_secs",
                            serde_json::Value::from(clock.total_audio_buffer_secs()),
                        ),
                        // 診断用: raw/processed/tx を分離して記録
                        // (Codex 助言、2026-05-01)
                        (
                            "audio_processed_secs",
                            serde_json::Value::from(clock.audio_processed_secs()),
                        ),
                        (
                            "audio_raw_pending_secs",
                            serde_json::Value::from(clock.audio_raw_pending_secs()),
                        ),
                        (
                            "audio_tx_queued_secs",
                            serde_json::Value::from(clock.audio_tx_queued_secs()),
                        ),
                        (
                            "pace_now",
                            serde_json::Value::from(clock.video_pacing_now_secs()),
                        ),
                    ],
                );
            }
            if let Err(TrySendError::Disconnected(_)) = send_result {
                break 'outer;
            }
        }
    }
}

/// Phase A: 音声 decode + resample + AudioFrame 送出を担う独立スレッド。
///
/// 旧 `run_decoder` の音声 packet 処理ブロック (avcodec decode + swresample +
/// `audio_tx.send`) と post-seek preroll trim (packet 段階 + sample 段階) を、
/// `audio_pkt_rx` から受け取った [`AudioWorkerMsg`] を処理する形に再構成したもの。
///
/// 呼び出し元 (= demux + video decode thread) は音声 packet を `audio_pkt_tx` に
/// enqueue するだけで、`audio_tx` (bounded=32) が満杯のときも自スレッドはブロック
/// しない。`audio_pkt_rx` (small bounded queue) は両 thread 間の逆圧経路として機能する。
///
/// シーク時は呼び出し元が `AudioWorkerMsg::Flush { serial, seek_target_secs,
/// trim_before_secs }` を送る。この thread は `Flush` 受領で内部 decoder を
/// `flush()` し、`current_seek_serial` / `drop_before_secs` /
/// `current_seek_target_secs` をリセットする。
///
/// **`trim_before_secs` の値**:
/// - 成功時 (Precise / Fast / forward retry): demux 側で常に `Some(target)` が送られる。
///   audio 側は seek 種別に関係なく target まで preroll trim する (= Codex 2 巡目 P1、
///   2026-05-01: Fast でも audio trim を残さないと clock anchor が target で凍結し
///   video pacing が 6-7 秒止まる regression が発生していた)。
/// - 失敗時: `None` (= demux 位置が動いていないので trim せず通常 pacing に戻す)。
///
/// `seek_target_secs` は世代単位で保持し、emit する全 AudioFrame に焼き付けて pump
/// へ伝搬する (= pump が BufferReady の audio_anchor pts に target を反映するため)。
///
/// EOF 時は `AudioWorkerMsg::Eof` を受けて内部 decoder を flush + 残フレーム drain。
/// その後は次の `Flush` か `Packet` か channel disconnect (= run_decoder 終了) を待つ。
fn run_audio_decode(
    mut setup: AudioSetup,
    audio_pkt_rx: Receiver<AudioWorkerMsg>,
    audio_tx: Sender<AudioFrame>,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    engine_state: Arc<std::sync::atomic::AtomicU8>,
) {
    use ffmpeg_the_third::util::frame::audio::Audio;

    // run_decoder と同じ thread-local state。Flush で reset する。
    let mut current_seek_serial: u64 = 0;
    let mut drop_before_secs: Option<f64> = None;
    // **Codex P1 (2026-05-01)**: Fast モードで preroll trim を省略しても
    // BufferReady の audio_anchor を user-requested target に維持するため、Flush
    // から受け取った `seek_target_secs` を世代単位で保持し、emit する全 AudioFrame
    // に焼き付ける (= pump がどのタイミングで観測しても target を取り出せる)。
    let mut current_seek_target_secs: Option<f64> = None;
    // Some codecs, notably WMA Pro in ASF/WMV, emit one correctly timestamped
    // frame followed by several decoded frames with pts/best_effort_timestamp
    // reset to 0. Keep a monotonic synthetic cursor per seek generation so the
    // audio clock and video pacing do not get pinned to zero.
    let mut next_audio_pts_secs: Option<f64> = None;
    let mut pause_park_last_log: Option<std::time::Instant> = None;

    'outer: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        loop {
            if cancel.load(Ordering::Acquire) {
                break 'outer;
            }
            let live_seek_serial = clock.current_seek_serial();
            if live_seek_serial != current_seek_serial {
                if pause_park_last_log.take().is_some() {
                    let engine_st = engine_state.load(Ordering::Acquire);
                    crate::logger::log(format!(
                        "[audio-decode] pause park exit: reason=seek_serial_changed serial={current_seek_serial} live_serial={live_seek_serial} state={} pkt_rx_len={} audio_tx_len={} clock_playing={} clock_seeking={}",
                        engine_state_code_name(engine_st),
                        audio_pkt_rx.len(),
                        audio_tx.len(),
                        clock.is_playing(),
                        clock.is_seeking()
                    ));
                }
                break;
            }
            let engine_st = engine_state.load(Ordering::Acquire);
            if !engine_state_parks_decode(engine_st) {
                if pause_park_last_log.take().is_some() {
                    crate::logger::log(format!(
                        "[audio-decode] pause park exit: serial={current_seek_serial} state={} pkt_rx_len={} audio_tx_len={} clock_playing={} clock_seeking={}",
                        engine_state_code_name(engine_st),
                        audio_pkt_rx.len(),
                        audio_tx.len(),
                        clock.is_playing(),
                        clock.is_seeking()
                    ));
                }
                break;
            }
            let now = std::time::Instant::now();
            if pause_park_last_log
                .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_secs(2))
            {
                crate::logger::log(format!(
                    "[audio-decode] pause park: serial={current_seek_serial} state={} pkt_rx_len={} audio_tx_len={} clock_playing={} clock_seeking={}",
                    engine_state_code_name(engine_st),
                    audio_pkt_rx.len(),
                    audio_tx.len(),
                    clock.is_playing(),
                    clock.is_seeking()
                ));
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "audio",
                        "pause_park",
                        None,
                        0,
                        &[
                            (
                                "serial",
                                serde_json::Value::from(current_seek_serial as i64),
                            ),
                            (
                                "engine_state",
                                serde_json::Value::from(engine_state_code_name(engine_st)),
                            ),
                            (
                                "pkt_rx_len",
                                serde_json::Value::from(audio_pkt_rx.len() as i64),
                            ),
                            (
                                "audio_tx_len",
                                serde_json::Value::from(audio_tx.len() as i64),
                            ),
                            ("clock_playing", serde_json::Value::from(clock.is_playing())),
                            ("clock_seeking", serde_json::Value::from(clock.is_seeking())),
                        ],
                    );
                }
                pause_park_last_log = Some(now);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let msg = match audio_pkt_rx.recv() {
            Ok(m) => m,
            // demux 側が exit (= run_decoder 終了 / cancel) → audio_pkt_tx drop →
            // 自スレッドも exit。
            Err(_) => break,
        };
        match msg {
            AudioWorkerMsg::Flush {
                serial,
                seek_target_secs,
                trim_before_secs,
            } => {
                setup.decoder.flush();
                current_seek_serial = serial;
                drop_before_secs = trim_before_secs;
                current_seek_target_secs = seek_target_secs;
                next_audio_pts_secs = None;
            }
            AudioWorkerMsg::Eof => {
                // 残フレーム drain: send_eof + receive_frame ループで decoder 内の
                // 残サンプルを最後まで取り出して送る。これにより末尾の数十 ms が
                // 抜けない。FFmpeg の API では NULL packet で EOF flush を伝える。
                use ffmpeg_the_third::ffi::avcodec_send_packet;
                unsafe {
                    let _ = avcodec_send_packet(setup.decoder.as_mut_ptr(), std::ptr::null());
                }
                let mut frame = Audio::empty();
                while setup.decoder.receive_frame(&mut frame).is_ok() {
                    if cancel.load(Ordering::Acquire) {
                        break 'outer;
                    }
                    if !emit_audio_frame(
                        &mut setup,
                        &mut frame,
                        &mut drop_before_secs,
                        current_seek_serial,
                        current_seek_target_secs,
                        None,
                        &mut next_audio_pts_secs,
                        &clock,
                        &audio_tx,
                        &engine_state,
                    ) {
                        break 'outer;
                    }
                }
                // EOF 後 decoder を flush して次回の Packet/Flush に備える。
                setup.decoder.flush();
            }
            AudioWorkerMsg::Packet { serial, packet } => {
                let live_seek_serial = clock.current_seek_serial();
                if serial != current_seek_serial || serial != live_seek_serial {
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "audio",
                            "stale_packet_drop",
                            None,
                            0,
                            &[
                                ("packet_serial", serde_json::Value::from(serial as i64)),
                                (
                                    "decoder_serial",
                                    serde_json::Value::from(current_seek_serial as i64),
                                ),
                                (
                                    "live_serial",
                                    serde_json::Value::from(live_seek_serial as i64),
                                ),
                                (
                                    "reason",
                                    serde_json::Value::from(if serial != live_seek_serial {
                                        "live_seek_advanced"
                                    } else {
                                        "decoder_serial_mismatch"
                                    }),
                                ),
                            ],
                        );
                    }
                    continue;
                }
                // Phase 8.E: post-seek audio preroll を packet 段階で **decode せず**
                // 切り捨てる。avformat は seek backward 後 keyframe 直前から packets
                // を返すため、音声 stream は target から数百 ms 前の packets が連続
                // して届く。旧コードはそれらを send_packet → receive_frame → resample
                // → drop_before_secs でドロップしていたが、デコード + resample が
                // 1 packet ~5ms かかり 50 packets で 250ms 分 demux ループが詰まり、
                // 結果として post-seek の音声出力が ~1 秒途切れる現象に直結していた。
                // packet の pts + duration が target より明確に前なら decode をスキップ。
                // (packet boundary 上では target を跨ぐもののみデコードして
                //  drop_before_secs の sample-level trim 経路で正確に切り出す。)
                if let Some(min) = drop_before_secs {
                    let pkt_pts = packet_timestamp(&packet).unwrap_or(i64::MIN);
                    if pkt_pts != i64::MIN {
                        let pkt_pts_secs =
                            (pkt_pts as f64) * setup.time_base_num / setup.time_base_den;
                        let pkt_dur_secs = (packet.duration().max(0) as f64) * setup.time_base_num
                            / setup.time_base_den;
                        if pkt_pts_secs + pkt_dur_secs < min - 0.020 {
                            continue;
                        }
                    }
                }
                let packet_decode_t0 = std::time::Instant::now();
                if let Err(e) = setup.decoder.send_packet(&packet) {
                    crate::logger::log(format!("audio send_packet: {e}"));
                    continue;
                }
                let mut frame = Audio::empty();
                while setup.decoder.receive_frame(&mut frame).is_ok() {
                    if cancel.load(Ordering::Acquire) {
                        break 'outer;
                    }
                    if clock.current_seek_serial() != current_seek_serial {
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "audio",
                                "stale_frame_drop",
                                None,
                                0,
                                &[
                                    (
                                        "decoder_serial",
                                        serde_json::Value::from(current_seek_serial as i64),
                                    ),
                                    (
                                        "live_serial",
                                        serde_json::Value::from(clock.current_seek_serial() as i64),
                                    ),
                                    ("reason", serde_json::Value::from("live_seek_advanced")),
                                ],
                            );
                        }
                        break;
                    }
                    if !emit_audio_frame(
                        &mut setup,
                        &mut frame,
                        &mut drop_before_secs,
                        current_seek_serial,
                        current_seek_target_secs,
                        Some(packet_decode_t0.elapsed().as_secs_f64() * 1000.0),
                        &mut next_audio_pts_secs,
                        &clock,
                        &audio_tx,
                        &engine_state,
                    ) {
                        break 'outer;
                    }
                }
            }
        }
    }
}

/// 1 audio frame を resample → trim → AudioFrame 化 → audio_tx に送出。
///
/// 戻り値: false なら audio_tx が disconnected なので呼び出し元はスレッド終了する。
/// drop_before_secs は preroll trim で drain した結果ここで `None` に戻ることがある。
fn emit_audio_frame(
    setup: &mut AudioSetup,
    frame: &mut ffmpeg_the_third::util::frame::audio::Audio,
    drop_before_secs: &mut Option<f64>,
    current_seek_serial: u64,
    current_seek_target_secs: Option<f64>,
    decode_wait_ms: Option<f64>,
    next_audio_pts_secs: &mut Option<f64>,
    clock: &AvClock,
    audio_tx: &Sender<AudioFrame>,
    engine_state: &std::sync::atomic::AtomicU8,
) -> bool {
    use ffmpeg::format::sample::{Sample, Type as SampleType};
    use ffmpeg::util::frame::audio::Audio;
    use ffmpeg_the_third as ffmpeg;

    let emit_t0 = std::time::Instant::now();
    let raw_pts_secs = audio_frame_timestamp(frame)
        .map(|pts| (pts as f64) * setup.time_base_num / setup.time_base_den);
    let mut pts_synthesized = false;
    let mut pts_secs = match (*next_audio_pts_secs, raw_pts_secs) {
        (Some(next), Some(raw)) if raw + 0.001 >= next => raw,
        (Some(next), _) => {
            pts_synthesized = true;
            next
        }
        (None, Some(raw)) => raw,
        (None, None) => {
            pts_synthesized = true;
            0.0
        }
    };
    const CHANNELS: usize = 2;
    let convert_t0 = std::time::Instant::now();
    let (mut samples, audio_path): (Vec<f32>, &'static str) =
        if let Some(downmix) = &setup.fast_downmix {
            match downmix.run(frame) {
                Some(samples) => (samples, "fast_downmix"),
                None => return true,
            }
        } else {
            let (_, guessed_frame_stereo) = normalize_audio_input_layout(frame.ch_layout());
            if guessed_frame_stereo {
                frame.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
            }
            let mut resampled = Audio::empty();
            if let Err(e) = setup.resampler.run(frame, &mut resampled) {
                crate::logger::log(format!("swr resample: {e}"));
                return true; // 1 frame 失敗は致命的でない
            }
            // 1 plane (packed) の f32 を取り出す。
            //
            // ⚠️ **`data(0)` は使わない**。`data(0)` が返すスライスは
            // ffmpeg-the-third の `linesize[0]` ベースで、SIMD アラインメント
            // のため **実サンプル数より大きいバイト列を返す**。
            // `chunks_exact(4)` で f32 化すると末尾のパディング
            // (未初期化メモリ or 0) も f32 として再生してしまい、
            // 強い "ブチブチ" ノイズの原因になる。
            //
            // door_player と同じく `(*frame.as_ptr()).data[0]` を直接 `*const f32`
            // としてキャストし、要素数 = `samples * channels` (= 実サンプル数)
            // を指定して `from_raw_parts` でスライス化する。これにより
            // FFmpeg の linesize パディングを完全にスキップできる。
            //
            // SAFETY:
            //   - resampled は packed format (Sample::F32(Type::Packed))
            //   - data[0] は frame の生存中有効
            //   - samples * channels * sizeof(f32) バイトは確実にアロケート済み
            //   - resampler.run() の出力なので i32 オーバーフローは起きない
            let nb_samples = resampled.samples();
            if nb_samples == 0 {
                // 解像度の都合等で 0 サンプルが返ることがある (resample のラグ)。
                // raw pointer dereference を避けて早期 return。
                return true;
            }
            // ランタイム不変条件チェック (Codex P3): resampler の出力が
            // 期待通り f32 packed であること。デバッグ時に format/layout
            // を取り違えていれば即座に panic で気付ける。
            debug_assert_eq!(resampled.format(), Sample::F32(SampleType::Packed));
            debug_assert!(resampled.is_packed());
            let element_count = nb_samples * CHANNELS;
            let samples = unsafe {
                let raw_ptr = (*resampled.as_ptr()).data[0] as *const f32;
                debug_assert!(!raw_ptr.is_null());
                std::slice::from_raw_parts(raw_ptr, element_count).to_vec()
            };
            (samples, "swr")
        };
    let convert_ms = convert_t0.elapsed().as_secs_f64() * 1000.0;

    // post-seek preroll の trim:
    // avformat_seek は keyframe に戻るので、target_secs 未満の
    // 音声フレームが届く。完全に target 前ならフレーム破棄、
    // 跨ぐなら先頭 N サンプルを drain して target ぴったりから始める。
    if let Some(min) = *drop_before_secs {
        let frame_secs = (samples.len() / CHANNELS) as f64 / setup.out_rate as f64;
        if pts_secs + frame_secs <= min {
            // 完全に target 前 → 捨てる
            *next_audio_pts_secs = Some(pts_secs + frame_secs);
            return true;
        }
        if pts_secs < min {
            let skip_pairs = ((min - pts_secs) * setup.out_rate as f64).ceil() as usize;
            let skip = (skip_pairs * CHANNELS).min(samples.len());
            samples.drain(..skip);
            pts_secs = min;
            if samples.is_empty() {
                return true;
            }
        } else {
            // target に到達 → preroll guard 解除
            *drop_before_secs = None;
        }
    }

    // 1 stereo pair = 2 float (samples_per_sec stereo = setup.out_rate * 2)
    let frame_sample_pairs = samples.len() / CHANNELS;
    let duration_secs = frame_sample_pairs as f64 / setup.out_rate as f64;
    *next_audio_pts_secs = Some(pts_secs + duration_secs);
    if clock.current_seek_serial() != current_seek_serial {
        if crate::perf::is_enabled() {
            crate::perf::event(
                "audio",
                "stale_frame_drop",
                None,
                0,
                &[
                    ("pts", serde_json::Value::from(pts_secs)),
                    (
                        "decoder_serial",
                        serde_json::Value::from(current_seek_serial as i64),
                    ),
                    (
                        "live_serial",
                        serde_json::Value::from(clock.current_seek_serial() as i64),
                    ),
                    (
                        "reason",
                        serde_json::Value::from("pre_send_live_seek_advanced"),
                    ),
                ],
            );
        }
        return true;
    }
    let (speed_at_enqueue, audio_tx_accounting_epoch) = clock.audio_tx_accounting_snapshot();
    let queued_wall_secs =
        duration_secs / speed_at_enqueue.max(crate::video::clock::MIN_PLAYBACK_SPEED);
    let frame_out = AudioFrame {
        samples,
        pts_secs,
        seek_serial: current_seek_serial,
        duration_secs,
        queued_wall_secs,
        audio_tx_accounting_epoch,
        seek_target_secs: current_seek_target_secs,
    };
    // tx queued 合計を **send 前に加算**。pump.recv 後の減算と
    // 競合しないよう順序を保つ。失敗時はロールバック。
    clock.add_audio_tx_queued_secs_for_epoch(queued_wall_secs, audio_tx_accounting_epoch);
    let send_t0 = std::time::Instant::now();
    let mut last_send_wait_log = send_t0;
    let mut pending_frame = Some(frame_out);
    loop {
        if clock.current_seek_serial() != current_seek_serial {
            clock.add_audio_tx_queued_secs_for_epoch(-queued_wall_secs, audio_tx_accounting_epoch);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "audio",
                    "stale_frame_drop",
                    None,
                    0,
                    &[
                        ("pts", serde_json::Value::from(pts_secs)),
                        (
                            "decoder_serial",
                            serde_json::Value::from(current_seek_serial as i64),
                        ),
                        (
                            "live_serial",
                            serde_json::Value::from(clock.current_seek_serial() as i64),
                        ),
                        (
                            "reason",
                            serde_json::Value::from("send_wait_live_seek_advanced"),
                        ),
                    ],
                );
            }
            return true;
        }
        let engine_st = engine_state.load(Ordering::Acquire);
        if engine_state_parks_decode(engine_st) {
            clock.add_audio_tx_queued_secs_for_epoch(-queued_wall_secs, audio_tx_accounting_epoch);
            crate::logger::log(format!(
                "[audio-decode] audio_tx send aborted for park: serial={current_seek_serial} pts={pts_secs:.3} audio_tx_len={} engine_state={} clock_playing={} clock_seeking={}",
                audio_tx.len(),
                engine_state_code_name(engine_st),
                clock.is_playing(),
                clock.is_seeking()
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "audio",
                    "frame_send_aborted_for_park",
                    None,
                    0,
                    &[
                        ("pts", serde_json::Value::from(pts_secs)),
                        (
                            "serial",
                            serde_json::Value::from(current_seek_serial as i64),
                        ),
                        (
                            "audio_tx_len",
                            serde_json::Value::from(audio_tx.len() as i64),
                        ),
                        (
                            "engine_state",
                            serde_json::Value::from(engine_state_code_name(engine_st)),
                        ),
                    ],
                );
            }
            return true;
        }
        let frame = pending_frame
            .take()
            .expect("pending audio frame should exist before send_timeout");
        match audio_tx.send_timeout(frame, std::time::Duration::from_millis(2)) {
            Ok(()) => break,
            Err(crossbeam_channel::SendTimeoutError::Timeout(frame)) => {
                pending_frame = Some(frame);
                let now = std::time::Instant::now();
                let waited = now.duration_since(send_t0);
                if waited >= std::time::Duration::from_millis(100)
                    && now.duration_since(last_send_wait_log)
                        >= std::time::Duration::from_millis(500)
                {
                    last_send_wait_log = now;
                    crate::logger::log(format!(
                        "[audio-decode] audio_tx send blocked {:.1}ms: serial={current_seek_serial} pts={pts_secs:.3} audio_tx_len={} engine_state={} clock_playing={} clock_seeking={}",
                        waited.as_secs_f64() * 1000.0,
                        audio_tx.len(),
                        engine_state_code_name(engine_st),
                        clock.is_playing(),
                        clock.is_seeking()
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "audio",
                            "frame_send_waiting",
                            None,
                            0,
                            &[
                                ("pts", serde_json::Value::from(pts_secs)),
                                (
                                    "wait_ms",
                                    serde_json::Value::from(waited.as_secs_f64() * 1000.0),
                                ),
                                (
                                    "serial",
                                    serde_json::Value::from(current_seek_serial as i64),
                                ),
                                (
                                    "audio_tx_len",
                                    serde_json::Value::from(audio_tx.len() as i64),
                                ),
                                (
                                    "engine_state",
                                    serde_json::Value::from(engine_state_code_name(engine_st)),
                                ),
                                ("clock_playing", serde_json::Value::from(clock.is_playing())),
                                ("clock_seeking", serde_json::Value::from(clock.is_seeking())),
                            ],
                        );
                    }
                }
            }
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                clock.add_audio_tx_queued_secs_for_epoch(
                    -queued_wall_secs,
                    audio_tx_accounting_epoch,
                );
                return false;
            }
        }
    }
    let send_wait_ms = send_t0.elapsed().as_secs_f64() * 1000.0;
    if crate::perf::is_enabled() {
        crate::perf::event(
            "audio",
            "frame",
            None,
            0,
            &[
                ("pts", serde_json::Value::from(pts_secs)),
                ("duration_secs", serde_json::Value::from(duration_secs)),
                (
                    "sample_pairs",
                    serde_json::Value::from(frame_sample_pairs as i64),
                ),
                ("path", serde_json::Value::from(audio_path)),
                (
                    "raw_pts",
                    raw_pts_secs
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("pts_synthesized", serde_json::Value::from(pts_synthesized)),
                (
                    "input_format",
                    serde_json::Value::from(setup.input_format.as_str()),
                ),
                (
                    "decode_wait_ms",
                    decode_wait_ms
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("convert_ms", serde_json::Value::from(convert_ms)),
                ("send_wait_ms", serde_json::Value::from(send_wait_ms)),
                (
                    "total_ms",
                    serde_json::Value::from(emit_t0.elapsed().as_secs_f64() * 1000.0),
                ),
                (
                    "input_channels",
                    serde_json::Value::from(setup.input_channels as i64),
                ),
                (
                    "audio_tx_queued_secs",
                    serde_json::Value::from(clock.audio_tx_queued_secs()),
                ),
                (
                    "seek_serial",
                    serde_json::Value::from(i64::try_from(current_seek_serial).unwrap_or(i64::MAX)),
                ),
            ],
        );
    }
    true
}

fn normalize_audio_input_layout(
    layout: ffmpeg_the_third::ChannelLayout<'_>,
) -> (ffmpeg_the_third::ChannelLayout<'static>, bool) {
    if layout.mask().is_none() && layout.channels() == 2 {
        (ffmpeg_the_third::ChannelLayout::STEREO, true)
    } else {
        (
            ffmpeg_the_third::ChannelLayout::from(layout.into_owned()),
            false,
        )
    }
}

fn audio_context_layout_summary(ctx: &ffmpeg_the_third::codec::context::Context) -> (u32, String) {
    unsafe {
        let avctx = ffmpeg_the_third::AsPtr::as_ptr(ctx);
        let layout = ffmpeg_the_third::ChannelLayout::from(&(*avctx).ch_layout);
        let desc = layout.description();
        (
            layout.channels(),
            if desc.is_empty() {
                "unknown".to_string()
            } else {
                desc
            },
        )
    }
}

fn request_stereo_audio_decoder_output(
    ctx: &mut ffmpeg_the_third::codec::context::Context,
    codec_name: &str,
    input_channels: u32,
    input_layout_desc: &str,
) -> bool {
    // mIV の audio output / VST chain は stereo 固定なので、multichannel decoder には
    // 可能なら最初から stereo 出力を要求する。WMA Pro 5.1ch などで、重い
    // 6ch decode + swresample downmix 経路を避けるための互換ワークアラウンド。
    let _ = ctx;
    if input_channels <= 2 {
        return false;
    }

    crate::logger::log(format!(
        "audio decoder stereo request unavailable: codec={codec_name} input_layout=\"{input_layout_desc}\" input_channels={input_channels}"
    ));
    false
}

#[derive(Clone, Debug)]
struct DownmixEntry {
    plane: usize,
    left: f32,
    right: f32,
    name: String,
}

#[derive(Clone, Debug)]
struct FastDownmixToStereo {
    channels: usize,
    format: ffmpeg_the_third::format::sample::Sample,
    entries: Vec<DownmixEntry>,
}

impl FastDownmixToStereo {
    fn new(
        input_format: ffmpeg_the_third::format::sample::Sample,
        input_layout: &ffmpeg_the_third::ChannelLayout<'_>,
        input_rate: u32,
        output_rate: u32,
    ) -> Option<Self> {
        use ffmpeg_the_third as ffmpeg;

        let channels = input_layout.channels() as usize;
        if !Self::supports_format(input_format) || channels <= 2 || input_rate != output_rate {
            return None;
        }

        let mut entries = Self::entries_for_layout(input_layout, channels);
        if entries.is_empty() {
            // Some legacy files only expose the channel count. Fall back to
            // FFmpeg's default ordering for that count so 5.1/7.1 material can
            // still use the fast path without assuming every file is 5.1.
            let fallback = ffmpeg::ChannelLayout::default_for_channels(channels as u32);
            entries = Self::entries_for_layout(&fallback, channels);
        }
        if entries.is_empty() {
            return None;
        }

        let this = Self {
            channels,
            format: input_format,
            entries,
        };

        let map = this
            .entries
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{:.3}/{:.3}",
                    entry.plane, entry.name, entry.left, entry.right
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        crate::logger::log(format!(
            "audio fast downmix enabled: layout=\"{}\" format={} channels={} map={map}",
            input_layout.description(),
            input_format.name(),
            channels,
        ));
        Some(this)
    }

    fn supports_format(format: ffmpeg_the_third::format::sample::Sample) -> bool {
        use ffmpeg::format::sample::{Sample, Type as SampleType};
        use ffmpeg_the_third as ffmpeg;

        matches!(
            format,
            Sample::F32(SampleType::Planar)
                | Sample::F32(SampleType::Packed)
                | Sample::I32(SampleType::Planar)
                | Sample::I32(SampleType::Packed)
                | Sample::I16(SampleType::Planar)
                | Sample::I16(SampleType::Packed)
        )
    }

    fn entries_for_layout(
        layout: &ffmpeg_the_third::ChannelLayout<'_>,
        channels: usize,
    ) -> Vec<DownmixEntry> {
        // Keep multichannel fold-down below the limiter in dense 5.1/7.1
        // material. This is intentionally conservative; users can still raise
        // gain downstream, while clipping before VST/CPAL is much harder to fix.
        const HEADROOM: f32 = 0.6;

        let mut entries = Vec::new();
        for plane in 0..channels {
            let channel = layout.channel_from_index(plane as u32);
            let (left, right) = Self::stereo_coeffs(channel);
            if left == 0.0 && right == 0.0 {
                continue;
            }
            entries.push(DownmixEntry {
                plane,
                left: left * HEADROOM,
                right: right * HEADROOM,
                name: channel.name(),
            });
        }
        entries
    }

    fn stereo_coeffs(channel: ffmpeg_the_third::Channel) -> (f32, f32) {
        use ffmpeg_the_third::Channel::*;

        const CENTER: f32 = 0.707_106_77;
        const SURROUND: f32 = 0.707_106_77;
        const HEIGHT_SIDE: f32 = 0.707_106_77;
        const HEIGHT_CENTER: f32 = 0.5;

        match channel {
            FrontLeft | StereoLeft => (1.0, 0.0),
            FrontRight | StereoRight => (0.0, 1.0),
            FrontCenter | TopCenter | TopFrontCenter | TopBackCenter | BottomFrontCenter
            | BackCenter => (CENTER, CENTER),
            LowFrequency | LowFrequency2 | Unused | Unknown | None => (0.0, 0.0),
            BackLeft | SideLeft | FrontLeftOfCenter | WideLeft | SurroundDirectLeft => {
                (SURROUND, 0.0)
            }
            BackRight | SideRight | FrontRightOfCenter | WideRight | SurroundDirectRight => {
                (0.0, SURROUND)
            }
            TopFrontLeft | TopBackLeft | TopSideLeft | BottomFrontLeft => (HEIGHT_SIDE, 0.0),
            TopFrontRight | TopBackRight | TopSideRight | BottomFrontRight => (0.0, HEIGHT_SIDE),
            SideSurroundLeft | TopSurroundLeft => (HEIGHT_SIDE, 0.0),
            SideSurroundRight | TopSurroundRight => (0.0, HEIGHT_SIDE),
            AmbisonicBase | AmbisonicEnd => (HEIGHT_CENTER, HEIGHT_CENTER),
        }
    }

    fn run(&self, frame: &ffmpeg_the_third::util::frame::audio::Audio) -> Option<Vec<f32>> {
        let nb_samples = frame.samples();
        if nb_samples == 0 {
            return None;
        }

        use ffmpeg::format::sample::{Sample, Type as SampleType};
        use ffmpeg_the_third as ffmpeg;

        match self.format {
            Sample::F32(SampleType::Planar) => {
                self.mix_planar(frame, |ptr, i| unsafe { *(ptr as *const f32).add(i) })
            }
            Sample::F32(SampleType::Packed) => {
                self.mix_packed(frame, |ptr, i| unsafe { *(ptr as *const f32).add(i) })
            }
            Sample::I32(SampleType::Planar) => self.mix_planar(frame, |ptr, i| unsafe {
                *(ptr as *const i32).add(i) as f32 / 2_147_483_648.0
            }),
            Sample::I32(SampleType::Packed) => self.mix_packed(frame, |ptr, i| unsafe {
                *(ptr as *const i32).add(i) as f32 / 2_147_483_648.0
            }),
            Sample::I16(SampleType::Planar) => self.mix_planar(frame, |ptr, i| unsafe {
                *(ptr as *const i16).add(i) as f32 / 32_768.0
            }),
            Sample::I16(SampleType::Packed) => self.mix_packed(frame, |ptr, i| unsafe {
                *(ptr as *const i16).add(i) as f32 / 32_768.0
            }),
            _ => None,
        }
    }

    fn mix_planar<F>(
        &self,
        frame: &ffmpeg_the_third::util::frame::audio::Audio,
        mut sample_at: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(*const u8, usize) -> f32,
    {
        let nb_samples = frame.samples();
        let mut out = vec![0.0; nb_samples * 2];
        for entry in &self.entries {
            let plane = self.plane_ptr(frame, entry.plane)?;
            for i in 0..nb_samples {
                let sample = sample_at(plane, i);
                let out_idx = i * 2;
                out[out_idx] += sample * entry.left;
                out[out_idx + 1] += sample * entry.right;
            }
        }
        Some(out)
    }

    fn mix_packed<F>(
        &self,
        frame: &ffmpeg_the_third::util::frame::audio::Audio,
        mut sample_at: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(*const u8, usize) -> f32,
    {
        let nb_samples = frame.samples();
        let base = self.packed_ptr(frame)?;
        let mut out = vec![0.0; nb_samples * 2];
        for i in 0..nb_samples {
            let frame_base = i * self.channels;
            let out_idx = i * 2;
            for entry in &self.entries {
                let sample = sample_at(base, frame_base + entry.plane);
                out[out_idx] += sample * entry.left;
                out[out_idx + 1] += sample * entry.right;
            }
        }
        Some(out)
    }

    fn plane_ptr(
        &self,
        frame: &ffmpeg_the_third::util::frame::audio::Audio,
        plane: usize,
    ) -> Option<*const u8> {
        unsafe {
            let av = frame.as_ptr();
            let data = if !(*av).extended_data.is_null() {
                (*av).extended_data
            } else {
                (*av).data.as_ptr() as *mut *mut u8
            };
            if data.is_null() {
                return None;
            }
            let ptr = *data.add(plane) as *const u8;
            if ptr.is_null() {
                return None;
            }
            Some(ptr)
        }
    }

    fn packed_ptr(&self, frame: &ffmpeg_the_third::util::frame::audio::Audio) -> Option<*const u8> {
        unsafe {
            let av = frame.as_ptr();
            let ptr = (*av).data[0] as *const u8;
            if ptr.is_null() { None } else { Some(ptr) }
        }
    }
}

struct AudioSetup {
    stream_idx: usize,
    out_rate: u32,
    input_rate: u32,
    input_channels: u32,
    input_layout_desc: String,
    input_format: String,
    output_channels: u32,
    decoder_stereo_requested: bool,
    decoder_stereo_effective: bool,
    fast_downmix: Option<FastDownmixToStereo>,
    time_base_num: f64,
    time_base_den: f64,
    decoder: ffmpeg_the_third::decoder::Audio,
    resampler: ffmpeg_the_third::software::resampling::Context,
    codec_name: String,
}

/// `av_hwdevice_ctx_create` で確保した AVBufferRef を保持し、Drop で `av_buffer_unref`
/// する RAII ラッパー。AVCodecContext は内部で別途 `av_buffer_ref` するので、ここで
/// drop しても codec 側のライフタイムには影響しない (refcount 管理)。
struct HwDevice {
    buf_ref: *mut ffmpeg_the_third::ffi::AVBufferRef,
}

impl Drop for HwDevice {
    fn drop(&mut self) {
        unsafe {
            ffmpeg_the_third::ffi::av_buffer_unref(&mut self.buf_ref);
        }
    }
}

// HwDevice は単独で thread を渡らないが、`run_decoder` の所有スコープ内で持つだけ
// なので Send/Sync は不要。明示的に impl しない。

struct OpenedVideoDecoder {
    decoder: ffmpeg_the_third::decoder::Video,
    hw_device: Option<HwDevice>,
    decoder_name: String,
    hw_probe: D3d11vaProbe,
}

#[derive(Clone)]
struct D3d11vaProbe {
    d3d11va_supported: bool,
    d3d11va_config: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecoderChoice {
    Default,
    ByName(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VideoDecoderCandidate {
    choice: DecoderChoice,
    reason: &'static str,
    allow_sw_fallback: bool,
}

fn preferred_video_decoders(
    codec_id: ffmpeg_the_third::codec::Id,
    hw_decode_requested: bool,
) -> Vec<VideoDecoderCandidate> {
    let mut candidates = Vec::new();
    if hw_decode_requested && codec_id == ffmpeg_the_third::codec::Id::AV1 {
        candidates.push(VideoDecoderCandidate {
            choice: DecoderChoice::ByName("av1"),
            reason: "av1_hw_preferred",
            allow_sw_fallback: false,
        });
    }
    candidates.push(VideoDecoderCandidate {
        choice: DecoderChoice::Default,
        reason: "default",
        allow_sw_fallback: true,
    });
    candidates
}

fn clone_codec_parameters(
    params: &ffmpeg_the_third::codec::ParametersRef<'_>,
) -> Result<ffmpeg_the_third::codec::Parameters, String> {
    let mut cloned = ffmpeg_the_third::codec::Parameters::new();
    let ret = unsafe {
        ffmpeg_the_third::ffi::avcodec_parameters_copy(cloned.as_mut_ptr(), params.as_ptr())
    };
    if ret < 0 {
        Err(format!("avcodec_parameters_copy failed: {ret}"))
    } else {
        Ok(cloned)
    }
}

fn resolve_video_decoder_candidate(
    codec_id: ffmpeg_the_third::codec::Id,
    candidate: VideoDecoderCandidate,
) -> Option<ffmpeg_the_third::Codec> {
    let codec = match candidate.choice {
        DecoderChoice::Default => ffmpeg_the_third::codec::decoder::find(codec_id),
        DecoderChoice::ByName(name) => ffmpeg_the_third::codec::decoder::find_by_name(name),
    }?;
    if codec.id() != codec_id {
        crate::logger::log(format!(
            "video decoder candidate skipped: codec={} candidate={} reason={} stage=id_mismatch candidate_id={}",
            codec_id.name(),
            codec.name(),
            candidate.reason,
            codec.id().name(),
        ));
        return None;
    }
    Some(codec)
}

fn open_video_decoder_with_candidates(
    video_params: &ffmpeg_the_third::codec::Parameters,
    codec_id: ffmpeg_the_third::codec::Id,
    hw_decode_requested: bool,
    #[cfg(windows)] gpu_video_device: Option<
        &std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
    >,
) -> Result<OpenedVideoDecoder, String> {
    let candidates = preferred_video_decoders(codec_id, hw_decode_requested);
    let mut errors = Vec::<String>::new();

    for candidate in candidates {
        let Some(codec) = resolve_video_decoder_candidate(codec_id, candidate) else {
            let label = match candidate.choice {
                DecoderChoice::Default => "default".to_string(),
                DecoderChoice::ByName(name) => name.to_string(),
            };
            errors.push(format!("{label}: decoder not found"));
            continue;
        };
        let decoder_name = codec.name().to_string();
        let hw_probe = probe_d3d11va_for_codec(codec);
        crate::logger::log(format!(
            "video decoder candidate: codec={} decoder={decoder_name} reason={} allow_sw_fallback={} hw_requested={hw_decode_requested} d3d11va_supported={} d3d11va_config={}",
            codec_id.name(),
            candidate.reason,
            candidate.allow_sw_fallback,
            hw_probe.d3d11va_supported,
            hw_probe.d3d11va_config,
        ));

        let should_try_hw = hw_decode_requested && hw_probe.d3d11va_supported;
        if should_try_hw {
            let mut ctx =
                ffmpeg_the_third::codec::context::Context::from_parameters(video_params.clone())
                    .map_err(|e| format!("{decoder_name}: context: {e}"))?;
            let hw_device = {
                #[cfg(windows)]
                {
                    try_init_d3d11va_for_codec(&decoder_name, codec, &mut ctx, gpu_video_device)
                }
                #[cfg(not(windows))]
                {
                    try_init_d3d11va_for_codec(&decoder_name, codec, &mut ctx)
                }
            };
            if let Some(hw_device) = hw_device {
                match ctx.decoder().open_as(codec).and_then(|o| o.video()) {
                    Ok(decoder) => {
                        crate::logger::log(format!(
                            "video decoder selected: codec={} decoder={decoder_name} reason={} decode_path=hw_d3d11va",
                            codec_id.name(),
                            candidate.reason
                        ));
                        return Ok(OpenedVideoDecoder {
                            decoder,
                            hw_device: Some(hw_device),
                            decoder_name,
                            hw_probe,
                        });
                    }
                    Err(e) => {
                        crate::logger::log(format!(
                            "video decoder candidate failed: codec={} decoder={decoder_name} reason={} stage=open_hw err={e}",
                            codec_id.name(),
                            candidate.reason
                        ));
                        errors.push(format!("{decoder_name}: open_hw: {e}"));
                        if !candidate.allow_sw_fallback {
                            continue;
                        }
                    }
                }
            } else {
                crate::logger::log(format!(
                    "video decoder candidate failed: codec={} decoder={decoder_name} reason={} stage=hw_init",
                    codec_id.name(),
                    candidate.reason
                ));
                if !candidate.allow_sw_fallback {
                    errors.push(format!("{decoder_name}: hw_init"));
                    continue;
                }
            }
        } else if hw_decode_requested && !candidate.allow_sw_fallback {
            crate::logger::log(format!(
                "video decoder candidate failed: codec={} decoder={decoder_name} reason={} stage=hw_probe",
                codec_id.name(),
                candidate.reason
            ));
            errors.push(format!("{decoder_name}: no_d3d11va"));
            continue;
        }

        if candidate.allow_sw_fallback {
            let ctx =
                ffmpeg_the_third::codec::context::Context::from_parameters(video_params.clone())
                    .map_err(|e| format!("{decoder_name}: context_sw: {e}"))?;
            match ctx.decoder().open_as(codec).and_then(|o| o.video()) {
                Ok(decoder) => {
                    crate::logger::log(format!(
                        "video decoder selected: codec={} decoder={decoder_name} reason={} decode_path=sw",
                        codec_id.name(),
                        candidate.reason
                    ));
                    return Ok(OpenedVideoDecoder {
                        decoder,
                        hw_device: None,
                        decoder_name,
                        hw_probe,
                    });
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "video decoder candidate failed: codec={} decoder={decoder_name} reason={} stage=open_sw err={e}",
                        codec_id.name(),
                        candidate.reason
                    ));
                    errors.push(format!("{decoder_name}: open_sw: {e}"));
                }
            }
        }
    }

    Err(errors.join("; "))
}

/// 指定 decoder について、D3D11VA の候補を列挙する。
/// 実際に HW が使われるかは device 作成と get_format の結果で確定するため、
/// ここでは「この decoder が D3D11VA を宣言しているか」だけを記録する。
fn probe_d3d11va_for_codec(codec: ffmpeg_the_third::Codec) -> D3d11vaProbe {
    use ffmpeg_the_third::ffi::*;

    unsafe {
        let mut configs = Vec::new();
        let mut d3d11va_supported = false;
        for i in 0_i32.. {
            let cfg = avcodec_get_hw_config(codec.as_ptr(), i);
            if cfg.is_null() {
                break;
            }
            let cfg = &*cfg;
            if cfg.device_type == AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA {
                let has_device_ctx =
                    (cfg.methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0;
                let has_d3d11_pix_fmt = cfg.pix_fmt == AVPixelFormat::AV_PIX_FMT_D3D11;
                if has_device_ctx && has_d3d11_pix_fmt {
                    d3d11va_supported = true;
                }
                configs.push(format!(
                    "idx={i},pix_fmt={:?},methods=0x{:x},device_ctx={has_device_ctx}",
                    cfg.pix_fmt, cfg.methods
                ));
            }
        }

        D3d11vaProbe {
            d3d11va_supported,
            d3d11va_config: if configs.is_empty() {
                "none".to_string()
            } else {
                configs.join(";")
            },
        }
    }
}

/// D3D11VA HW デコードを試みる。サポート確認 → デバイス作成 → AVCodecContext へ装着
/// までを行い、成功時は `Some(HwDevice)` を返す。失敗 (codec 非対応 / device 作成失敗)
/// 時は `None` を返し、SW で続行する。
///
/// `gpu_video_device` が `Some` の場合は **そのデバイスを FFmpeg と共有** する
/// (= mIV の VideoProcessor が同じデバイスで動作可能、CreateVideoProcessorInputView
/// の前提)。`None` の場合は FFmpeg が独自デバイスを作る (旧経路)。
fn try_init_d3d11va_for_codec(
    codec_name_for_log: &str,
    codec: ffmpeg_the_third::Codec,
    ctx: &mut ffmpeg_the_third::codec::context::Context,
    #[cfg(windows)] gpu_video_device: Option<
        &std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
    >,
) -> Option<HwDevice> {
    use ffmpeg_the_third::ffi::*;

    unsafe {
        let probe = probe_d3d11va_for_codec(codec);
        if !probe.d3d11va_supported {
            crate::logger::log(format!(
                "HW: decoder {codec_name_for_log} does not support D3D11VA HW_DEVICE_CTX"
            ));
            return None;
        }

        // HW デバイスコンテキスト作成。
        //    gpu_video_device があれば mIV の D3D11 デバイスを共有、なければ FFmpeg
        //    が新デバイスを作る (= 旧経路)。
        #[cfg(windows)]
        let buf_ref: *mut AVBufferRef = if let Some(gpu_dev) = gpu_video_device {
            match crate::video::gpu_renderer::create_ffmpeg_hw_device_ctx(gpu_dev) {
                Ok(b) => {
                    crate::logger::log(format!(
                        "HW: D3D11VA shared with GpuVideoDevice for decoder {codec_name_for_log}"
                    ));
                    b
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "HW: shared D3D11 setup failed ({e}), falling back to ffmpeg-owned device"
                    ));
                    let mut buf: *mut AVBufferRef = std::ptr::null_mut();
                    let ret = av_hwdevice_ctx_create(
                        &mut buf,
                        AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        0,
                    );
                    if ret < 0 || buf.is_null() {
                        crate::logger::log(format!(
                            "HW: av_hwdevice_ctx_create(D3D11VA) failed: {ret}"
                        ));
                        return None;
                    }
                    buf
                }
            }
        } else {
            let mut buf: *mut AVBufferRef = std::ptr::null_mut();
            let ret = av_hwdevice_ctx_create(
                &mut buf,
                AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            );
            if ret < 0 || buf.is_null() {
                crate::logger::log(format!("HW: av_hwdevice_ctx_create(D3D11VA) failed: {ret}"));
                return None;
            }
            buf
        };
        #[cfg(not(windows))]
        let buf_ref: *mut AVBufferRef = {
            let mut buf: *mut AVBufferRef = std::ptr::null_mut();
            let ret = av_hwdevice_ctx_create(
                &mut buf,
                AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            );
            if ret < 0 || buf.is_null() {
                crate::logger::log(format!("HW: av_hwdevice_ctx_create(D3D11VA) failed: {ret}"));
                return None;
            }
            buf
        };

        // AVCodecContext にぶら下げる + get_format コールバック
        let avctx = ctx.as_mut_ptr();
        let new_ref = av_buffer_ref(buf_ref);
        if new_ref.is_null() {
            let mut to_free = buf_ref;
            av_buffer_unref(&mut to_free);
            crate::logger::log("HW: av_buffer_ref returned null".to_string());
            return None;
        }
        (*avctx).hw_device_ctx = new_ref;
        (*avctx).get_format = Some(get_hw_format);

        crate::logger::log(format!(
            "HW: D3D11VA initialized for decoder {codec_name_for_log}"
        ));
        Some(HwDevice { buf_ref })
    }
}

/// `AVCodecContext.get_format` コールバック。D3D11 が候補にあれば選択、無ければ
/// 先頭の SW フォーマットにフォールバックして libavcodec を SW デコードに退避させる。
unsafe extern "C" fn get_hw_format(
    _ctx: *mut ffmpeg_the_third::ffi::AVCodecContext,
    fmt_list: *const ffmpeg_the_third::ffi::AVPixelFormat,
) -> ffmpeg_the_third::ffi::AVPixelFormat {
    use ffmpeg_the_third::ffi::AVPixelFormat;
    if fmt_list.is_null() {
        return AVPixelFormat::AV_PIX_FMT_NONE;
    }
    unsafe {
        let mut p = fmt_list;
        let mut first = AVPixelFormat::AV_PIX_FMT_NONE;
        let mut idx = 0;
        while *p != AVPixelFormat::AV_PIX_FMT_NONE {
            if idx == 0 {
                first = *p;
            }
            if *p == AVPixelFormat::AV_PIX_FMT_D3D11 {
                return AVPixelFormat::AV_PIX_FMT_D3D11;
            }
            p = p.add(1);
            idx += 1;
        }
        crate::logger::log(format!(
            "HW: get_format: D3D11 not in candidate list, falling back to {first:?}"
        ));
        first
    }
}

fn clamp_dims(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    if w <= max_dim && h <= max_dim {
        return (w, h);
    }
    let scale = max_dim as f64 / w.max(h) as f64;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    (nw, nh)
}

fn duration_to_secs(dur: i64) -> f64 {
    // FFmpeg の duration は AV_TIME_BASE (= 1_000_000) 単位
    if dur <= 0 {
        return 0.0;
    }
    dur as f64 / 1_000_000.0
}

/// AVFormatContext のグローバル metadata から、与えられたキー候補を順番に試して
/// 最初に値が取れたもの (空文字でない) を返す。Phase 5.4 の埋め込みメタ抽出用。
fn read_metadata_value(
    input: &ffmpeg_the_third::format::context::Input,
    keys: &[&str],
) -> Option<String> {
    let dict = input.metadata();
    for k in keys {
        if let Some(v) = dict.get(k) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    for (key, value) in dict.iter() {
        if keys
            .iter()
            .any(|candidate| key.eq_ignore_ascii_case(candidate))
            && !value.trim().is_empty()
        {
            return Some(value.to_string());
        }
    }
    None
}

fn read_metadata_http_url(
    input: &ffmpeg_the_third::format::context::Input,
    keys: &[&str],
) -> Option<String> {
    read_metadata_value(input, keys)
        .and_then(|value| crate::external_links::normalize_http_url(&value))
}

/// AV_PIX_FMT_D3D11 の AVFrame を mIV 側 D3D11 デバイス上で NV12→RGBA blit して
/// `VideoFrame::Gpu(D3d11Frame)` を作る (CPU readback + swscale を完全に省略する経路)。
///
/// `AVFrame.data[0]` = `*mut ID3D11Texture2D`、`data[1]` = subresource index (intptr) が
/// AV_PIX_FMT_D3D11 の規約 ([ffmpeg-sys-the-third bindings.rs] AV_PIX_FMT_D3D11 の
/// rustdoc 参照)。これらをそのまま `GpuVideoDevice::blit_nv12_to_rgba` に渡す。
///
/// Drop 後の AVFrame は input texture を Release するので、blit 完了 (= GPU 命令が
/// キューに積まれる) 後に AVFrame を解放するだけで安全 (D3D11 driver 内部で
/// テクスチャの寿命管理がされる)。
#[cfg(windows)]
fn try_gpu_blit_path(
    gpu_dev: &std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
    frame: &ffmpeg_the_third::util::frame::Video,
    dst_w: u32,
    dst_h: u32,
    pts_secs: f64,
    current_seek_serial: u64,
    first_frame_logged: &mut bool,
    hw_active_initially: bool,
    fps_num: u32,
    fps_den: u32,
) -> Result<VideoFrame, crate::video::gpu_renderer::GpuVideoError> {
    use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
    use windows::core::Interface;

    // SAFETY: AV_PIX_FMT_D3D11 の data[0] は ID3D11Texture2D* で、AVFrame の生存中有効。
    // data[1] は intptr_t としての subresource index。
    let (texture_raw, subresource) = unsafe {
        let raw = frame.as_ptr();
        let data0 = (*raw).data[0];
        let data1 = (*raw).data[1] as usize;
        (data0 as *mut std::ffi::c_void, data1 as u32)
    };
    if texture_raw.is_null() {
        return Err(crate::video::gpu_renderer::GpuVideoError::Blt(
            "AVFrame.data[0] is null".into(),
        ));
    }

    // ID3D11Texture2D を `windows` 0.61 系の COM インタフェースとして包む (AddRef)。
    // SAFETY: COM のキャスト規約に従い、IUnknown 互換 vtable を持つ raw ポインタを
    // 安全な COM ハンドルに昇格させる。
    let texture: ID3D11Texture2D = unsafe {
        // from_raw_borrowed は Option<&Self> を返すが、追加 ref を取るために
        // clone() してから .map(...).unwrap() で値化する。
        let opt: Option<&ID3D11Texture2D> = ID3D11Texture2D::from_raw_borrowed(&texture_raw);
        match opt {
            Some(t) => t.clone(),
            None => {
                return Err(crate::video::gpu_renderer::GpuVideoError::Blt(
                    "from_raw_borrowed null".into(),
                ));
            }
        }
    };

    // 入力 D3D11 フォーマットから 10-bit を判定 (P010 / P016 → 10-bit、NV12 → 8-bit)。
    let in_desc = unsafe {
        let mut d = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut d);
        d
    };
    let source_ten_bit = matches!(
        in_desc.Format,
        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_P010
            | windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_P016
    );

    let (final_w, final_h) = (dst_w, dst_h);

    // GPU で NV12→RGBA 変換、出力は NT 共有テクスチャ。
    // active_w/active_h は AVFrame の論理寸法 (= 実画像領域)。FFmpeg HW frames は
    // 16 アライン由来で texture 寸法が大きい場合があるので、ここで active 領域を渡す。
    let active_w = frame.width();
    let active_h = frame.height();
    // 色空間ヒント: FFmpeg の transfer characteristic から HDR PQ / HDR HLG / SDR を判定。
    // 多くの SDR 動画は transfer 未指定 (UNSPECIFIED) で来るが、その場合は SDR にフォール
    // バック。VPP は色空間情報なしより、何か明示された方が良い結果になる。
    let color_hint = {
        use ffmpeg_the_third::util::color::TransferCharacteristic as Trc;
        match frame.color_transfer_characteristic() {
            Trc::SMPTE2084 => crate::video::gpu_renderer::VideoColorHint::HdrPq,
            Trc::ARIB_STD_B67 => crate::video::gpu_renderer::VideoColorHint::HdrHlg,
            _ => crate::video::gpu_renderer::VideoColorHint::Sdr,
        }
    };
    let blit = unsafe {
        gpu_dev.blit_nv12_to_rgba(
            &texture,
            subresource,
            active_w,
            active_h,
            final_w,
            final_h,
            source_ten_bit,
            color_hint,
            fps_num,
            fps_den,
            pts_secs,
        )?
    };
    // `output_texture` は D3D11 側の COM オブジェクト。NT shared handle 経由で
    // D3D12 側に開いてもらう運用なので、ここで drop しても問題ない (NT カーネル
    // オブジェクトの refcount で D3D12 側のリソースは生存)。
    let _ = blit.output_texture;
    let shared_handle = blit.shared_handle;
    let fence_value = blit.fence_value;
    let fence_shared_handle = gpu_dev.fence_shared_handle();
    let fence_gen = gpu_dev.fence_gen();

    // perf: 初回フレームは GPU 経路を明示
    if !*first_frame_logged && crate::perf::is_enabled() {
        crate::perf::event(
            "video",
            "first_frame",
            None,
            0,
            &[
                (
                    "decode_path",
                    serde_json::Value::from("hw_d3d11va_gpu_blit"),
                ),
                (
                    "frame_pix_fmt",
                    serde_json::Value::from(format!("{:?}", in_desc.Format)),
                ),
                ("frame_w", serde_json::Value::from(in_desc.Width as i64)),
                ("frame_h", serde_json::Value::from(in_desc.Height as i64)),
                ("hw_active", serde_json::Value::from(hw_active_initially)),
            ],
        );
        *first_frame_logged = true;
    }

    let d3d11_frame = crate::video::gpu_renderer::D3d11Frame {
        width: final_w,
        height: final_h,
        pts_secs,
        seek_serial: current_seek_serial,
        shared_handle,
        close_shared_handle_on_drop: blit.close_shared_handle_on_drop,
        shared_output_in_use: blit.shared_output_in_use,
        shared_output_notify: blit.shared_output_notify,
        shared_output_keyed_mutex: blit.shared_output_keyed_mutex,
        shared_output_released_to_reader: blit.shared_output_released_to_reader,
        // Display output is normalized to BGRA8. The source bit depth is used
        // only while configuring the D3D11 video processor input/color space.
        ten_bit: false,
        fence_value,
        fence_shared_handle,
        fence_gen,
    };
    Ok(VideoFrame {
        width: final_w,
        height: final_h,
        data: VideoFrameData::Gpu(d3d11_frame),
        pts_secs,
        seek_serial: current_seek_serial,
    })
}

#[cfg(test)]
mod decoder_candidate_tests {
    use super::{
        BwdifFilterKey, DecoderChoice, bwdif_filter_key, bwdif_force_all_frames,
        field_order_is_interlaced, normalize_audio_input_layout, preferred_video_decoders,
        selected_video_rate, should_try_deinterlace,
    };
    use crate::settings::VideoDeinterlaceMode;
    use ffmpeg_the_third::ChannelLayout;
    use ffmpeg_the_third::FieldOrder;
    use ffmpeg_the_third::codec::Id;
    use ffmpeg_the_third::format::Pixel;
    use ffmpeg_the_third::util::frame::video::Video;

    #[test]
    fn unspecified_two_channel_audio_layout_is_guessed_as_stereo() {
        let (layout, guessed) = normalize_audio_input_layout(ChannelLayout::unspecified(2));
        assert!(guessed);
        assert_eq!(layout.mask(), ChannelLayout::STEREO.mask());
    }

    #[test]
    fn specified_stereo_audio_layout_is_left_unchanged() {
        let (layout, guessed) = normalize_audio_input_layout(ChannelLayout::STEREO);
        assert!(!guessed);
        assert_eq!(layout.mask(), ChannelLayout::STEREO.mask());
    }

    #[test]
    fn unspecified_non_stereo_audio_layout_is_not_guessed() {
        let (layout, guessed) = normalize_audio_input_layout(ChannelLayout::unspecified(6));
        assert!(!guessed);
        assert!(layout.mask().is_none());
        assert_eq!(layout.channels(), 6);
    }

    #[test]
    fn h264_hevc_have_single_default_candidate_even_with_hw() {
        for id in [Id::H264, Id::HEVC] {
            let candidates = preferred_video_decoders(id, true);
            assert_eq!(candidates.len(), 1, "{id:?}");
            assert_eq!(candidates[0].choice, DecoderChoice::Default);
            assert!(candidates[0].allow_sw_fallback);
        }
    }

    #[test]
    fn av1_hw_prefers_native_then_default() {
        let candidates = preferred_video_decoders(Id::AV1, true);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].choice, DecoderChoice::ByName("av1"));
        assert!(!candidates[0].allow_sw_fallback);
        assert_eq!(candidates[1].choice, DecoderChoice::Default);
        assert!(candidates[1].allow_sw_fallback);
    }

    #[test]
    fn av1_without_hw_uses_default_only() {
        let candidates = preferred_video_decoders(Id::AV1, false);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].choice, DecoderChoice::Default);
        assert!(candidates[0].allow_sw_fallback);
    }

    #[test]
    fn vp9_uses_default_only_even_with_hw() {
        let candidates = preferred_video_decoders(Id::VP9, true);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].choice, DecoderChoice::Default);
        assert!(candidates[0].allow_sw_fallback);
    }

    #[test]
    fn video_rate_falls_back_to_stream_rate_when_average_is_missing() {
        let rate = selected_video_rate(
            ffmpeg_the_third::Rational(0, 0),
            ffmpeg_the_third::Rational(24, 1),
        );
        assert_eq!(rate, Some((24, 1)));
    }

    #[test]
    fn field_order_marks_interlaced_streams() {
        for order in [
            FieldOrder::TT,
            FieldOrder::BB,
            FieldOrder::TB,
            FieldOrder::BT,
        ] {
            assert!(field_order_is_interlaced(order), "{order:?}");
        }
        assert!(!field_order_is_interlaced(FieldOrder::Progressive));
        assert!(!field_order_is_interlaced(FieldOrder::Unknown));
    }

    #[test]
    fn auto_deinterlace_uses_stream_field_order_hint() {
        assert!(should_try_deinterlace(
            VideoDeinterlaceMode::Auto,
            false,
            true,
            false
        ));
        assert!(bwdif_force_all_frames(
            VideoDeinterlaceMode::Auto,
            false,
            true
        ));
    }

    #[test]
    fn auto_deinterlace_keeps_frame_flag_mode_when_available() {
        assert!(should_try_deinterlace(
            VideoDeinterlaceMode::Auto,
            true,
            false,
            false
        ));
        assert!(!bwdif_force_all_frames(
            VideoDeinterlaceMode::Auto,
            true,
            false
        ));
    }

    #[test]
    fn off_and_failure_disable_deinterlace() {
        assert!(!should_try_deinterlace(
            VideoDeinterlaceMode::Off,
            true,
            true,
            false
        ));
        assert!(!should_try_deinterlace(
            VideoDeinterlaceMode::Auto,
            true,
            true,
            true
        ));
    }

    #[test]
    fn bwdif_filter_key_normalizes_missing_sar_and_time_base() {
        let mut frame = Video::empty();
        frame.set_format(Pixel::YUV420P);
        frame.set_width(720);
        frame.set_height(480);

        let key = bwdif_filter_key(&frame, 0, 0);
        assert_eq!(
            key,
            BwdifFilterKey {
                pix_fmt: Pixel::YUV420P,
                width: 720,
                height: 480,
                sar_num: 1,
                sar_den: 1,
                time_base_num: 1,
                time_base_den: 1,
            }
        );
    }

    #[test]
    fn bwdif_filter_reports_missing_filter() {
        let key = BwdifFilterKey {
            pix_fmt: Pixel::None,
            width: 0,
            height: 0,
            sar_num: 1,
            sar_den: 1,
            time_base_num: 1,
            time_base_den: 1,
        };
        let err = match super::BwdifFilter::new(key, false) {
            Ok(_) => panic!("invalid bwdif key unexpectedly created a filter graph"),
            Err(err) => err,
        };
        assert!(
            err.contains("buffer init") || err.contains("graph"),
            "unexpected error: {err}"
        );
    }
}
