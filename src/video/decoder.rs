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
    /// このフレーム分の再生時間 (秒)。total audio buffer の差分加算に使う。
    pub duration_secs: f64,
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
///
/// `hw_decode` が true なら D3D11VA HW デコードを試行する。コーデック非対応 / デバイス
/// 初期化失敗 / get_format で SW 形式が返った場合は **黙って SW にフォールバック**
/// する (perf log の `video.open.decode_path` で実際の経路を確認可能)。
pub fn spawn(
    path: PathBuf,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    target_audio_sample_rate: u32,
    hw_decode: bool,
) -> DecodeHandles {
    // 60fps 1080p で 8 フレーム = 約 130ms のバッファ。decoder pacing の閾値
    // (100ms) と組み合わせて「pacing 直前に 1-2 フレーム余裕がある」状態を
    // 維持し、vsync 1 周期で取り損ねた分を次周期に displayable な状態で
    // 取れるようにする。bounded(4) では 60fps で常に Full → drop に陥る。
    let (video_tx, video_rx) = bounded::<VideoFrame>(8);
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
                hw_decode,
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
    hw_decode_requested: bool,
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
    let video_avg_fps = {
        let r = video_stream.avg_frame_rate();
        let d = r.denominator();
        if d == 0 { 0.0 } else { r.numerator() as f64 / d as f64 }
    };

    let mut video_decoder_ctx = match ffmpeg::codec::context::Context::from_parameters(video_params) {
        Ok(c) => c,
        Err(e) => {
            let _ = info_tx.send(Err(format!("video codec context: {e}")));
            return;
        }
    };

    // ── HW デコード初期化 (D3D11VA) ──
    // 失敗時は黙って SW デコードに落ちる。`hw_device` を _hw_device で持って Drop 時に
    // unref されるようにし、AVCodecContext は内部でさらに ref を取るので競合しない。
    let codec_id = video_decoder_ctx.id();
    let hw_setup_result: Option<HwDevice> = if hw_decode_requested {
        try_init_d3d11va(codec_id, &mut video_decoder_ctx)
    } else {
        None
    };
    let hw_active_initially = hw_setup_result.is_some();
    // hw_device 自身は AVCodecContext が内部で av_buffer_ref しているので、ここで保持
    // し続ける必要は厳密には無い。ただし decoder open 失敗時 (= 我々が unref しない
    // と即時リーク) を考えてスレッド終了まで保持する。
    let _hw_device = hw_setup_result;

    let mut video_decoder = match video_decoder_ctx.decoder().video() {
        Ok(d) => d,
        Err(e) => {
            // HW 有効で open 失敗 → SW で再試行
            if hw_active_initially {
                crate::logger::log(format!(
                    "HW decoder open failed ({e}), retrying with SW"
                ));
                let retry_ctx = match ffmpeg::codec::context::Context::from_parameters(
                    input.streams().best(MediaType::Video).unwrap().parameters(),
                ) {
                    Ok(c) => c,
                    Err(e2) => {
                        let _ = info_tx.send(Err(format!("video codec context (retry): {e2}")));
                        return;
                    }
                };
                drop(_hw_device);
                match retry_ctx.decoder().video() {
                    Ok(d) => d,
                    Err(e2) => {
                        let _ = info_tx.send(Err(format!("video decoder open (SW retry): {e2}")));
                        return;
                    }
                }
            } else {
                let _ = info_tx.send(Err(format!("video decoder open: {e}")));
                return;
            }
        }
    };
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
    let mut scaler: Option<ScaleContext> = None;
    let mut scaler_key: Option<(Pixel, u32, u32)> = None;
    // 1 フレーム目で実際の format を perf に出力する (HW 期待で SW にフォールバックした
    // ケースを `decode_path` から判別するため)。
    let mut first_frame_logged = false;

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
    if !has_audio {
        // 音声無し動画: 最初から fallback wall clock を使う
        clock.mark_audio_inactive();
    }

    // ── 動画情報を通知 ──
    let duration_secs = duration_to_secs(input.duration());
    let video_codec_name = video_decoder
        .codec()
        .map(|c| c.name().to_string())
        .unwrap_or_else(|| "?".to_string());
    let info = VideoInfo {
        width: src_w,
        height: src_h,
        duration_secs,
        video_codec: video_codec_name.clone(),
        audio_codec: audio_setup.as_ref().map(|a| a.codec_name.clone()),
        has_audio,
    };
    let _ = info_tx.send(Ok(info));

    // perf: 動画特性を 1 行に記録 (解析時の最初の手がかり)。
    if crate::perf::is_enabled() {
        let pix_fmt = format!("{:?}", video_decoder.format());
        let decode_path = if hw_active_initially { "hw_d3d11va" } else { "sw" };
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file_size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(-1);
        let (audio_codec, audio_rate, audio_ch) = audio_setup
            .as_ref()
            .map(|a| (a.codec_name.clone(), a.out_rate as i64, 2_i64))
            .unwrap_or_else(|| ("none".to_string(), 0, 0));
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
                ("video_codec", serde_json::Value::from(video_codec_name)),
                ("avg_fps", serde_json::Value::from(video_avg_fps)),
                ("duration_secs", serde_json::Value::from(duration_secs)),
                ("audio_codec", serde_json::Value::from(audio_codec)),
                ("audio_rate", serde_json::Value::from(audio_rate)),
                ("audio_channels", serde_json::Value::from(audio_ch)),
            ],
        );
    }

    // ── デコードループ ──
    let video_tb_num = video_time_base.numerator() as f64;
    let video_tb_den = video_time_base.denominator() as f64;
    let mut audio_setup = audio_setup;
    let mut current_seek_serial: u64 = 0;
    // 直前に video_tx へ enqueue 成功したフレームの pts。pts ギャップ計測用。
    let mut last_enqueued_pts: f64 = 0.0;
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
            // notify_seek_completed が pre-seek 音声会計のリセットも担う
            // (post-seek hang 防止のため不可分)。
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

                    // HW デコードの場合、`frame.format()` は `AV_PIX_FMT_D3D11`。
                    // av_hwframe_transfer_data で GPU → CPU メモリに NV12 (10-bit なら P010)
                    // として落とし、その SW フレームを scaler に食わせる。
                    // get_format で SW にフォールバックされていれば format() は SW pix_fmt
                    // を返すのでこの分岐に入らない。
                    let mut sw_owned: Option<Video> = None;
                    let frame_for_scaler: &Video = {
                        let fmt = frame.format();
                        if matches!(fmt, Pixel::D3D11) {
                            let mut sw = Video::empty();
                            unsafe {
                                use ffmpeg_the_third::ffi::av_hwframe_transfer_data;
                                let ret = av_hwframe_transfer_data(
                                    sw.as_mut_ptr(),
                                    frame.as_ptr(),
                                    0,
                                );
                                if ret < 0 {
                                    crate::logger::log(format!(
                                        "av_hwframe_transfer_data failed: {ret}"
                                    ));
                                    continue;
                                }
                            }
                            sw_owned = Some(sw);
                            sw_owned.as_ref().unwrap()
                        } else {
                            &frame
                        }
                    };

                    // scaler の lazy 構築 / 入力 (フォーマット|寸法) 変化時の再構築。
                    let cur_fmt = frame_for_scaler.format();
                    let cur_w = frame_for_scaler.width();
                    let cur_h = frame_for_scaler.height();
                    let cur_key = (cur_fmt, cur_w, cur_h);
                    if !first_frame_logged && crate::perf::is_enabled() {
                        let actual_path = if matches!(frame.format(), Pixel::D3D11) {
                            "hw_d3d11va"
                        } else if hw_active_initially {
                            "sw_fallback_after_hw_init"
                        } else {
                            "sw"
                        };
                        crate::perf::event(
                            "video",
                            "first_frame",
                            None,
                            0,
                            &[
                                ("decode_path", serde_json::Value::from(actual_path)),
                                ("frame_pix_fmt", serde_json::Value::from(format!("{cur_fmt:?}"))),
                                ("frame_w", serde_json::Value::from(cur_w as i64)),
                                ("frame_h", serde_json::Value::from(cur_h as i64)),
                            ],
                        );
                        first_frame_logged = true;
                    }
                    if scaler.is_none() || scaler_key != Some(cur_key) {
                        match ScaleContext::get(
                            cur_fmt, cur_w, cur_h, Pixel::RGBA, dst_w, dst_h, ScaleFlags::BILINEAR,
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
                        bgra,
                        pts_secs,
                        seek_serial: current_seek_serial,
                    };
                    // **動画フレームは try_send (非ブロッキング)**。bounded(4) が満杯なら
                    // フレームを drop して音声経路を生かす (Codex 指摘)。これにより
                    // 動画 demux/decode が UI の消費に詰まったときも音声 demux/decode が
                    // 止まらず、音声 ringbuf の underrun = ブチブチノイズを防ぐ。
                    let scale_ms = send_t0.elapsed().as_secs_f64() * 1000.0 - decode_ms;

                    // ── デコーダのペーシング ──
                    //
                    // pts が `video_pacing_now_secs() + PACE_LEAD` 以上先行している間 sleep。
                    // pacing 無しだと try_send Full でフレーム連続 drop → channel 内 pts が
                    // 不連続になり UI 側で「数百 ms 凍結 → ジャンプ」のカクツキを生む。
                    //
                    // audio safety: audio_active かつ総音声バッファが AUDIO_SAFE_LO 以下なら
                    // pacing skip して audio packet 読み込みを優先 (sleep で audio 枯渇させない)。
                    const PACE_LEAD_SECS: f64 = 0.10;
                    const AUDIO_SAFE_LO: f64 = 0.25;
                    while !cancel.load(Ordering::Acquire) && clock.is_playing() {
                        // **post-seek デッドロック対策**:
                        // override が立っている間、pace_now は target で凍結する。
                        // forward seek で post-seek 第一フレームが GOP 先 (例 4K HEVC で
                        // 2.8 秒先) にあると、ahead > PACE_LEAD で永久に sleep する。
                        // しかし UI tick はこのフレームを受け取って override を clear
                        // しないと clock を進められない → デッドロック。
                        // override 中は **pacing をスキップして即 send** する。
                        // UI が表示し override を clear した直後から通常 pacing に戻る
                        // (Codex P1 助言)。
                        if clock.is_seeking() {
                            break;
                        }
                        let ahead = pts_secs - clock.video_pacing_now_secs();
                        if ahead <= PACE_LEAD_SECS {
                            break;
                        }
                        if clock.is_audio_active()
                            && clock.total_audio_buffer_secs() < AUDIO_SAFE_LO
                        {
                            // audio が枯渇しそう → pacing skip して decoder を進める
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }

                    // perf: 直前に enqueue した pts との差を記録 (= channel に
                    // 入っているフレームの pts ギャップが分かる、dropped_full だけでは
                    // 「どれくらい飛んだか」が見えないため)。
                    let pts_gap = pts_secs - last_enqueued_pts;
                    use crossbeam_channel::TrySendError;
                    let send_result = video_tx.try_send(frame_out);
                    let dropped_full = matches!(&send_result, Err(TrySendError::Full(_)));
                    if !dropped_full && send_result.is_ok() {
                        last_enqueued_pts = pts_secs;
                    }
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "video",
                            "decode",
                            None,
                            0,
                            &[
                                ("pts", serde_json::Value::from(pts_secs)),
                                ("decode_ms", serde_json::Value::from(decode_ms.round() / 1.0)),
                                ("scale_ms", serde_json::Value::from(scale_ms.round() / 1.0)),
                                ("dropped_full", serde_json::Value::from(dropped_full)),
                                ("pts_gap_ms", serde_json::Value::from(pts_gap * 1000.0)),
                                ("audio_buf_secs", serde_json::Value::from(clock.total_audio_buffer_secs())),
                                ("pace_now", serde_json::Value::from(clock.video_pacing_now_secs())),
                            ],
                        );
                    }
                    if let Err(TrySendError::Disconnected(_)) = send_result {
                        break 'outer;
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

                        // 1 stereo pair = 2 float (samples_per_sec stereo = a.out_rate * 2)
                        let duration_secs = (samples.len() / 2) as f64 / a.out_rate as f64;
                        let frame_out = AudioFrame {
                            samples,
                            pts_secs,
                            seek_serial: current_seek_serial,
                            duration_secs,
                        };
                        // tx queued 合計を **send 前に加算**。pump.recv 後の減算と
                        // 競合しないよう順序を保つ。失敗時はロールバック (Codex 指摘)。
                        clock.add_audio_tx_queued_secs(duration_secs);
                        if audio_tx.send(frame_out).is_err() {
                            clock.add_audio_tx_queued_secs(-duration_secs);
                            break 'outer;
                        }
                    }
                    break;
                }
            }
        }
        if !got_packet {
            // EOF or demux stall。先に seek 要求をチェック (race で EOF flag 立てる
            // 前に新シークが来ていれば即通常ループに戻る)。
            if clock.peek_seek_request_pending() {
                continue;
            }
            // EOF 確定。スレッドは終わらせず、cancel か新しい seek 要求が来るまで
            // idle ループで待つ。これで末尾停止後の re-seek / replay が
            // decoder 再生成なしで動作する。
            clock.notify_eof_reached();
            loop {
                if cancel.load(Ordering::Acquire) {
                    crate::logger::log(format!(
                        "video decoder finished: {}",
                        path.display()
                    ));
                    return;
                }
                if clock.peek_seek_request_pending() {
                    clock.clear_eof_reached();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
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

/// D3D11VA HW デコードを試みる。サポート確認 → デバイス作成 → AVCodecContext へ装着
/// までを行い、成功時は `Some(HwDevice)` を返す。失敗 (codec 非対応 / device 作成失敗)
/// 時は `None` を返し、SW で続行する。
fn try_init_d3d11va(
    codec_id: ffmpeg_the_third::codec::Id,
    ctx: &mut ffmpeg_the_third::codec::context::Context,
) -> Option<HwDevice> {
    use ffmpeg_the_third::ffi::*;

    unsafe {
        // 1. デコーダコーデックを取得 (avcodec_find_decoder)
        let codec = avcodec_find_decoder(codec_id.into());
        if codec.is_null() {
            crate::logger::log(format!(
                "HW: avcodec_find_decoder({codec_id:?}) returned null"
            ));
            return None;
        }

        // 2. avcodec_get_hw_config で D3D11VA + HW_DEVICE_CTX をサポートするか確認
        let mut supported = false;
        for i in 0_i32.. {
            let cfg = avcodec_get_hw_config(codec, i);
            if cfg.is_null() {
                break;
            }
            let cfg = &*cfg;
            if cfg.device_type == AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
                && (cfg.methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
                && cfg.pix_fmt == AVPixelFormat::AV_PIX_FMT_D3D11
            {
                supported = true;
                break;
            }
        }
        if !supported {
            crate::logger::log(format!(
                "HW: codec {codec_id:?} does not support D3D11VA HW_DEVICE_CTX"
            ));
            return None;
        }

        // 3. HW デバイスコンテキスト作成 (D3D11VA、デフォルトアダプタ)
        let mut buf_ref: *mut AVBufferRef = std::ptr::null_mut();
        let ret = av_hwdevice_ctx_create(
            &mut buf_ref,
            AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        );
        if ret < 0 || buf_ref.is_null() {
            crate::logger::log(format!(
                "HW: av_hwdevice_ctx_create(D3D11VA) failed: {ret}"
            ));
            return None;
        }

        // 4. AVCodecContext にぶら下げる + get_format コールバック
        let avctx = ctx.as_mut_ptr();
        // 既存の hw_device_ctx を上書きする前に av_buffer_unref しておく必要があるが、
        // from_parameters 直後はゼロなのでそのまま代入。
        let new_ref = av_buffer_ref(buf_ref);
        if new_ref.is_null() {
            // av_buffer_ref のメモリ確保失敗 (極めて稀)。元の buf_ref を解放して None。
            let mut to_free = buf_ref;
            av_buffer_unref(&mut to_free);
            crate::logger::log("HW: av_buffer_ref returned null".to_string());
            return None;
        }
        (*avctx).hw_device_ctx = new_ref;
        (*avctx).get_format = Some(get_hw_format);

        crate::logger::log(format!(
            "HW: D3D11VA initialized for codec {codec_id:?}"
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
