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
/// 長尺動画で仮 gain を返す既定スキャン量。10 分ぶん測れたら再生を始め、最終 scan は継続する。
pub const PROVISIONAL_SCAN_AFTER_SECS: f64 = 10.0 * 60.0;
/// 仮結果として採用する最低 loudness。先頭側が無音に近い動画で +24dB 仮 gain を返さない。
const PROVISIONAL_MIN_VALID_LUFS: f32 = -70.0;
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

struct PreparedNormalizeAudio {
    stream_idx: usize,
    stream_tb: ffmpeg::Rational,
    decoder: ffmpeg::decoder::Audio,
}

/// Select and own the decoder state before excluding every other stream from this scanner's input.
/// The packet loop retains its stream-index check as a defensive boundary.
fn prepare_normalize_audio_stream(
    input: &mut ffmpeg::format::context::Input,
) -> Result<PreparedNormalizeAudio, NormalizeScanError> {
    let (stream_idx, stream_tb, codec_context) = {
        let audio_stream = input
            .streams()
            .best(MediaType::Audio)
            .ok_or(NormalizeScanError::NoAudio)?;
        let stream_idx = audio_stream.index();
        let stream_tb = audio_stream.time_base();
        let codec_context =
            ffmpeg::codec::context::Context::from_parameters(audio_stream.parameters())
                .map_err(|e| NormalizeScanError::Ffmpeg(format!("codec context: {e}")))?;
        (stream_idx, stream_tb, codec_context)
    };
    let decoder = codec_context
        .decoder()
        .audio()
        .map_err(|e| NormalizeScanError::Ffmpeg(format!("audio decoder: {e}")))?;
    crate::audio_decode::discard_unselected_streams(input, stream_idx);
    Ok(PreparedNormalizeAudio {
        stream_idx,
        stream_tb,
        decoder,
    })
}

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
    let mut noop = |_result: NormalizeResult| {};
    scan_audio_loudness_impl(path, target_lufs_milli, cancel, progress, None, &mut noop)
}

/// `provisional_after_secs` ぶん処理できた時点で、算出可能なら仮 `NormalizeResult` を
/// `on_provisional` に一度だけ返し、そのまま最後まで scan を継続する。
///
/// 仮結果は「現在までに観測した loudness / true peak」だけに基づくため DB 保存しない。
/// App 側は再生開始用のセッション内 gain として扱い、最終 `Ok` の結果だけを永続化する。
pub fn scan_audio_loudness_with_provisional(
    path: &Path,
    target_lufs_milli: i32,
    cancel: Arc<AtomicBool>,
    progress: Arc<NormalizeScanProgress>,
    provisional_after_secs: f64,
    on_provisional: &mut dyn FnMut(NormalizeResult),
) -> Result<NormalizeResult, NormalizeScanError> {
    scan_audio_loudness_impl(
        path,
        target_lufs_milli,
        cancel,
        progress,
        Some(provisional_after_secs),
        on_provisional,
    )
}

fn scan_audio_loudness_impl(
    path: &Path,
    target_lufs_milli: i32,
    cancel: Arc<AtomicBool>,
    progress: Arc<NormalizeScanProgress>,
    provisional_after_secs: Option<f64>,
    on_provisional: &mut dyn FnMut(NormalizeResult),
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
    let PreparedNormalizeAudio {
        stream_idx,
        stream_tb,
        mut decoder,
    } = prepare_normalize_audio_stream(&mut input)?;

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
    let mut last_processed_secs = 0.0_f64;
    let mut provisional_emitted = false;
    let provisional_after_secs = provisional_after_secs.filter(|secs| {
        secs.is_finite() && *secs > 0.0 && (duration_secs <= 0.0 || duration_secs > *secs + 1.0)
    });

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
                last_processed_secs = pts_secs;
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
            maybe_emit_provisional(
                provisional_after_secs,
                &mut provisional_emitted,
                last_processed_secs,
                target_lufs_milli,
                target_lufs,
                last_integrated_lufs,
                last_true_peak_linear,
                max_momentary_lufs,
                on_provisional,
            );
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
        maybe_emit_provisional(
            provisional_after_secs,
            &mut provisional_emitted,
            last_processed_secs,
            target_lufs_milli,
            target_lufs,
            last_integrated_lufs,
            last_true_peak_linear,
            max_momentary_lufs,
            on_provisional,
        );
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

    compute_normalize_result(
        target_lufs_milli,
        target_lufs,
        last_integrated_lufs,
        last_true_peak_linear,
        max_momentary_lufs,
        duration_secs,
    )
}

#[allow(clippy::too_many_arguments)]
fn maybe_emit_provisional(
    provisional_after_secs: Option<f64>,
    provisional_emitted: &mut bool,
    processed_secs: f64,
    target_lufs_milli: i32,
    target_lufs: f32,
    last_integrated_lufs: f32,
    last_true_peak_linear: f32,
    max_momentary_lufs: f32,
    on_provisional: &mut dyn FnMut(NormalizeResult),
) {
    let Some(threshold_secs) = provisional_after_secs else {
        return;
    };
    if *provisional_emitted || processed_secs < threshold_secs {
        return;
    }
    let measured_secs = processed_secs.max(threshold_secs);
    match compute_normalize_result(
        target_lufs_milli,
        target_lufs,
        last_integrated_lufs,
        last_true_peak_linear,
        max_momentary_lufs,
        measured_secs,
    ) {
        Ok(result) => {
            if result.integrated_lufs <= PROVISIONAL_MIN_VALID_LUFS
                && max_momentary_lufs <= PROVISIONAL_MIN_VALID_LUFS
            {
                return;
            }
            *provisional_emitted = true;
            on_provisional(result);
        }
        Err(NormalizeScanError::SilentInput) | Err(NormalizeScanError::InvalidLoudness) => {}
        Err(_) => {}
    }
}

fn compute_normalize_result(
    target_lufs_milli: i32,
    target_lufs: f32,
    last_integrated_lufs: f32,
    last_true_peak_linear: f32,
    max_momentary_lufs: f32,
    measured_duration_secs: f64,
) -> Result<NormalizeResult, NormalizeScanError> {
    // 短尺動画 (< 30s) や integrated が無効なら momentary 最大値を使う。
    let integrated_lufs = if !last_integrated_lufs.is_finite()
        || (measured_duration_secs > 0.0 && measured_duration_secs < MIN_RELIABLE_DURATION_SECS)
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
    use std::process::Command;

    fn assert_normalize_result_near(left: NormalizeResult, right: NormalizeResult) {
        assert_eq!(left.target_lufs_milli, right.target_lufs_milli);
        assert!(
            (left.gain_db - right.gain_db).abs() <= 1.0e-4,
            "gain differs: {left:?} vs {right:?}"
        );
        assert!(
            (left.integrated_lufs - right.integrated_lufs).abs() <= 1.0e-4,
            "integrated LUFS differs: {left:?} vs {right:?}"
        );
        assert!(
            (left.true_peak_db - right.true_peak_db).abs() <= 1.0e-4,
            "true peak differs: {left:?} vs {right:?}"
        );
    }

    fn run_ffmpeg(ffmpeg_exe: &std::ffi::OsStr, args: &[&std::ffi::OsStr]) {
        let output = Command::new(ffmpeg_exe)
            .args(args)
            .output()
            .expect("start ffmpeg fixture generator");
        assert!(
            output.status.success(),
            "ffmpeg fixture generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn generated_multistream_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf)
    {
        let temp = tempfile::tempdir().expect("tempdir");
        let multi = temp.path().join("video-two-audio.mkv");
        let selected_audio = temp.path().join("selected-audio.mka");
        let ffmpeg_exe = std::env::var_os("MIV_TEST_FFMPEG")
            .unwrap_or_else(|| std::ffi::OsString::from("ffmpeg.exe"));
        let multi_args = [
            std::ffi::OsStr::new("-hide_banner"),
            std::ffi::OsStr::new("-loglevel"),
            std::ffi::OsStr::new("error"),
            std::ffi::OsStr::new("-y"),
            std::ffi::OsStr::new("-f"),
            std::ffi::OsStr::new("lavfi"),
            std::ffi::OsStr::new("-i"),
            std::ffi::OsStr::new("testsrc2=size=64x64:rate=1:duration=40"),
            std::ffi::OsStr::new("-f"),
            std::ffi::OsStr::new("lavfi"),
            std::ffi::OsStr::new("-i"),
            std::ffi::OsStr::new("sine=frequency=440:sample_rate=48000:duration=40,volume=0.10"),
            std::ffi::OsStr::new("-f"),
            std::ffi::OsStr::new("lavfi"),
            std::ffi::OsStr::new("-i"),
            std::ffi::OsStr::new("sine=frequency=997:sample_rate=48000:duration=40,volume=0.70"),
            std::ffi::OsStr::new("-map"),
            std::ffi::OsStr::new("0:v:0"),
            std::ffi::OsStr::new("-map"),
            std::ffi::OsStr::new("1:a:0"),
            std::ffi::OsStr::new("-map"),
            std::ffi::OsStr::new("2:a:0"),
            std::ffi::OsStr::new("-c:v"),
            std::ffi::OsStr::new("mpeg4"),
            std::ffi::OsStr::new("-q:v"),
            std::ffi::OsStr::new("10"),
            std::ffi::OsStr::new("-c:a"),
            std::ffi::OsStr::new("pcm_s16le"),
            std::ffi::OsStr::new("-disposition:a:0"),
            std::ffi::OsStr::new("0"),
            std::ffi::OsStr::new("-disposition:a:1"),
            std::ffi::OsStr::new("default"),
            multi.as_os_str(),
        ];
        run_ffmpeg(&ffmpeg_exe, &multi_args);
        let selected_args = [
            std::ffi::OsStr::new("-hide_banner"),
            std::ffi::OsStr::new("-loglevel"),
            std::ffi::OsStr::new("error"),
            std::ffi::OsStr::new("-y"),
            std::ffi::OsStr::new("-i"),
            multi.as_os_str(),
            std::ffi::OsStr::new("-map"),
            std::ffi::OsStr::new("0:a:1"),
            std::ffi::OsStr::new("-c:a"),
            std::ffi::OsStr::new("copy"),
            selected_audio.as_os_str(),
        ];
        run_ffmpeg(&ffmpeg_exe, &selected_args);
        (temp, multi, selected_audio)
    }

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

    #[test]
    fn compute_result_limits_gain_by_true_peak() {
        let result =
            compute_normalize_result(-14000, -14.0, -24.0, 1.0, f32::NEG_INFINITY, 600.0).unwrap();

        assert!((result.gain_db - -1.0).abs() < 1.0e-6);
        assert!((result.true_peak_db - 0.0).abs() < 1.0e-6);
    }

    #[test]
    fn compute_result_uses_momentary_for_short_or_invalid_integrated() {
        let result =
            compute_normalize_result(-14000, -14.0, f32::NEG_INFINITY, 0.1, -18.0, 12.0).unwrap();

        assert!((result.integrated_lufs - -18.0).abs() < 1.0e-6);
        assert!((result.gain_db - 4.0).abs() < 1.0e-6);
    }

    #[test]
    fn provisional_emit_waits_for_valid_loudness() {
        let mut emitted = Vec::new();
        let mut invalid_emitted_flag = false;
        maybe_emit_provisional(
            Some(600.0),
            &mut invalid_emitted_flag,
            600.0,
            -14000,
            -14.0,
            f32::NEG_INFINITY,
            0.0,
            f32::NEG_INFINITY,
            &mut |result| emitted.push(result),
        );
        assert!(!invalid_emitted_flag);
        assert!(emitted.is_empty());

        let mut silent_finite_flag = false;
        maybe_emit_provisional(
            Some(600.0),
            &mut silent_finite_flag,
            600.0,
            -14000,
            -14.0,
            -120.0,
            0.0,
            -120.0,
            &mut |result| emitted.push(result),
        );
        assert!(!silent_finite_flag);
        assert!(emitted.is_empty());

        let mut emitted_flag = false;
        maybe_emit_provisional(
            Some(600.0),
            &mut emitted_flag,
            600.0,
            -14000,
            -14.0,
            -20.0,
            0.1,
            -19.0,
            &mut |result| emitted.push(result),
        );
        assert!(emitted_flag);
        assert_eq!(emitted.len(), 1);
    }

    #[test]
    #[ignore = "requires the FFmpeg CLI to generate a multi-stream fixture"]
    fn normalize_discard_preserves_selected_audio_results_and_cancellation() {
        ffmpeg::init().expect("ffmpeg init");
        let (_temp, multi, selected_audio) = generated_multistream_fixture();

        let fresh_input = ffmpeg::format::input(&multi).expect("open fresh fixture input");
        let fresh_discards = fresh_input
            .streams()
            .map(|stream| stream.discard())
            .collect::<Vec<_>>();
        let fresh_selected_idx = fresh_input
            .streams()
            .best(MediaType::Audio)
            .expect("best audio in fresh fixture")
            .index();

        let mut prepared_input =
            ffmpeg::format::input(&multi).expect("open prepared fixture input");
        let prepared = prepare_normalize_audio_stream(&mut prepared_input).expect("prepare audio");
        assert_eq!(prepared.stream_idx, fresh_selected_idx);
        assert_eq!(
            prepared.stream_idx, 2,
            "the second audio stream must be selected"
        );
        for stream in prepared_input.streams() {
            if stream.index() == prepared.stream_idx {
                assert_eq!(stream.discard(), fresh_discards[stream.index()]);
            } else {
                assert_eq!(
                    stream.discard(),
                    ffmpeg::ffi::AVDiscard::AVDISCARD_ALL.into(),
                    "stream {} was not discarded",
                    stream.index()
                );
            }
        }
        assert_eq!(
            fresh_input
                .streams()
                .map(|stream| stream.discard())
                .collect::<Vec<_>>(),
            fresh_discards,
            "preparing a separate input changed the fresh context"
        );

        let full_multi_progress = Arc::new(NormalizeScanProgress::default());
        let full_multi = scan_audio_loudness(
            &multi,
            -14000,
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&full_multi_progress),
        )
        .expect("scan multi-stream fixture");
        let full_selected = scan_audio_loudness(
            &selected_audio,
            -14000,
            Arc::new(AtomicBool::new(false)),
            Arc::new(NormalizeScanProgress::default()),
        )
        .expect("scan selected audio fixture");
        assert_normalize_result_near(full_multi, full_selected);
        assert!(
            full_multi_progress.pts_processed_ms.load(Ordering::Acquire) >= 39_000,
            "full scan did not publish progress near the end of the 40 second fixture"
        );
        assert!(
            full_multi_progress.duration_ms.load(Ordering::Acquire) >= 39_000,
            "fixture duration was not published"
        );

        let mut provisional_multi = Vec::new();
        let provisional_multi_final = scan_audio_loudness_with_provisional(
            &multi,
            -14000,
            Arc::new(AtomicBool::new(false)),
            Arc::new(NormalizeScanProgress::default()),
            0.5,
            &mut |result| provisional_multi.push(result),
        )
        .expect("scan multi-stream fixture with provisional result");
        let mut provisional_selected = Vec::new();
        let provisional_selected_final = scan_audio_loudness_with_provisional(
            &selected_audio,
            -14000,
            Arc::new(AtomicBool::new(false)),
            Arc::new(NormalizeScanProgress::default()),
            0.5,
            &mut |result| provisional_selected.push(result),
        )
        .expect("scan selected audio fixture with provisional result");
        assert_eq!(provisional_multi.len(), 1);
        assert_eq!(provisional_selected.len(), 1);
        assert_normalize_result_near(provisional_multi[0], provisional_selected[0]);
        assert_normalize_result_near(full_multi, provisional_multi_final);
        assert_normalize_result_near(provisional_multi_final, provisional_selected_final);

        let pre_cancel = Arc::new(AtomicBool::new(true));
        let mut pre_cancel_provisional = Vec::new();
        let pre_cancel_result = scan_audio_loudness_with_provisional(
            &multi,
            -14000,
            pre_cancel,
            Arc::new(NormalizeScanProgress::default()),
            0.5,
            &mut |result| pre_cancel_provisional.push(result),
        );
        assert!(matches!(
            pre_cancel_result,
            Err(NormalizeScanError::Cancelled)
        ));
        assert!(pre_cancel_provisional.is_empty());

        let callback_cancel = Arc::new(AtomicBool::new(false));
        let callback_cancel_from_provisional = Arc::clone(&callback_cancel);
        let mut callback_cancel_provisional = Vec::new();
        let callback_cancel_result = scan_audio_loudness_with_provisional(
            &multi,
            -14000,
            callback_cancel,
            Arc::new(NormalizeScanProgress::default()),
            0.5,
            &mut |result| {
                callback_cancel_provisional.push(result);
                callback_cancel_from_provisional.store(true, Ordering::Release);
            },
        );
        assert!(matches!(
            callback_cancel_result,
            Err(NormalizeScanError::Cancelled)
        ));
        assert_eq!(callback_cancel_provisional.len(), 1);
    }
}
