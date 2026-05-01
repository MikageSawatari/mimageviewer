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

use crossbeam_channel::{Receiver, Sender, bounded};

use super::clock::AvClock;

/// Phase B (3-thread split): demux thread から video decode thread に流すメッセージ。
///
/// Phase A で audio decode を分離した後も、demux と video decode は同じスレッドで
/// 動いていた。video decode 中に I/O bound な demux が止まると、(1) 音声 packet も
/// 流れず audio decode 待機が広がる、(2) HDD random read のスパイクが video decode
/// 経路の中で吸収されない、という二次的なストールが残る。Phase B は demux も独立
/// スレッドにして video decode と並行動作させる。
///
/// `Flush` と `Eof` は `AudioWorkerMsg` と同じセマンティクスで、順序保証は channel
/// が担保する。`Packet` には `seek_serial` を含めず、video decode thread 側が
/// `Flush` を受領した時点で `current_seek_serial` を更新する (= 順序保証で十分、
/// Mutex 不要)。
enum VideoWorkerMsg {
    /// avformat から取り出した未デコード動画 packet。video decode thread が
    /// `send_packet` → `receive_frame` → (GPU blit / swscale) → pacing →
    /// `video_tx.try_send` を行う。
    Packet(ffmpeg_the_third::Packet),
    /// シーク完了通知。video decode thread はこれを受けて自分の avcodec デコーダ
    /// を `flush()` し、`current_seek_serial` を更新、`drop_before_secs` を
    /// `target_secs` に設定する。`target_secs` が `Some(t)` のときは post-seek
    /// 1 枚目を待機する (= post_seek_frame_sent=false)、`None` (= seek 失敗) は
    /// 通常 pacing に戻す。
    Flush {
        serial: u64,
        target_secs: Option<f64>,
    },
    /// EOF 到達通知。video decode thread は何もせずに次の `Packet` か `Flush`
    /// か channel disconnect を待つ (旧 `run_decoder` の挙動と同じく、内部
    /// 残フレームは EOF で失われる)。
    Eof,
}

// HwDevice (= AVBufferRef のラッパー) を別スレッドに move するため Send を実装する。
// FFmpeg の av_buffer_ref / av_buffer_unref は内部で atomic refcount を使っているので、
// 異なるスレッドからの ref/unref は安全 (Sync は不要 = 1 thread が排他所有する形で使う)。
unsafe impl Send for HwDevice {}

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
/// `audio_pkt_tx` に enqueue するだけにする。`audio_pkt_tx` (bounded=64) が満杯に
/// なっても demux スレッドが一時停止するだけで、video decode は別経路でそのまま
/// 進行する (Phase B で demux も video decode から分離予定)。
///
/// `Flush` / `Eof` は順序保証のため packet と同じ channel に enqueue する
/// (Mutex + 別チャネルだと「Flush 通知より後に届いた packet が前世代として
/// decode される」race が起きる)。
enum AudioWorkerMsg {
    /// avformat から取り出した未デコード音声 packet。audio decode thread が
    /// `send_packet` → `receive_frame` → resample → `audio_tx.send` を行う。
    Packet(ffmpeg_the_third::Packet),
    /// シーク完了通知。audio decode thread はこれを受けたら自分の avcodec デコーダ
    /// を `flush()` して、`drop_before_secs` を `target_secs` (= preroll 中に切り
    /// 捨てる下限) に設定する。`target_secs` が `None` なら seek 失敗 (= demux 位置
    /// は動いていない) なので preroll trim は行わない。
    Flush {
        serial: u64,
        target_secs: Option<f64>,
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
    let video_avg_fps = {
        let r = video_stream.avg_frame_rate();
        let d = r.denominator();
        if d == 0 { 0.0 } else { r.numerator() as f64 / d as f64 }
    };
    // VPP ContentDesc に渡す raw 分数 (= num/den のまま渡すことで丸め誤差を排除)。
    // 0 の場合は VPP 側で 60/1 にフォールバックされる。
    let (video_fps_num, video_fps_den) = {
        let r = video_stream.avg_frame_rate();
        let n = r.numerator();
        let d = r.denominator();
        if n <= 0 || d <= 0 {
            (0u32, 0u32)
        } else {
            (n as u32, d as u32)
        }
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
    //
    // gpu_video_device が利用可能な場合は **mIV 側で作成した D3D11 デバイス** を
    // FFmpeg に渡して共有する (= HW デコーダの NV12 出力と ID3D11VideoProcessor が
    // 同じデバイス上で動き、CreateVideoProcessorInputView で受け渡せる)。
    // gpu_video_device 不在 / 失敗時は従来通り FFmpeg が新デバイスを作成し、
    // 出力は av_hwframe_transfer_data で CPU readback する旧経路。
    let codec_id = video_decoder_ctx.id();
    let hw_setup_result: Option<HwDevice> = if hw_decode_requested {
        #[cfg(windows)]
        {
            try_init_d3d11va(
                codec_id,
                &mut video_decoder_ctx,
                gpu_video_device.as_ref(),
            )
        }
        #[cfg(not(windows))]
        {
            try_init_d3d11va(codec_id, &mut video_decoder_ctx)
        }
    } else {
        None
    };
    let hw_active_initially = hw_setup_result.is_some();
    // Phase B: `_hw_device` は mut にする (= SW 再試行時に take() で空にする)。
    // 以前は `drop(_hw_device)` で move していたが、後段で video decode thread に
    // move する必要があるため、None 状態に置き換える形に変更。
    let mut _hw_device = hw_setup_result;

    let video_decoder = match video_decoder_ctx.decoder().video() {
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
                // HW デバイスを drop (= AVBufferRef を unref) して None にする。
                // この値は後段で video decode thread に move されるが、SW 再試行後は
                // None なのでそのまま渡しても問題ない。
                _hw_device = None;
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
    // Phase B: scaler / scaler_key / first_frame_logged はすべて run_video_decode の
    // ローカル変数として所有される (= デコーダ + GPU パスは別 thread)。

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
    #[cfg(windows)]
    let gpu_path_active = gpu_video_device.is_some();
    #[cfg(not(windows))]
    let gpu_path_active = false;

    // ── 埋め込みメタデータ + チャプター (Phase 5.4) ──
    // 標準キーを拾う。Matroska / MP4 / ffmetadata で共通する小文字キー名を探し、
    // 大文字違いも順に試す。値が空なら None。
    let title = read_metadata_value(&input, &["title", "TITLE"]);
    let artist = read_metadata_value(&input, &["artist", "ARTIST", "author"]);
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
        video_codec: video_codec_name.clone(),
        audio_codec: audio_setup.as_ref().map(|a| a.codec_name.clone()),
        has_audio,
        hw_decode_active: hw_active_initially,
        gpu_path_active,
        title,
        artist,
        description,
        avg_fps: video_avg_fps,
        bit_rate_bps,
        chapters,
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

    // ── Phase A (3-thread split): audio decode を独立スレッドに切り出す ──
    // 音声 packet 検出時は decode せず `audio_pkt_tx` に enqueue するだけにする。
    // これにより `audio_tx` (bounded=32) が満杯でも demux/video decode は止まらず、
    // `video_tx` (bounded=24) が枯渇しなくなる (旧構造の "buf 0/24" 振動の解消)。
    //
    // 容量 256: ~5 秒分の音声 packet (= 約 50 packets/sec × 5s) を吸収可能。
    // VST3 PDC が 2.0 秒近辺の場合に audio chain が starve しないよう、
    // demux horizon を pace_lead (0.60s) + packet queue (5s) ≒ 5.6s まで広げる
    // (= Codex 助言、2026-05-01)。compressed audio packet なのでメモリは軽い
    // (= 数 KB/packet × 256 ≒ 数百 KB)。
    let audio_stream_idx_for_demux: Option<usize> =
        audio_setup.as_ref().map(|a| a.stream_idx);
    let (audio_pkt_tx, audio_pkt_rx) = bounded::<AudioWorkerMsg>(256);
    // audio decode thread の JoinHandle。run_decoder 終了時に
    // `drop(audio_pkt_tx)` → channel disconnect → audio thread exit を経由して join する。
    // `audio_setup` は ここで consume される (= 以降 demux からは触らない)。
    let audio_decode_handle: Option<std::thread::JoinHandle<()>> = if let Some(setup) =
        audio_setup
    {
        let clock_a = clock.clone();
        let cancel_a = cancel.clone();
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
    // 容量 256: 60fps で ~4.3 秒分、120fps で ~2.1 秒分の video packet を吸収可能。
    // VST3 PDC 2.0 秒近辺で video pacing が wait に入っているとき、demux が
    // video_pkt_tx full で止まると audio packet も止まる構造だったため拡大
    // (= Codex 助言、2026-05-01)。compressed video packet なので decoded frame queue を
    // 増やすより遥かに軽い (= 数 KB-数十 KB/packet × 256 ≒ 数 MB)。
    let video_tb_num = video_time_base.numerator() as f64;
    let video_tb_den = video_time_base.denominator() as f64;
    let (video_pkt_tx, video_pkt_rx) = bounded::<VideoWorkerMsg>(256);
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
                    video_tb_num,
                    video_tb_den,
                    video_fps_num,
                    video_fps_den,
                    hw_active_initially,
                )
            })
            .expect("spawn video-decode thread")
    };
    // この時点で run_decoder = demux thread として再構成される。video_decoder /
    // _hw_device / gpu_video_device / video_tx はすべて video decode thread が所有。
    // 以下のループは demux + seek 調停 + EOF idle wait に専念する。

    // ── デコードループ (demux thread) ──

    'outer: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }

        // シーク要求を確認
        if let Some(req) = clock.take_seek_request() {
            let super::clock::SeekRequest {
                target_secs,
                serial,
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
            let backward = |input: &mut ffmpeg::format::context::Input| -> Result<(), ffmpeg::Error> {
                use ffmpeg_the_third::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
                let ret = unsafe {
                    av_seek_frame(input.as_mut_ptr(), -1, target_pts, AVSEEK_FLAG_BACKWARD as i32)
                };
                if ret >= 0 { Ok(()) } else { Err(ffmpeg::Error::from(ret)) }
            };

            // Phase 9.F (2026-04-30): 前方/後方/絶対に関係なく **常に backward+preroll**
            // を使う。旧コードは前方相対で `input.seek(target..)` (= avformat_seek_file
            // with min_ts=target、target 以降の keyframe に着地) を使い、preroll なしで
            // 「最寄り keyframe から再生」する設計だった。
            //
            // しかし forward seek は典型的な GOP (1-3 秒) で **target+0.5〜2 秒の
            // keyframe** に着地し、video 1 枚目 pts >> target になる。一方 mp4 の
            // 音声 packet 順は概ね pts 順なので、avformat 内部の read 位置が
            // keyframe + audio になっていても、最初の audio packet pts は keyframe
            // pts より少し前に出ることがある (= mp4 muxing で audio が video keyframe
            // 直前に挟まれているケース)。
            //
            // 結果として transition_to_playing 時に anchor.pts (= audio_pts) が
            // video frame pts より **数百 ms 〜数秒** 早くなり、UI tick の
            // `pts <= now + lead_tol` 判定で video frame が future 扱いになって
            // 表示が止まり、音声だけ進んで video が「早送りで追いつく」現象に。
            //
            // backward+preroll なら video/audio 両方が **target 直前の keyframe** で
            // 始まり drop_before_secs で target にトリム → 確実に同位置で再生開始。
            // GOP が長い動画では preroll decode のために 0.5〜3 秒余分にかかるが、
            // SW デコードでも 30fps wall × 数秒 = ~100ms 程度の遅延に収まり実用上
            // 問題ない。
            let mut seek_result = backward(&mut input);
            // backward が失敗したら forward を retry (= EOF 近傍など、target 以前に
            // keyframe が無い場合)。
            if seek_result.is_err() {
                crate::logger::log(format!(
                    "backward seek failed at {target_secs:.3}s, retry as forward"
                ));
                seek_result = input.seek(target_pts, target_pts..);
            }
            crate::logger::log(format!(
                "seek: target={target_secs:.3}s serial={serial} result={seek_result:?}"
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
            // - 成功時: target_secs = Some(t) → 受信側は post-seek preroll 用に
            //   drop_before_secs = Some(t) を設定 (video は post_seek_frame_sent=false に)
            // - 失敗時: target_secs = None → 受信側は preroll trim せず通常 pacing に戻す
            let target_for_flush = if seek_result.is_ok() {
                Some(target_secs)
            } else {
                None
            };
            // video_pkt_tx は drop されない (video decode thread が生きている間ずっと
            // 受信可能)。send は blocking なので順序保証されるが、cancel 中は
            // disconnect になる可能性がある (= video decode thread 終了後)。
            if video_pkt_tx
                .send(VideoWorkerMsg::Flush {
                    serial,
                    target_secs: target_for_flush,
                })
                .is_err()
            {
                break 'outer;
            }
            if audio_stream_idx_for_demux.is_some() {
                let _ = audio_pkt_tx.send(AudioWorkerMsg::Flush {
                    serial,
                    target_secs: target_for_flush,
                });
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
            let _ = engine_event_tx.try_send(
                crate::video::engine::EngineEvent::Decoder(
                    crate::video::engine::state::DecoderEvent::SeekCompleted {
                        epoch: serial,
                        actual_pts: target_secs,
                    },
                ),
            );
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
                // Phase B: video packet は decode せず video decode thread に転送する。
                // pre-decode preroll trim (= drop_before_secs check) と pacing logic は
                // すべて video decode thread 側に移管。
                //
                // `send` (blocking) を使う理由: 順序保証 channel に enqueue するので、
                // 直前の Flush marker と packet の到着順が逆転しない。bounded(64) が
                // 満杯なら demux 側を一時 stall させて逆圧をかけるのが正しい。
                if video_pkt_tx
                    .send(VideoWorkerMsg::Packet(packet))
                    .is_err()
                {
                    // video decode thread が既に終了している → 自分も exit。
                    break 'outer;
                }
                break; // 1 パケット消費したらループ先頭でシークチェック
            } else if let Some(audio_idx) = audio_stream_idx_for_demux {
                if stream.index() == audio_idx {
                    // Phase A: 音声 packet は decode せず audio decode thread に転送する。
                    // packet 段階の pre-decode preroll trim と sample-level trim は両方
                    // audio decode thread 側に移管した (= AudioWorkerMsg::Flush で渡した
                    // target_secs を audio thread が `drop_before_secs` として保持)。
                    //
                    // `send` (blocking) を使う理由: 順序保証 channel に enqueue するので、
                    // 直前の Flush marker と packet の到着順が逆転しない。bounded(64) が
                    // 満杯なら demux 側を一時 stall させて逆圧をかけるのが正しい
                    // (audio_pkt_rx 側が止まっているのに packet を取り続けると memory が
                    // 無制限に膨らむ)。stall 中も cancel は反映可能 (recv 側 disconnect で
                    // SendError → 'outer break)。
                    if audio_pkt_tx.send(AudioWorkerMsg::Packet(packet)).is_err() {
                        // audio decode thread が既に終了している (= disconnect)。
                        // VideoPlayer の shutdown 経路 → 自分も exit。
                        break 'outer;
                    }
                    break; // 1 パケット消費したらループ先頭でシークチェック
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
            // Phase A: audio decode thread にも Eof を通知して残フレームを drain させる。
            // (= 末尾の音声を確実に出し切る。drain しないと数十 ms の音声が抜ける。)
            if audio_stream_idx_for_demux.is_some() {
                let _ = audio_pkt_tx.send(AudioWorkerMsg::Eof);
            }
            // Phase B: video decode thread にも Eof を通知。動画は内部残フレームを
            // 失っても許容なので drain しないが、Eof 自体は送って状態を伝える。
            let _ = video_pkt_tx.send(VideoWorkerMsg::Eof);
            loop {
                if cancel.load(Ordering::Acquire) {
                    crate::logger::log(format!(
                        "video decoder finished: {}",
                        path.display()
                    ));
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
/// (= drop してカウンタ加算)。`video_pkt_rx` (bounded=64) は demux ↔ video decode
/// の逆圧経路として機能する。
///
/// シーク時は demux 側が `VideoWorkerMsg::Flush { serial, target_secs }` を送る。
/// この thread は `Flush` 受領で内部 decoder を `flush()` し、
/// `current_seek_serial` / `drop_before_secs` / `post_seek_frame_sent` をリセット
/// する。`target_secs.is_none()` (= seek 失敗) の場合は preroll trim せず通常
/// pacing に戻す (= post_seek_frame_sent を直ちに true)。
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
    video_tb_num: f64,
    video_tb_den: f64,
    video_fps_num: u32,
    video_fps_den: u32,
    hw_active_initially: bool,
) {
    use ffmpeg_the_third as ffmpeg;
    use ffmpeg::format::Pixel;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::video::Video;

    let mut scaler: Option<ScaleContext> = None;
    let mut scaler_key: Option<(Pixel, u32, u32)> = None;
    let mut first_frame_logged = false;
    let mut current_seek_serial: u64 = 0;
    let mut drop_before_secs: Option<f64> = None;
    let mut post_seek_frame_sent: bool = true;
    let mut last_enqueued_pts: f64 = 0.0;

    'outer: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let msg = match video_pkt_rx.recv() {
            Ok(m) => m,
            Err(_) => break, // demux thread exited → channel disconnect
        };
        let packet = match msg {
            VideoWorkerMsg::Flush { serial, target_secs } => {
                video_decoder.flush();
                current_seek_serial = serial;
                drop_before_secs = target_secs;
                // 成功時 (target_secs = Some) → post-seek 1 枚目を待つので false
                // 失敗時 (target_secs = None) → 通常 pacing に戻すので true
                post_seek_frame_sent = target_secs.is_none();
                continue;
            }
            VideoWorkerMsg::Eof => {
                // 動画は EOF で内部 frame を失っても許容 (旧 run_decoder と同じ挙動)。
                // 何もせず次の Packet/Flush/disconnect を待つ。
                continue;
            }
            VideoWorkerMsg::Packet(p) => p,
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
            let pts = frame.pts().unwrap_or(0);
            let pts_secs = (pts as f64) * video_tb_num / video_tb_den;
            // post-seek preroll: target 前のフレームは描画しない
            if let Some(min) = drop_before_secs {
                if pts_secs + 0.005 < min {
                    continue;
                } else {
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
                if let Some(gpu_dev) = gpu_video_device.as_ref() {
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
                            const AUDIO_SAFE_LO: f64 = 0.10;   // 旧 0.25 (= cap 1.5s 時代)
                            const AUDIO_SAFE_HI: f64 = 0.20;   // 旧 0.75 (= cap 300ms に整合)
                            const SEEK_BURST_LEAD_MAX_SECS: f64 = 0.20;
                            const AUDIO_CRITICAL_LO: f64 = 0.03;  // 旧 0.08 (= 将来 audio 専用 emergency 用に保持)
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
                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                    continue;
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
                                let engine_playing = engine_st
                                    == crate::video::engine::actor::state_code::PLAYING;
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
                                    std::thread::sleep(std::time::Duration::from_millis(5));
                                    continue;
                                }
                                // Phase 9.E (2026-04-30 fixup): post-seek 1 枚目は
                                // **audio_buf に関係なく必ず送出** (= override clear に必須)。
                                // 旧コード (Phase 8.F-9.D) は seek_burst 全体を
                                // `audio_buf < AUDIO_SAFE_HI` で gate していたが、forward
                                // seek (= avformat が target 後の keyframe に着地) で
                                // ahead が 0.5-2 秒になり、かつ audio buffer が満杯
                                // (= 一時停止後など) のとき、`!post_seek_frame_sent` 経路が
                                // 発火せず 1 枚目が送出できない → override が永久残留 →
                                // deadlock になっていた。
                                if clock.is_seeking() && !post_seek_frame_sent {
                                    break;
                                }
                                if clock.is_seeking()
                                    && !in_audio_escape
                                    && audio_buf < AUDIO_SAFE_HI
                                {
                                    if ahead < SEEK_BURST_LEAD_MAX_SECS {
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
                                // (= 60fps で 36 frames 相当、queue 48 frames に余裕あり)。
                                // 非 Windows では VST3 機能なし → pdc_latency=0 = 既存動作。
                                const VIDEO_QUEUE_LEAD_CAP_SECS: f64 = 0.60;
                                #[cfg(windows)]
                                let pdc_latency = clock
                                    .vst3_pdc_latency_secs()
                                    .min(crate::video::dsp::MAX_PDC_LATENCY_SECS);
                                #[cfg(not(windows))]
                                let pdc_latency: f64 = 0.0;
                                let pace_lead = if allow_pace_lead {
                                    (PACE_LEAD_SECS + pdc_latency).min(VIDEO_QUEUE_LEAD_CAP_SECS)
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
                                let _ = AUDIO_CRITICAL_LO;  // 将来 audio 専用 emergency 用に定数保持
                                if in_audio_escape && ahead < SEEK_BURST_LEAD_MAX_SECS {
                                    break;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(5));
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
                            let send_result = video_tx.try_send(gpu_frame_out);
                            let dropped_full =
                                matches!(&send_result, Err(TrySendError::Full(_)));
                            if !dropped_full && send_result.is_ok() {
                                last_enqueued_pts = pts_secs;
                                post_seek_frame_sent = true;
                            }
                            if dropped_full {
                                skipped_frame_count
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                                        (
                                            "dropped_full",
                                            serde_json::Value::from(dropped_full),
                                        ),
                                        (
                                            "pts_gap_ms",
                                            serde_json::Value::from(pts_gap * 1000.0),
                                        ),
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
                                            serde_json::Value::from(
                                                clock.audio_processed_secs(),
                                            ),
                                        ),
                                        (
                                            "audio_raw_pending_secs",
                                            serde_json::Value::from(
                                                clock.audio_raw_pending_secs(),
                                            ),
                                        ),
                                        (
                                            "audio_tx_queued_secs",
                                            serde_json::Value::from(
                                                clock.audio_tx_queued_secs(),
                                            ),
                                        ),
                                        (
                                            "pace_now",
                                            serde_json::Value::from(
                                                clock.video_pacing_now_secs(),
                                            ),
                                        ),
                                    ],
                                );
                            }
                            if let Err(TrySendError::Disconnected(_)) = send_result {
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
            const AUDIO_SAFE_LO: f64 = 0.10;   // 旧 0.25 (= 300ms cap 整合)
            const AUDIO_SAFE_HI: f64 = 0.20;   // 旧 0.75
            const SEEK_BURST_LEAD_MAX_SECS: f64 = 0.20;
            const AUDIO_CRITICAL_LO: f64 = 0.03;  // 旧 0.08 (= 将来 audio 専用 emergency 用に保持)
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
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                let audio_buf = clock.total_audio_buffer_secs();
                let audio_active = clock.is_audio_active();
                // Phase 9.D (2026-04-30): Buffering 中も PACE_LEAD で lookahead 許可
                // (詳細は GPU 経路の同コメント参照)。
                let engine_playing = engine_st
                    == crate::video::engine::actor::state_code::PLAYING;
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
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                // Phase 9.E: post-seek 1 枚目は audio_buf 不問で必ず送出
                // (詳細は GPU 経路の同コメント参照、forward seek deadlock 修正)。
                if clock.is_seeking() && !post_seek_frame_sent {
                    break;
                }
                if clock.is_seeking() && !in_audio_escape && audio_buf < AUDIO_SAFE_HI {
                    if ahead < SEEK_BURST_LEAD_MAX_SECS {
                        break;
                    }
                    if audio_active && audio_buf < AUDIO_SAFE_LO {
                        break;
                    }
                }
                // PDC-aware pace_lead with queue-cap (= GPU 経路と同じ理屈、Codex 助言改訂版)。
                // 詳細コメントは GPU 経路を参照。
                const VIDEO_QUEUE_LEAD_CAP_SECS: f64 = 0.60;
                #[cfg(windows)]
                let pdc_latency = clock
                    .vst3_pdc_latency_secs()
                    .min(crate::video::dsp::MAX_PDC_LATENCY_SECS);
                #[cfg(not(windows))]
                let pdc_latency: f64 = 0.0;
                let pace_lead = if allow_pace_lead {
                    (PACE_LEAD_SECS + pdc_latency).min(VIDEO_QUEUE_LEAD_CAP_SECS)
                } else {
                    0.0
                };
                if ahead <= pace_lead {
                    break;
                }
                // audio_escape bypass: GPU 経路と同じ理屈で `audio_buf < CRITICAL` 単独 bypass を撤去。
                let _ = AUDIO_CRITICAL_LO;  // 将来 audio 専用 emergency 用に定数保持
                if in_audio_escape && ahead < SEEK_BURST_LEAD_MAX_SECS {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
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
                        ("decode_ms", serde_json::Value::from(decode_ms.round() / 1.0)),
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
/// しない。`audio_pkt_rx` (bounded=64) は両 thread 間の逆圧経路として機能する。
///
/// シーク時は呼び出し元が `AudioWorkerMsg::Flush { serial, target_secs }` を送る。
/// この thread は `Flush` 受領で内部 decoder を `flush()` し、
/// `current_seek_serial` / `drop_before_secs` をリセットする。`target_secs` が
/// `None` なら seek 失敗 (= demux 位置は動いていない) なので preroll trim は行わない。
///
/// EOF 時は `AudioWorkerMsg::Eof` を受けて内部 decoder を flush + 残フレーム drain。
/// その後は次の `Flush` か `Packet` か channel disconnect (= run_decoder 終了) を待つ。
fn run_audio_decode(
    mut setup: AudioSetup,
    audio_pkt_rx: Receiver<AudioWorkerMsg>,
    audio_tx: Sender<AudioFrame>,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
) {
    use ffmpeg_the_third::util::frame::audio::Audio;

    // run_decoder と同じ thread-local state。Flush で reset する。
    let mut current_seek_serial: u64 = 0;
    let mut drop_before_secs: Option<f64> = None;

    'outer: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let msg = match audio_pkt_rx.recv() {
            Ok(m) => m,
            // demux 側が exit (= run_decoder 終了 / cancel) → audio_pkt_tx drop →
            // 自スレッドも exit。
            Err(_) => break,
        };
        match msg {
            AudioWorkerMsg::Flush { serial, target_secs } => {
                setup.decoder.flush();
                current_seek_serial = serial;
                drop_before_secs = target_secs;
            }
            AudioWorkerMsg::Eof => {
                // 残フレーム drain: send_eof + receive_frame ループで decoder 内の
                // 残サンプルを最後まで取り出して送る。これにより末尾の数十 ms が
                // 抜けない。FFmpeg の API では NULL packet で EOF flush を伝える。
                use ffmpeg_the_third::ffi::avcodec_send_packet;
                unsafe {
                    let _ = avcodec_send_packet(
                        setup.decoder.as_mut_ptr(),
                        std::ptr::null(),
                    );
                }
                let mut frame = Audio::empty();
                while setup.decoder.receive_frame(&mut frame).is_ok() {
                    if cancel.load(Ordering::Acquire) {
                        break 'outer;
                    }
                    if !emit_audio_frame(
                        &mut setup,
                        &frame,
                        &mut drop_before_secs,
                        current_seek_serial,
                        &clock,
                        &audio_tx,
                    ) {
                        break 'outer;
                    }
                }
                // EOF 後 decoder を flush して次回の Packet/Flush に備える。
                setup.decoder.flush();
            }
            AudioWorkerMsg::Packet(packet) => {
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
                    let pkt_pts = packet.pts().unwrap_or(i64::MIN);
                    if pkt_pts != i64::MIN {
                        let pkt_pts_secs =
                            (pkt_pts as f64) * setup.time_base_num / setup.time_base_den;
                        let pkt_dur_secs = (packet.duration().max(0) as f64)
                            * setup.time_base_num
                            / setup.time_base_den;
                        if pkt_pts_secs + pkt_dur_secs < min - 0.020 {
                            continue;
                        }
                    }
                }
                if let Err(e) = setup.decoder.send_packet(&packet) {
                    crate::logger::log(format!("audio send_packet: {e}"));
                    continue;
                }
                let mut frame = Audio::empty();
                while setup.decoder.receive_frame(&mut frame).is_ok() {
                    if cancel.load(Ordering::Acquire) {
                        break 'outer;
                    }
                    if !emit_audio_frame(
                        &mut setup,
                        &frame,
                        &mut drop_before_secs,
                        current_seek_serial,
                        &clock,
                        &audio_tx,
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
    frame: &ffmpeg_the_third::util::frame::audio::Audio,
    drop_before_secs: &mut Option<f64>,
    current_seek_serial: u64,
    clock: &AvClock,
    audio_tx: &Sender<AudioFrame>,
) -> bool {
    use ffmpeg_the_third as ffmpeg;
    use ffmpeg::format::sample::{Sample, Type as SampleType};
    use ffmpeg::util::frame::audio::Audio;

    let pts = frame.pts().unwrap_or(0);
    let mut pts_secs = (pts as f64) * setup.time_base_num / setup.time_base_den;
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
    const CHANNELS: usize = 2;
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
    let mut samples: Vec<f32> = unsafe {
        let raw_ptr = (*resampled.as_ptr()).data[0] as *const f32;
        debug_assert!(!raw_ptr.is_null());
        std::slice::from_raw_parts(raw_ptr, element_count).to_vec()
    };

    // post-seek preroll の trim:
    // avformat_seek は keyframe に戻るので、target_secs 未満の
    // 音声フレームが届く。完全に target 前ならフレーム破棄、
    // 跨ぐなら先頭 N サンプルを drain して target ぴったりから始める。
    if let Some(min) = *drop_before_secs {
        let frame_secs =
            (samples.len() / CHANNELS) as f64 / setup.out_rate as f64;
        if pts_secs + frame_secs <= min {
            // 完全に target 前 → 捨てる
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
    let duration_secs = (samples.len() / 2) as f64 / setup.out_rate as f64;
    let frame_out = AudioFrame {
        samples,
        pts_secs,
        seek_serial: current_seek_serial,
        duration_secs,
    };
    // tx queued 合計を **send 前に加算**。pump.recv 後の減算と
    // 競合しないよう順序を保つ。失敗時はロールバック。
    clock.add_audio_tx_queued_secs(duration_secs);
    if audio_tx.send(frame_out).is_err() {
        clock.add_audio_tx_queued_secs(-duration_secs);
        return false;
    }
    true
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
///
/// `gpu_video_device` が `Some` の場合は **そのデバイスを FFmpeg と共有** する
/// (= mIV の VideoProcessor が同じデバイスで動作可能、CreateVideoProcessorInputView
/// の前提)。`None` の場合は FFmpeg が独自デバイスを作る (旧経路)。
fn try_init_d3d11va(
    codec_id: ffmpeg_the_third::codec::Id,
    ctx: &mut ffmpeg_the_third::codec::context::Context,
    #[cfg(windows)] gpu_video_device: Option<
        &std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
    >,
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

        // 3. HW デバイスコンテキスト作成。
        //    gpu_video_device があれば mIV の D3D11 デバイスを共有、なければ FFmpeg
        //    が新デバイスを作る (= 旧経路)。
        #[cfg(windows)]
        let buf_ref: *mut AVBufferRef = if let Some(gpu_dev) = gpu_video_device {
            match crate::video::gpu_renderer::create_ffmpeg_hw_device_ctx(gpu_dev) {
                Ok(b) => {
                    crate::logger::log(format!(
                        "HW: D3D11VA shared with GpuVideoDevice for codec {codec_id:?}"
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
                crate::logger::log(format!(
                    "HW: av_hwdevice_ctx_create(D3D11VA) failed: {ret}"
                ));
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
                crate::logger::log(format!(
                    "HW: av_hwdevice_ctx_create(D3D11VA) failed: {ret}"
                ));
                return None;
            }
            buf
        };

        // 4. AVCodecContext にぶら下げる + get_format コールバック
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
    None
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
    let ten_bit = matches!(
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
            ten_bit,
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
                ("decode_path", serde_json::Value::from("hw_d3d11va_gpu_blit")),
                (
                    "frame_pix_fmt",
                    serde_json::Value::from(format!("{:?}", in_desc.Format)),
                ),
                ("frame_w", serde_json::Value::from(in_desc.Width as i64)),
                ("frame_h", serde_json::Value::from(in_desc.Height as i64)),
                (
                    "hw_active",
                    serde_json::Value::from(hw_active_initially),
                ),
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
        ten_bit,
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
