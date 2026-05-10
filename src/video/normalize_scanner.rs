//! 動画音量ノーマライズ用 EBU R128 (LUFS) 測定。
//!
//! ## 役割
//! 指定された動画ファイルの音声を最初から最後まで decode し、ffmpeg の `ebur128` filter
//! に流して integrated loudness (LUFS) と true peak (dBTP) を取得する。ターゲット LUFS との
//! 差分から適用ゲイン (dB) を算出して `NormalizeResult` で返す。
//!
//! ## スレッド前提
//! 同期 worker thread から呼ばれることを想定。長時間 (動画全長を CPU 単独でデコード) かかる
//! ため UI スレッドからは絶対に呼ばないこと。`cancel: Arc<AtomicBool>` を共有して
//! UI スレッドからキャンセル可能にする。
//!
//! ## 進捗
//! `progress: Arc<NormalizeScanProgress>` の atomic に処理済み PTS (ミリ秒) を書き込む。
//! 動画 duration が 0 / 不明の場合は `indeterminate=true` を立てて UI 側にスピナー表示
//! を促す。
//!
//! ## アルゴリズム
//! 1. abuffer (decoder native fmt/rate/layout) → aformat=stereo,flt,48000 → ebur128 → abuffersink
//! 2. 各 packet を decode → frame ごとに graph に push、sink から受け取った frame の metadata
//!    から `lavfi.r128.M` (momentary) と `lavfi.r128.I` (integrated) と `lavfi.r128.true_peak`
//!    (linear、要 dB 変換) を取得
//! 3. EOF 後に最終 metadata を見て gain_db を計算:
//!    - `gain_db_raw = target_lufs - integrated_lufs`
//!    - `true_peak_after_gain_db = true_peak_db + gain_db_raw`
//!    - `true_peak_db <= -1` を維持するよう gain_db を絞る (= clip 防止)
//!    - 最後に `±24dB` にクランプ
//! 4. integrated LUFS が `-inf` (= 完全無音) の場合は `Err(SilentInput)` を返す。
//!    UI 側で `[OnUnmeasured]` に戻して通知する。
//!
//! ## 短尺動画の信頼性
//! BS.1770-4 integrated は 30 秒以下では信頼性が低い。fallback として scan 中観測した
//! `lavfi.r128.M` (momentary) の最大値を使う。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::media::Type as MediaType;
use ffmpeg::util::frame::audio::Audio;
use ffmpeg_the_third as ffmpeg;

use crate::video::normalize_types::NormalizeResult;

/// EBU R128 が integrated LUFS を信頼できる動画長 (秒)。
const MIN_RELIABLE_DURATION_SECS: f64 = 30.0;
/// scanner 内で固定する出力 sample rate。filter graph の aformat で揃える。
///
/// ⚠️ 再生側 (`decoder.rs::audio_setup`) は cpal の出力デバイス sample rate を使うため
/// 環境次第で 44.1kHz / 96kHz 等になりうる。ただし EBU R128 / BS.1770 K-weighting は
/// rate 非依存設計で、48kHz と 44.1kHz の integrated LUFS 差は通常 0.1dB 以下なので
/// scanner は 48k 固定で実用上問題ない。再生側 `FastDownmixToStereo` が使われる
/// 5.1/7.1 素材では downmix 係数が ffmpeg `aformat` のデフォルト (BS.775) と若干異なる
/// 可能性があるが、ノーマライズ用途では BS.775 の方が放送基準に沿う。
const TARGET_RATE: u32 = 48_000;
/// EAGAIN の errno (Windows MSVC libc)。
const EAGAIN_ERRNO: i32 = 11;

/// スキャン進捗の atomic 構造体。worker から書き込み、UI 側が `Acquire` で読む。
#[derive(Default, Debug)]
pub struct NormalizeScanProgress {
    /// 処理済み PTS (ミリ秒)。
    pub pts_processed_ms: AtomicU64,
    /// 動画の総 duration (ミリ秒)。0 のまま動かない場合は `indeterminate` を見る。
    pub duration_ms: AtomicU64,
    /// duration 不明 / 取れない動画 (live stream 等) なら true。UI はスピナー表示する。
    pub indeterminate: AtomicBool,
}

#[derive(Debug)]
pub enum NormalizeScanError {
    /// FFmpeg 呼び出しが失敗した (詳細メッセージ付き)。
    Ffmpeg(String),
    /// 動画に音声ストリームがなかった。
    NoAudio,
    /// 完全無音 (integrated LUFS = -inf)。測定不能なので UI 側 OnUnmeasured に戻す。
    SilentInput,
    /// 計算結果が finite でない (defensive)。
    InvalidLoudness,
    /// ユーザーがキャンセルした。
    Cancelled,
}

impl std::fmt::Display for NormalizeScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ffmpeg(s) => write!(f, "FFmpeg error: {s}"),
            Self::NoAudio => write!(f, "動画に音声ストリームがありません"),
            Self::SilentInput => write!(f, "音声がほぼ無音のため測定できません"),
            Self::InvalidLoudness => write!(f, "音量測定値が異常 (NaN/Inf) のため適用できません"),
            Self::Cancelled => write!(f, "ユーザーキャンセル"),
        }
    }
}

impl std::error::Error for NormalizeScanError {}

/// メイン関数。
///
/// `target_lufs_milli` は LUFS の千分の一単位 (例 `-14000` = -14.000 LUFS)。
/// 内部計算用に `target_lufs_milli as f32 / 1000.0` で float に戻して使う。
pub fn scan_audio_loudness(
    path: &Path,
    target_lufs_milli: i32,
    cancel: Arc<AtomicBool>,
    progress: Arc<NormalizeScanProgress>,
) -> Result<NormalizeResult, NormalizeScanError> {
    let target_lufs = target_lufs_milli as f32 / 1000.0;

    let mut input = ffmpeg::format::input(&path)
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("format::input: {e}")))?;

    // duration を progress に publish。0 / negative なら indeterminate。
    let duration_secs = duration_to_secs(input.duration());
    if duration_secs > 0.0 && duration_secs.is_finite() {
        progress
            .duration_ms
            .store((duration_secs * 1000.0) as u64, Ordering::Release);
    } else {
        progress.indeterminate.store(true, Ordering::Release);
    }

    // ── audio stream 選択 ──
    let audio_stream = input
        .streams()
        .best(MediaType::Audio)
        .ok_or(NormalizeScanError::NoAudio)?;
    let stream_idx = audio_stream.index();
    let stream_tb = audio_stream.time_base();
    let params = audio_stream.parameters();
    let ctx = ffmpeg::codec::context::Context::from_parameters(params)
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("codec context: {e}")))?;
    let mut decoder = ctx
        .decoder()
        .audio()
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("audio decoder: {e}")))?;

    let in_fmt = decoder.format();
    let in_rate = decoder.rate();
    let in_layout = decoder.ch_layout();
    let layout_desc = in_layout.description();
    let in_fmt_name = sample_fmt_name(in_fmt);

    // ── filter graph ──
    let mut graph = ffmpeg::filter::Graph::new();
    let abuffer = ffmpeg::filter::find("abuffer")
        .ok_or_else(|| NormalizeScanError::Ffmpeg("filter 'abuffer' not found".to_string()))?;
    let abuffersink = ffmpeg::filter::find("abuffersink")
        .ok_or_else(|| NormalizeScanError::Ffmpeg("filter 'abuffersink' not found".to_string()))?;

    let abuffer_args = format!(
        "time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout={}",
        stream_tb.numerator().max(1),
        stream_tb.denominator().max(1),
        in_rate,
        in_fmt_name,
        layout_desc,
    );
    graph
        .add(&abuffer, "in", &abuffer_args)
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("graph add abuffer: {e}")))?;
    graph
        .add(&abuffersink, "out", "")
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("graph add abuffersink: {e}")))?;

    let chain = format!(
        "aformat=channel_layouts=stereo:sample_fmts=flt:sample_rates={TARGET_RATE},ebur128=metadata=1:peak=true"
    );
    graph
        .output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(&chain))
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("graph parse: {e}")))?;
    graph
        .validate()
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("graph validate: {e}")))?;

    // ── decode loop ──
    let mut last_integrated_lufs: f32 = f32::NEG_INFINITY;
    let mut last_true_peak_linear: f32 = 0.0;
    let mut max_momentary_lufs: f32 = f32::NEG_INFINITY;
    let mut emitted_frames: u64 = 0;

    // packet を 1 つずつ処理しながら cancel チェック
    let packet_iter = input.packets();
    for pkt_result in packet_iter {
        if cancel.load(Ordering::Acquire) {
            return Err(NormalizeScanError::Cancelled);
        }
        let (stream, packet) = match pkt_result {
            Ok(p) => p,
            Err(e) => {
                crate::logger::log(format!("normalize_scanner packet error (continuing): {e}"));
                continue;
            }
        };
        if stream.index() != stream_idx {
            continue;
        }
        // 進捗更新
        if let Some(pts) = packet.pts() {
            let pts_secs =
                pts as f64 * stream_tb.numerator() as f64 / stream_tb.denominator() as f64;
            if pts_secs.is_finite() && pts_secs >= 0.0 {
                progress
                    .pts_processed_ms
                    .store((pts_secs * 1000.0) as u64, Ordering::Release);
            }
        }
        if let Err(e) = decoder.send_packet(&packet) {
            crate::logger::log(format!("normalize_scanner send_packet: {e}"));
            continue;
        }
        let mut frame = Audio::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            if cancel.load(Ordering::Acquire) {
                return Err(NormalizeScanError::Cancelled);
            }
            push_frame_to_graph(&mut graph, &frame)?;
            pull_frames_and_update_metadata(
                &mut graph,
                &mut last_integrated_lufs,
                &mut last_true_peak_linear,
                &mut max_momentary_lufs,
                &mut emitted_frames,
            )?;
        }
    }

    // EOF drain: decoder に NULL packet を送る
    {
        use ffmpeg::ffi::avcodec_send_packet;
        unsafe {
            let _ = avcodec_send_packet(decoder.as_mut_ptr(), std::ptr::null());
        }
    }
    let mut frame = Audio::empty();
    while decoder.receive_frame(&mut frame).is_ok() {
        if cancel.load(Ordering::Acquire) {
            return Err(NormalizeScanError::Cancelled);
        }
        push_frame_to_graph(&mut graph, &frame)?;
        pull_frames_and_update_metadata(
            &mut graph,
            &mut last_integrated_lufs,
            &mut last_true_peak_linear,
            &mut max_momentary_lufs,
            &mut emitted_frames,
        )?;
    }

    // filter graph EOF: source に NULL を流して下流に EOF 伝播 → 最終 metadata frame を pull
    unsafe {
        use ffmpeg::ffi::av_buffersrc_add_frame;
        let mut src = graph
            .get("in")
            .ok_or_else(|| NormalizeScanError::Ffmpeg("graph 'in' missing".to_string()))?;
        let _ = av_buffersrc_add_frame(src.as_mut_ptr(), std::ptr::null_mut());
    }
    pull_frames_and_update_metadata(
        &mut graph,
        &mut last_integrated_lufs,
        &mut last_true_peak_linear,
        &mut max_momentary_lufs,
        &mut emitted_frames,
    )?;

    // ── 結果計算 ──
    // 短尺動画 (< 30s) や integrated が無効なら momentary 最大値を使う。
    let integrated_lufs = if !last_integrated_lufs.is_finite()
        || (duration_secs > 0.0 && duration_secs < MIN_RELIABLE_DURATION_SECS)
    {
        if max_momentary_lufs.is_finite() {
            max_momentary_lufs
        } else {
            return Err(NormalizeScanError::SilentInput);
        }
    } else {
        last_integrated_lufs
    };

    if !integrated_lufs.is_finite() {
        return Err(NormalizeScanError::SilentInput);
    }

    // true peak (linear → dBTP)。0 / -inf の保護。
    let true_peak_db = if last_true_peak_linear > 0.0 && last_true_peak_linear.is_finite() {
        20.0 * last_true_peak_linear.log10()
    } else {
        // 完全無音 / ピーク不明 → -120 dBTP で代用 (= true_peak headroom 計算で問題なし)
        -120.0
    };

    let gain_db_raw = target_lufs - integrated_lufs;
    let true_peak_after_gain = true_peak_db + gain_db_raw;
    let true_peak_headroom = -1.0 - true_peak_after_gain; // 負なら超過
    let gain_db = if true_peak_headroom < 0.0 {
        gain_db_raw + true_peak_headroom
    } else {
        gain_db_raw
    };
    let gain_db = if gain_db.is_finite() {
        gain_db.clamp(-24.0, 24.0)
    } else {
        return Err(NormalizeScanError::InvalidLoudness);
    };

    Ok(NormalizeResult {
        gain_db,
        integrated_lufs,
        true_peak_db,
        target_lufs_milli,
    })
}

fn push_frame_to_graph(
    graph: &mut ffmpeg::filter::Graph,
    frame: &Audio,
) -> Result<(), NormalizeScanError> {
    let mut src = graph
        .get("in")
        .ok_or_else(|| NormalizeScanError::Ffmpeg("graph 'in' missing".to_string()))?;
    src.source()
        .add(frame)
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("graph source.add: {e}")))
}

fn pull_frames_and_update_metadata(
    graph: &mut ffmpeg::filter::Graph,
    last_integrated_lufs: &mut f32,
    last_true_peak_linear: &mut f32,
    max_momentary_lufs: &mut f32,
    emitted_frames: &mut u64,
) -> Result<(), NormalizeScanError> {
    loop {
        let mut out = Audio::empty();
        let mut sink = graph
            .get("out")
            .ok_or_else(|| NormalizeScanError::Ffmpeg("graph 'out' missing".to_string()))?;
        match sink.sink().frame(&mut out) {
            Ok(()) => {
                *emitted_frames += 1;
                let md = out.metadata();
                if let Some(s) = md.get("lavfi.r128.I") {
                    if let Ok(v) = s.parse::<f32>() {
                        *last_integrated_lufs = v;
                    }
                }
                if let Some(s) = md.get("lavfi.r128.true_peak") {
                    if let Ok(v) = s.parse::<f32>() {
                        *last_true_peak_linear = v;
                    }
                }
                if let Some(s) = md.get("lavfi.r128.M") {
                    if let Ok(v) = s.parse::<f32>() {
                        if v.is_finite() && v > *max_momentary_lufs {
                            *max_momentary_lufs = v;
                        }
                    }
                }
            }
            Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN_ERRNO => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(e) => {
                return Err(NormalizeScanError::Ffmpeg(format!("sink frame: {e}")));
            }
        }
    }
    Ok(())
}

fn sample_fmt_name(fmt: Sample) -> &'static str {
    match fmt {
        Sample::None => "none",
        Sample::U8(SampleType::Packed) => "u8",
        Sample::U8(SampleType::Planar) => "u8p",
        Sample::I16(SampleType::Packed) => "s16",
        Sample::I16(SampleType::Planar) => "s16p",
        Sample::I32(SampleType::Packed) => "s32",
        Sample::I32(SampleType::Planar) => "s32p",
        Sample::I64(SampleType::Packed) => "s64",
        Sample::I64(SampleType::Planar) => "s64p",
        Sample::F32(SampleType::Packed) => "flt",
        Sample::F32(SampleType::Planar) => "fltp",
        Sample::F64(SampleType::Packed) => "dbl",
        Sample::F64(SampleType::Planar) => "dblp",
    }
}

/// `AVFormatContext::duration` を秒に。`AV_NOPTS_VALUE` (= i64::MIN) や 0 / 負値は 0.0。
fn duration_to_secs(duration: i64) -> f64 {
    if duration == i64::MIN || duration <= 0 {
        return 0.0;
    }
    // ffmpeg の duration は AV_TIME_BASE (1_000_000) 単位。
    duration as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_to_secs_handles_no_pts() {
        assert_eq!(duration_to_secs(i64::MIN), 0.0);
        assert_eq!(duration_to_secs(0), 0.0);
        assert_eq!(duration_to_secs(-1), 0.0);
        assert_eq!(duration_to_secs(1_500_000), 1.5);
        assert_eq!(duration_to_secs(60_000_000), 60.0);
    }

    #[test]
    fn no_audio_path_returns_no_audio_error() {
        // 存在しないパスは format::input で失敗するので NoAudio までは行かないが、
        // SilentInput / Ffmpeg 系のエラーが返ることを確認 (= panic しない)。
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(NormalizeScanProgress::default());
        let result = scan_audio_loudness(
            Path::new("C:/this/path/should/not/exist.mp4"),
            -14000,
            cancel,
            progress,
        );
        assert!(result.is_err(), "expected Err, got {:?}", result.ok());
    }
}
