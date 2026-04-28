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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Sender, bounded};

use super::clock::AvClock;

/// 1 動画フレーム (BGRA、tightly packed)。
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// BGRA8 ピクセル列 (width * height * 4 バイト)。
    pub bgra: Vec<u8>,
    /// 提示時刻 (秒)。AvClock との比較に使う。
    pub pts_secs: f64,
    /// シーク世代。これが現行の AvClock seek_serial と異なれば UI は捨てる。
    pub seek_serial: u64,
}

/// 1 音声フレーム (interleaved stereo f32、48kHz)。
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub pts_secs: f64,
    pub seek_serial: u64,
}

/// デコード開始時に分かる動画情報。UI の HUD で利用。
#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    pub has_audio: bool,
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
pub fn spawn(
    path: PathBuf,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    target_audio_sample_rate: u32,
) -> DecodeHandles {
    let (video_tx, video_rx) = bounded::<VideoFrame>(4);
    let (audio_tx, audio_rx) = bounded::<AudioFrame>(32);
    let (info_tx, info_rx) = bounded::<Result<VideoInfo, String>>(1);

    std::thread::Builder::new()
        .name("video-decode".into())
        .spawn(move || {
            run_decoder(
                path,
                clock,
                cancel,
                target_audio_sample_rate,
                video_tx,
                audio_tx,
                info_tx,
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
    video_tx: Sender<VideoFrame>,
    audio_tx: Sender<AudioFrame>,
    info_tx: Sender<Result<VideoInfo, String>>,
) {
    use ffmpeg_the_third as ffmpeg;
    use ffmpeg::format::Pixel;
    use ffmpeg::format::sample::{Sample, Type as SampleType};
    use ffmpeg::media::Type as MediaType;
    use ffmpeg::software::resampling::Context as ResampleContext;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::{audio::Audio, video::Video};

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

    let video_decoder_ctx = match ffmpeg::codec::context::Context::from_parameters(video_params) {
        Ok(c) => c,
        Err(e) => {
            let _ = info_tx.send(Err(format!("video codec context: {e}")));
            return;
        }
    };
    let mut video_decoder = match video_decoder_ctx.decoder().video() {
        Ok(d) => d,
        Err(e) => {
            let _ = info_tx.send(Err(format!("video decoder open: {e}")));
            return;
        }
    };
    let src_w = video_decoder.width();
    let src_h = video_decoder.height();
    let src_fmt = video_decoder.format();

    // 出力サイズは GPU テクスチャ上限に合わせて縮める
    let max_dim = crate::app::MAX_TEXTURE_DIM as u32;
    let (dst_w, dst_h) = clamp_dims(src_w, src_h, max_dim);

    // BGRA で受け取り (egui::ColorImage は RGBA だが BGR↔RGB は UI 側でも吸収できる)。
    // ここでは安定優先で RGBA に変換する。
    let mut scaler = match ScaleContext::get(
        src_fmt,
        src_w,
        src_h,
        Pixel::RGBA,
        dst_w,
        dst_h,
        ScaleFlags::BILINEAR,
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = info_tx.send(Err(format!("sws_scale init: {e}")));
            return;
        }
    };

    // ── 音声ストリーム選択 (任意) ──
    let audio_setup = match input.streams().best(MediaType::Audio) {
        Some(audio_stream) => {
            let idx = audio_stream.index();
            let tb = audio_stream.time_base();
            let params = audio_stream.parameters();
            match ffmpeg::codec::context::Context::from_parameters(params) {
                Ok(ctx) => match ctx.decoder().audio() {
                    Ok(dec) => {
                        let in_fmt = dec.format();
                        let in_rate = dec.rate();
                        // FFmpeg 7.x API: channel_layout → ch_layout, get → get2
                        let in_layout = dec.ch_layout();
                        // 出力は f32 packed stereo / target_audio_sample_rate
                        let out_fmt = Sample::F32(SampleType::Packed);
                        let out_rate = target_audio_sample_rate;
                        let out_layout = ffmpeg::ChannelLayout::STEREO;
                        match ResampleContext::get2(
                            in_fmt, in_layout, in_rate, out_fmt, out_layout, out_rate,
                        ) {
                            Ok(rs) => Some(AudioSetup {
                                stream_idx: idx,
                                out_rate,
                                time_base_num: tb.numerator() as f64,
                                time_base_den: tb.denominator() as f64,
                                decoder: dec,
                                resampler: rs,
                                codec_name: "audio".to_string(),
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
                },
                Err(e) => {
                    crate::logger::log(format!("audio codec context failed: {e}"));
                    None
                }
            }
        }
        None => None,
    };

    let has_audio = audio_setup.is_some();

    // ── 動画情報を通知 ──
    let duration_secs = duration_to_secs(input.duration());
    let info = VideoInfo {
        width: src_w,
        height: src_h,
        duration_secs,
        video_codec: video_decoder
            .codec()
            .map(|c| c.name().to_string())
            .unwrap_or_else(|| "?".to_string()),
        audio_codec: audio_setup.as_ref().map(|a| a.codec_name.clone()),
        has_audio,
    };
    let _ = info_tx.send(Ok(info));

    // ── デコードループ ──
    let video_tb_num = video_time_base.numerator() as f64;
    let video_tb_den = video_time_base.denominator() as f64;
    let mut audio_setup = audio_setup;
    let mut current_seek_serial: u64 = 0;
    // post-seek preroll: avformat_seek_file は target ぴったりではなく
    // **直前の keyframe** に戻ることが多い。そのまま再生すると seek 直前の
    // PTS (例: 10s) のフレームが届き、それで AvClock が更新されてシークバーが
    // 戻って見える。target_secs 未満のフレームは renderer/audio に届く前に
    // ここで捨てる (動画) / 部分 trim する (音声)。
    let mut drop_before_secs: Option<f64> = None;

    'outer: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }

        // シーク要求を確認
        if let Some(req) = clock.take_seek_request() {
            let super::clock::SeekRequest {
                target_secs,
                direction,
                serial,
            } = req;
            current_seek_serial = serial;
            drop_before_secs = Some(target_secs);
            // タイムスタンプは AV_TIME_BASE_Q (1/1_000_000 秒、マイクロ秒) 単位。
            let target_pts = (target_secs * 1_000_000.0) as i64;

            // 後方 / 絶対シークは `av_seek_frame + AVSEEK_FLAG_BACKWARD` を使う。
            // `avformat_seek_file` (= `Input::seek`) は AVSEEK_FLAG_BACKWARD を
            // 無視するため、デマクサが target を跨いだ前後どちらの keyframe を
            // 選ぶか不定。raw FFI 経由で確実に target 以前へ飛ばす。
            let backward = |input: &mut ffmpeg::format::context::Input| -> Result<(), ffmpeg::Error> {
                use ffmpeg_the_third::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
                let ret = unsafe {
                    av_seek_frame(input.as_mut_ptr(), -1, target_pts, AVSEEK_FLAG_BACKWARD as i32)
                };
                if ret >= 0 { Ok(()) } else { Err(ffmpeg::Error::from(ret)) }
            };

            let mut seek_result = if direction > 0 {
                input.seek(target_pts, target_pts..)
            } else {
                backward(&mut input)
            };
            // 前方で keyframe が target 以降に無い (≒ EOF 直前) → backward retry
            if seek_result.is_err() && direction > 0 {
                crate::logger::log(format!(
                    "forward seek failed at {target_secs:.3}s, retry as backward"
                ));
                seek_result = backward(&mut input);
            }
            crate::logger::log(format!(
                "seek: target={target_secs:.3}s dir={direction} serial={serial} result={seek_result:?}"
            ));
            if seek_result.is_err() {
                // シーク失敗時に drop_before_secs を残すと以降のフレームが全 drop
                // されて override が永久に残る。素通しさせ override は他経路の
                // clear に委ねる。
                drop_before_secs = None;
            }
            video_decoder.flush();
            if let Some(ref mut a) = audio_setup {
                a.decoder.flush();
            }
            clock.notify_seek_completed(target_secs);
        }

        // 1 パケット読み込み
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
                if let Err(e) = video_decoder.send_packet(&packet) {
                    crate::logger::log(format!("video send_packet: {e}"));
                    continue;
                }
                let mut frame = Video::empty();
                while video_decoder.receive_frame(&mut frame).is_ok() {
                    if cancel.load(Ordering::Acquire) {
                        break 'outer;
                    }
                    let pts = frame.pts().unwrap_or(0);
                    let pts_secs = (pts as f64) * video_tb_num / video_tb_den;
                    // post-seek preroll: target 前のフレームは描画しない
                    if let Some(min) = drop_before_secs {
                        if pts_secs + 0.005 < min {
                            continue;
                        } else {
                            // target に到達した → preroll guard 解除 (動画側のみ)
                            // 音声側はまだ trim 必要なので drop_before_secs はそのまま残す
                        }
                    }
                    let mut rgba = Video::empty();
                    if let Err(e) = scaler.run(&frame, &mut rgba) {
                        crate::logger::log(format!("sws_scale: {e}"));
                        continue;
                    }
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
                        bgra,
                        pts_secs,
                        seek_serial: current_seek_serial,
                    };
                    // **動画フレームは try_send (非ブロッキング)**。bounded(4) が満杯なら
                    // フレームを drop して音声経路を生かす (Codex 指摘)。これにより
                    // 動画 demux/decode が UI の消費に詰まったときも音声 demux/decode が
                    // 止まらず、音声 ringbuf の underrun = ブチブチノイズを防ぐ。
                    use crossbeam_channel::TrySendError;
                    match video_tx.try_send(frame_out) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            // UI 側が遅れている。動画フレームを 1 枚捨てて音声を生かす。
                        }
                        Err(TrySendError::Disconnected(_)) => break 'outer,
                    }
                }
                break; // 1 パケット消費したらループ先頭でシークチェック
            } else if let Some(ref mut a) = audio_setup {
                if stream.index() == a.stream_idx {
                    if let Err(e) = a.decoder.send_packet(&packet) {
                        crate::logger::log(format!("audio send_packet: {e}"));
                        continue;
                    }
                    let mut frame = Audio::empty();
                    while a.decoder.receive_frame(&mut frame).is_ok() {
                        if cancel.load(Ordering::Acquire) {
                            break 'outer;
                        }
                        let pts = frame.pts().unwrap_or(0);
                        let mut pts_secs = (pts as f64) * a.time_base_num / a.time_base_den;
                        let mut resampled = Audio::empty();
                        if let Err(e) = a.resampler.run(&frame, &mut resampled) {
                            crate::logger::log(format!("swr resample: {e}"));
                            continue;
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
                        const CHANNELS: usize = 2;
                        let nb_samples = resampled.samples();
                        if nb_samples == 0 {
                            // 解像度の都合等で 0 サンプルが返ることがある (resample のラグ)。
                            // raw pointer dereference を避けて早期 continue。
                            continue;
                        }
                        // ランタイム不変条件チェック (Codex P3): resampler の出力が
                        // 期待通り f32 packed であること。デバッグ時に format/layout
                        // を取り違えていれば即座に panic で気付ける。
                        debug_assert_eq!(resampled.format(), Sample::F32(SampleType::Packed));
                        debug_assert!(resampled.is_packed());
                        let element_count = nb_samples * CHANNELS;
                        let samples: Vec<f32> = unsafe {
                            let raw_ptr = (*resampled.as_ptr()).data[0] as *const f32;
                            debug_assert!(!raw_ptr.is_null());
                            std::slice::from_raw_parts(raw_ptr, element_count).to_vec()
                        };
                        let mut samples = samples;

                        // post-seek preroll の trim:
                        // avformat_seek は keyframe に戻るので、target_secs 未満の
                        // 音声フレームが届く。完全に target 前ならフレーム破棄、
                        // 跨ぐなら先頭 N サンプルを drain して target ぴったりから始める。
                        if let Some(min) = drop_before_secs {
                            const CHANNELS: usize = 2;
                            let frame_secs = (samples.len() / CHANNELS) as f64
                                / a.out_rate as f64;
                            if pts_secs + frame_secs <= min {
                                // 完全に target 前 → 捨てる
                                continue;
                            }
                            if pts_secs < min {
                                let skip_pairs = ((min - pts_secs) * a.out_rate as f64)
                                    .ceil() as usize;
                                let skip = (skip_pairs * CHANNELS).min(samples.len());
                                samples.drain(..skip);
                                pts_secs = min;
                                if samples.is_empty() {
                                    continue;
                                }
                            } else {
                                // target に到達 → preroll guard 解除
                                drop_before_secs = None;
                            }
                        }

                        let frame_out = AudioFrame {
                            samples,
                            pts_secs,
                            seek_serial: current_seek_serial,
                        };
                        if audio_tx.send(frame_out).is_err() {
                            break 'outer;
                        }
                    }
                    break;
                }
            }
        }
        if !got_packet {
            // EOF
            break;
        }
    }

    crate::logger::log(format!("video decoder finished: {}", path.display()));
    // 関数スコープを抜ける際に video_tx / audio_tx が drop され、UI 側に EOF が伝わる。
}

struct AudioSetup {
    stream_idx: usize,
    out_rate: u32,
    time_base_num: f64,
    time_base_den: f64,
    decoder: ffmpeg_the_third::decoder::Audio,
    resampler: ffmpeg_the_third::software::resampling::Context,
    codec_name: String,
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
