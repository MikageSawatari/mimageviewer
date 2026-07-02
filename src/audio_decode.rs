//! 音声ファイルを解析用に「全尺デコード → インターリーブ stereo f32 / 48kHz」する
//! オフラインデコーダ。
//!
//! 再生は本体の `VideoPlayer` (音声のみでも可) を再利用するが、タイムライン解析は
//! 再生ペーシングと独立した専用ワーカーで走らせたい (docs/music-integration-plan.md
//! §5.3 / D9)。ここでは FFmpeg (avformat + avcodec + swresample) で 1 ファイルを丸ごと
//! デコードし、`music_core::analyze_stereo_timeline` が要求する
//! **インターリーブ stereo f32 PCM** を作る。
//!
//! swresample から packed f32 を取り出す手順は動画側 `video/decoder.rs` の実績ある
//! 実装 (linesize パディングを raw pointer で回避する) をそのまま踏襲する。
//!
//! ⚠️ 実際のデコード動作は FFmpeg DLL + 実ファイルが要るため実機検証項目。純ロジックの
//! 解析 (`analyze_stereo_timeline`) と DB は機械なしでテスト済み。

use std::path::Path;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::software::resampling::Context as ResampleContext;
use ffmpeg::util::frame::Audio as AudioFrame;
use ffmpeg_the_third as ffmpeg;
use music_core::{AudioStreamInfo, DecodedAudio};

/// 解析用の出力サンプルレート (Hz)。動画音声パイプラインと揃えて 48kHz stereo にする。
const OUT_RATE: u32 = 48_000;
const OUT_CHANNELS: usize = 2;

static FFMPEG_INIT: Once = Once::new();

fn ensure_ffmpeg_init() {
    FFMPEG_INIT.call_once(|| {
        let _ = ffmpeg::init();
    });
}

/// 音声ファイルをデコードして `music_core` のタイムライン解析結果を返す。
///
/// decode (FFmpeg) → `analyze_stereo_timeline` (純ロジック) の合成。DB キャッシュの
/// 参照・保存は呼び出し側 (Inc 3 の解析ワーカー) の責務にして、この関数はステートレスに
/// 保つ。`cancel` は decode 中と解析直前で確認する。
pub fn analyze_audio_file(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<music_core::TimelineAnalysis, String> {
    analyze_audio_file_with_config(path, cancel, music_core::AnalysisConfig::default())
}

/// `analyze_audio_file` の解析 config を明示する版。音楽ビュー (Inc 3) はラボと同じ
/// 細かい bin (`bin_secs = 0.010`) でタイムラインを描くため、default より高解像度の
/// config を渡す。decode → `analyze_stereo_timeline` の合成は共通。
pub fn analyze_audio_file_with_config(
    path: &Path,
    cancel: &AtomicBool,
    config: music_core::AnalysisConfig,
) -> Result<music_core::TimelineAnalysis, String> {
    let decoded = decode_audio_file_to_stereo_f32(path, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }
    Ok(music_core::analyze_stereo_timeline(
        &decoded.stereo_samples,
        decoded.info.sample_rate,
        config,
    ))
}

/// 音声ファイルを全尺デコードして 48kHz interleaved stereo f32 PCM を返す。
///
/// `cancel` が立ったら途中で `Err` を返して打ち切る (呼び出し側ワーカーが結果を破棄する)。
pub fn decode_audio_file_to_stereo_f32(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<DecodedAudio, String> {
    ensure_ffmpeg_init();

    let pb = path.to_path_buf();
    let mut ictx = ffmpeg::format::input(&pb).map_err(|e| format!("format::input: {e}"))?;

    // 最良の音声ストリームを選ぶ。`stream.parameters()` は stream を借用するので、
    // codec context の構築まで stream スコープ内で済ませてから owned な context を取り出す
    // (この後 ictx.packets() が ictx を可変借用するため)。
    let (stream_index, codec_ctx) = {
        let stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| "音声ストリームが見つかりません".to_string())?;
        let idx = stream.index();
        let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| format!("codec context: {e}"))?;
        (idx, ctx)
    };
    let mut decoder = codec_ctx
        .decoder()
        .audio()
        .map_err(|e| format!("audio decoder open: {e}"))?;

    let in_fmt = decoder.format();
    let in_rate = decoder.rate();
    // レイアウト未指定 (AV_CHANNEL_ORDER_UNSPEC、古い WMA 等) だと swresample::get2 が
    // 内部の mask().unwrap() で panic するので、チャンネル数から既定レイアウトへ差し替える
    // (video/decoder.rs の normalize_audio_input_layout と同じ手順)。
    let in_layout = normalize_layout(decoder.ch_layout());

    let mut resampler = ResampleContext::get2(
        in_fmt,
        in_layout,
        in_rate,
        Sample::F32(SampleType::Packed),
        ffmpeg::ChannelLayout::STEREO,
        OUT_RATE,
    )
    .map_err(|e| format!("swresample init: {e}"))?;

    let mut out: Vec<f32> = Vec::new();
    let mut frame = AudioFrame::empty();

    for res in ictx.packets() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let (stream, packet) = res.map_err(|e| format!("demux: {e}"))?;
        if stream.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            // 1 packet の decode 失敗は致命的でない (壊れフレーム等) ので継続。
            continue;
        }
        drain_decoder(&mut decoder, &mut frame, &mut resampler, &mut out, in_rate)?;
    }

    // EOF: decoder に溜まった残りフレームを drain する。
    let _ = decoder.send_eof();
    drain_decoder(&mut decoder, &mut frame, &mut resampler, &mut out, in_rate)?;

    let frame_count = out.len() / OUT_CHANNELS;
    let duration_secs = frame_count as f64 / OUT_RATE as f64;

    Ok(DecodedAudio {
        info: AudioStreamInfo {
            sample_rate: OUT_RATE,
            channels: OUT_CHANNELS as u16,
            duration_secs,
        },
        stereo_samples: out,
    })
}

/// 入力レイアウトを swresample が扱える (mask を持つ native order の) レイアウトへ
/// 正規化する。mask を持たない (UNSPEC) 入力はチャンネル数から既定レイアウトへ差し替える。
fn normalize_layout(layout: ffmpeg::ChannelLayout<'_>) -> ffmpeg::ChannelLayout<'static> {
    if layout.mask().is_some() {
        return ffmpeg::ChannelLayout::from(layout.into_owned());
    }
    let channels = layout.channels();
    let substitute = ffmpeg::ChannelLayout::default_for_channels(channels);
    if substitute.mask().is_some() {
        return substitute;
    }
    if channels >= 2 {
        ffmpeg::ChannelLayout::STEREO
    } else {
        ffmpeg::ChannelLayout::MONO
    }
}

/// decoder に溜まったフレームを `receive_frame` で全て取り出し、resample して `out` へ。
fn drain_decoder(
    decoder: &mut ffmpeg::decoder::Audio,
    frame: &mut AudioFrame,
    resampler: &mut ResampleContext,
    out: &mut Vec<f32>,
    in_rate: u32,
) -> Result<(), String> {
    while decoder.receive_frame(frame).is_ok() {
        append_resampled(resampler, frame, out, in_rate)?;
    }
    Ok(())
}

/// 1 音声フレームを 48kHz stereo f32 packed に resample して `out` に追記する。
///
/// swresample の内部バッファリング (位相補間の遅延) は解析用途では無視できる (曲全体で
/// bin 化するので末尾数十サンプルの欠落は影響しない)。よって動画のストリーミング再生と
/// 同じく「入力フレームごとに 1 回 run」する方式にし、最終 flush は行わない。
fn append_resampled(
    resampler: &mut ResampleContext,
    frame: &mut AudioFrame,
    out: &mut Vec<f32>,
    in_rate: u32,
) -> Result<(), String> {
    let in_samples = frame.samples();
    if in_samples == 0 {
        return Ok(());
    }
    // フレーム単位のレイアウト正規化 (Codex P2): resampler は正規化済みレイアウトで
    // 構築されているが、各フレームの ch_layout が UNSPEC のままだと swr_convert_frame が
    // 失敗する (古い WMA 等)。マスクを持たないフレームだけ差し替える
    // (video/decoder.rs:5432-5435 と同じ手順)。
    if frame.ch_layout().mask().is_none() {
        let normalized = normalize_layout(frame.ch_layout());
        frame.set_ch_layout(normalized);
    }
    // 出力バッファは標準 FFmpeg パターン ceil(in * out_rate / in_rate) + swr delay + safety
    // で確保する (Codex P2)。floor + 固定 64 だと downsample や delay > 64 のときに
    // サンプルが swr 内部 delay に取り残されたり run() error になる
    // (video/decoder.rs:5376 resample_output_buffer_samples と同式)。
    const SWR_OUTPUT_SAFETY_SAMPLES: u64 = 32;
    let delay_out = resampler
        .delay()
        .map(|d| d.output.max(0) as u64)
        .unwrap_or(0);
    let rate_converted = (in_samples as u64 * OUT_RATE as u64).div_ceil(in_rate.max(1) as u64);
    let out_cap = (rate_converted + delay_out + SWR_OUTPUT_SAFETY_SAMPLES) as usize;
    let mut resampled = AudioFrame::empty();
    unsafe {
        resampled.alloc(
            Sample::F32(SampleType::Packed),
            out_cap,
            ffmpeg::ChannelLayoutMask::STEREO,
        );
        resampled.set_rate(OUT_RATE);
    }
    if let Err(e) = resampler.run(frame, &mut resampled) {
        // 1 フレームの resample 失敗は致命的でない。
        crate::logger::log(format!("[audio] swr resample: {e}"));
        return Ok(());
    }
    let nb = resampled.samples();
    if nb == 0 {
        return Ok(());
    }
    debug_assert_eq!(resampled.format(), Sample::F32(SampleType::Packed));
    debug_assert!(resampled.is_packed());
    // linesize パディングを避けるため data[0] を直接 f32 スライス化する
    // (video/decoder.rs と同じ手順)。
    let count = nb * OUT_CHANNELS;
    // 長尺 / 巨大ファイルで Vec が GB 級に膨らんでも abort させず、確保失敗は Err で
    // 上位ワーカーへ返す (Codex P2: extend_from_slice の暗黙 abort 回避)。
    if out.try_reserve(count).is_err() {
        return Err(format!(
            "音声デコードのメモリ確保に失敗しました (ファイルが長すぎる可能性: {} samples)",
            out.len() + count
        ));
    }
    unsafe {
        let ptr = (*resampled.as_ptr()).data[0] as *const f32;
        if ptr.is_null() {
            return Ok(());
        }
        out.extend_from_slice(std::slice::from_raw_parts(ptr, count));
    }
    Ok(())
}
