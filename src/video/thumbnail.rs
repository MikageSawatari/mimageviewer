//! 動画シークバー上のホバープレビュー用サムネイル抽出。
//!
//! ## 設計
//!
//! メインの `decoder` とは **完全に独立** したワーカースレッドが、同じ動画ファイルを
//! 別の [`ffmpeg::format::Input`] で開き、初回 cache miss で長寿命の補助デコーダを
//! 生成する。動画設定で HW decode が有効なら FFmpeg-owned D3D11VA を優先し、
//! 初期化や readback、または `get_format` 起因の `send_packet` 失敗時は SW decode に
//! フォールバックする。要求された `target_secs` の前後の keyframe から 1 枚だけデコードし、`swscale` で
//! `THUMB_W x THUMB_H` の RGBA に変換 → キャッシュに格納する。
//!
//! UI スレッドは [`ThumbnailWorker::request`] で「この target_secs のサムネが欲しい」
//! と通知し、[`ThumbnailWorker::nearest`] で結果を受け取る。worker は busy なら新しい
//! request で **古い request を捨てて最新だけ処理** する (drain semantics)。
//!
//! ## キャッシュ粒度
//!
//! `target_secs` を [`SECONDS_PER_BUCKET`] 秒単位で丸めて整数キーにしている
//! (例: 0.5s 単位)。ホバー時のマウス連続移動でキャッシュヒット率を高めるため。
//!
//! ## メモリ
//!
//! The cache has no fixed entry-count cap. A `VideoPlayer` owns one worker, so
//! generated thumbnails live only while that video player is alive and are
//! released together with it. For scale, 400 thumbnails at 320x180 RGBA are
//! about 92 MB.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Sender, bounded};

/// サムネ画像のピクセルサイズ (16:9)。HUD で表示する際は egui 側で更にスケールできる。
/// Phase 5.2 で 160x90 → 320x180 に 2x 拡大 (見た目改善)。
pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;
/// キャッシュ key の粒度 (秒)。シーク サムネ + 動画ジャンプパネル左サムネで共通の
/// 粒度を使う必要がある (= UI が同 pts に対して同じ key を期待するため)。
pub const SECONDS_PER_BUCKET: f64 = 0.5;

/// 1 件のサムネイル (RGBA、`THUMB_W x THUMB_H` 固定とは限らないので w/h を持つ)。
#[derive(Clone)]
pub struct Thumbnail {
    /// 実際にデコードできた frame の PTS (秒)。要求時刻ではなく実フレーム時刻なので、
    /// UI 側はこれを使って「目標位置と合っているか」を正しく判定できる。
    pub target_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// サムネイルキャッシュ + 最新リクエスト追跡。worker と UI で共有する。
struct ThumbnailState {
    cache: HashMap<i64, Thumbnail>,
}

impl ThumbnailState {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    fn get_nearest(&self, target_secs: f64) -> Option<Thumbnail> {
        // 完全ヒット優先
        let key = bucket_key(target_secs);
        if let Some(t) = self.cache.get(&key) {
            return Some(t.clone());
        }
        // 近傍 (前後 1 バケット) も許容
        for d in [-1, 1, -2, 2] {
            if let Some(t) = self.cache.get(&(key + d)) {
                return Some(t.clone());
            }
        }
        None
    }

    fn insert(&mut self, key: i64, thumb: Thumbnail) {
        self.cache.insert(key, thumb);
    }
}

/// シークサムネのキャッシュキー (秒 → 0.5 秒バケット整数)。同 pts の連続要求で
/// hit させるため、ホバー側 (シークバー / 動画左ジャンプパネル) からも同一関数を
/// 使う必要がある。
pub fn bucket_key(secs: f64) -> i64 {
    (secs / SECONDS_PER_BUCKET).round() as i64
}

/// 「pending リクエスト無し」を表す sentinel。`f64::NAN` の bits とは別の値を使う
/// (NaN bits は無数にあるので比較しづらい)。`u64::MAX` を予約。
const PENDING_NONE: u64 = u64::MAX;

/// VideoPlayer 1 つにつき 1 つ作る。Drop で worker thread が停止する。
pub struct ThumbnailWorker {
    /// 最新リクエストの target_secs (f64 bits)、または PENDING_NONE。
    /// UI が `request` で常に最新の target を上書き、worker が `swap(PENDING_NONE)` で取り出す。
    pending_target_bits: Arc<AtomicU64>,
    /// 起床通知 (内容は使わない、capacity 1 の signal だけ)。
    wake_tx: Sender<()>,
    state: Arc<Mutex<ThumbnailState>>,
    cancel: Arc<AtomicBool>,
    /// `Drop` で `cancel = true` + thread.join。`Option` は drop 順序のため。
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ThumbnailWorker {
    /// 動画ファイルパスを渡してワーカーを起動。
    /// 内部で `ffmpeg::format::input` を別途開くので、メインデコーダの状態には影響しない。
    /// 失敗しても `Some(worker)` を返し、worker thread 内で諦めて即終了する
    /// (UI 側はサムネが返らないだけ)。
    pub fn spawn(path: PathBuf, hw_decode: bool) -> Self {
        let (wake_tx, wake_rx) = bounded::<()>(1);
        let pending_target_bits = Arc::new(AtomicU64::new(PENDING_NONE));
        let state = Arc::new(Mutex::new(ThumbnailState::new()));
        let cancel = Arc::new(AtomicBool::new(false));

        let worker_pending = pending_target_bits.clone();
        let worker_state = state.clone();
        let worker_cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("video-thumb".into())
            .spawn(move || {
                run_worker(
                    path,
                    hw_decode,
                    wake_rx,
                    worker_pending,
                    worker_state,
                    worker_cancel,
                );
            })
            .ok();

        Self {
            pending_target_bits,
            wake_tx,
            state,
            cancel,
            thread,
        }
    }

    /// 「この `target_secs` のサムネが欲しい」と通知する。
    /// `pending_target_bits` を最新値で上書きし、wake channel を try_send で叩く。
    /// 上書きセマンティクスなので「マウス連続移動で古い target が処理待ちで残り
    /// 最新が落ちる」ことが起きない。
    pub fn request(&self, target_secs: f64) {
        self.pending_target_bits
            .store(target_secs.to_bits(), Ordering::Release);
        let _ = self.wake_tx.try_send(());
    }

    /// 直近のキャッシュから target_secs に最も近いものを取り出す。
    /// 厳密ヒットしなくても前後 ±1 〜 ±2 バケット内にあれば返す (連続スクラブ向け)。
    pub fn nearest(&self, target_secs: f64) -> Option<Thumbnail> {
        self.state.lock().unwrap().get_nearest(target_secs)
    }
}

impl Drop for ThumbnailWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        // UI スレッドで join すると、動画間ホイール移動や S-mode の動画切替時に
        // ffmpeg seek/decode 中の worker を数十〜数百 ms 待つことがある。worker は
        // Arc 所有の状態だけを触り、cancel/wake 切断で自力終了できるため detach する。
        let _ = self.thread.take();
    }
}

enum DecodeOutcome {
    Ready(Thumbnail),
    Superseded,
    NoFrame,
}

struct SeekThumbnailDecoder {
    input: ffmpeg_the_third::format::context::Input,
    stream_idx: usize,
    tb_num: f64,
    tb_den: f64,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    scaler: Option<ffmpeg_the_third::software::scaling::Context>,
    scaler_src_fmt: Option<ffmpeg_the_third::format::Pixel>,
    decoder: crate::video::decoder::AuxVideoDecoder,
}

impl SeekThumbnailDecoder {
    fn open(path: &Path, hw_preferred: bool) -> Result<Self, String> {
        use ffmpeg::media::Type as MediaType;
        use ffmpeg_the_third as ffmpeg;

        ffmpeg::init().map_err(|e| format!("ffmpeg init failed: {e}"))?;
        let input = ffmpeg::format::input(path).map_err(|e| format!("open input failed: {e}"))?;
        let video_stream = input
            .streams()
            .best(MediaType::Video)
            .ok_or_else(|| "video stream not found".to_string())?;
        let stream_idx = video_stream.index();
        let time_base = video_stream.time_base();
        let tb_num = time_base.numerator() as f64;
        let tb_den = time_base.denominator() as f64;
        let params_ref = video_stream.parameters();
        let params = crate::video::decoder::clone_codec_parameters(&params_ref)?;
        let codec_id = params.id();

        let decoder = crate::video::decoder::open_aux_video_decoder_with_fallback(
            &params,
            codec_id,
            hw_preferred,
            "video-thumb",
        )?;
        let src_w = decoder.width();
        let src_h = decoder.height();
        let (dst_w, dst_h) = fit_within(src_w, src_h, THUMB_W, THUMB_H);
        crate::logger::log(format!(
            "video-thumb: decoder ready codec={} decoder={} decode_path={} d3d11va_supported={} d3d11va_config={} src_size={}x{} dst_size={}x{}",
            codec_id.name(),
            decoder.decoder_name(),
            if decoder.hw_decode_active() {
                "hw_d3d11va"
            } else {
                "sw"
            },
            decoder.d3d11va_supported(),
            decoder.d3d11va_config(),
            src_w,
            src_h,
            dst_w,
            dst_h,
        ));

        Ok(Self {
            input,
            stream_idx,
            tb_num,
            tb_den,
            src_w,
            src_h,
            dst_w,
            dst_h,
            scaler: None,
            scaler_src_fmt: None,
            decoder,
        })
    }

    fn hw_decode_active(&self) -> bool {
        self.decoder.hw_decode_active()
    }

    fn decode_path(&self) -> &'static str {
        if self.hw_decode_active() {
            "hw_d3d11va"
        } else {
            "sw"
        }
    }

    fn decode_thumbnail(
        &mut self,
        target_secs: f64,
        pending_target_bits: &AtomicU64,
        cancel: &AtomicBool,
    ) -> Result<DecodeOutcome, String> {
        use ffmpeg::format::Pixel;
        use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
        use ffmpeg::util::frame::video::Video;
        use ffmpeg_the_third as ffmpeg;

        let target_pts = (target_secs * 1_000_000.0) as i64;
        let seek_ok = unsafe {
            use ffmpeg::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
            av_seek_frame(
                self.input.as_mut_ptr(),
                -1,
                target_pts,
                AVSEEK_FLAG_BACKWARD as i32,
            ) >= 0
        };
        if !seek_ok {
            return Ok(DecodeOutcome::NoFrame);
        }
        self.decoder.decoder_mut().flush();

        let mut got_frame: Option<Video> = None;
        let mut last_frame: Option<Video> = None;
        let mut superseded = false;
        let hw_decode_active = self.hw_decode_active();

        // backward seek 後の keyframe から target_secs に到達する frame まで decode
        // し続ける。長い GOP でも正しい時刻に近いサムネを返すため decode 数に
        // 固定上限は置かず、cancel と新 request による supersede で止める。
        for item in self.input.packets() {
            if cancel.load(Ordering::Acquire) {
                return Ok(DecodeOutcome::Superseded);
            }
            // overlay は同 bucket に同じ request を再送することがあるため、別 bucket
            // だけを supersede とみなす。同 bucket は完了後の cache hit で消化する。
            let pending = pending_target_bits.load(Ordering::Acquire);
            if pending != PENDING_NONE
                && bucket_key(f64::from_bits(pending)) != bucket_key(target_secs)
            {
                superseded = true;
                break;
            }

            let (stream, packet) = match item {
                Ok(sp) => sp,
                Err(_) => break,
            };
            if stream.index() != self.stream_idx {
                continue;
            }
            if let Err(e) = self.decoder.decoder_mut().send_packet(&packet) {
                if hw_decode_active {
                    return Err(format!("HW send_packet failed: {e}"));
                }
                continue;
            }
            let mut frame = Video::empty();
            while self.decoder.decoder_mut().receive_frame(&mut frame).is_ok() {
                let Some(ts) = crate::video::decoder::video_frame_timestamp(&frame) else {
                    got_frame = Some(frame);
                    break;
                };
                let pts_secs = ts as f64 * self.tb_num / self.tb_den;
                if pts_secs >= target_secs {
                    got_frame = Some(frame);
                    break;
                }
                last_frame = Some(frame);
                frame = Video::empty();
            }
            if got_frame.is_some() {
                break;
            }
        }
        if superseded {
            return Ok(DecodeOutcome::Superseded);
        }

        let Some(frame) = got_frame.or(last_frame) else {
            return Ok(DecodeOutcome::NoFrame);
        };
        let frame_pts_secs = crate::video::decoder::video_frame_timestamp(&frame)
            .map(|pts| pts as f64 * self.tb_num / self.tb_den)
            .unwrap_or(target_secs);

        // HW (D3D11) frame は SW download してから scaler に渡す。SW frame はそのまま。
        let mut sw_holder: Option<Video> = None;
        let frame_for_scaler =
            crate::video::swscale_helpers::prepare_frame_for_swscale(&frame, &mut sw_holder)
                .map_err(|e| e.to_string())?;

        let cur_src_fmt = frame_for_scaler.format();
        if self.scaler.is_none() || self.scaler_src_fmt != Some(cur_src_fmt) {
            crate::logger::log(format!(
                "video-thumb: -> ScaleContext::get src_fmt={cur_src_fmt:?} src_size={}x{} dst_size={}x{}",
                self.src_w, self.src_h, self.dst_w, self.dst_h
            ));
            let scaler = ScaleContext::get(
                cur_src_fmt,
                self.src_w,
                self.src_h,
                Pixel::RGBA,
                self.dst_w,
                self.dst_h,
                ScaleFlags::BILINEAR,
            )
            .map_err(|e| format!("sws_scale init failed: {e}"))?;
            self.scaler = Some(scaler);
            self.scaler_src_fmt = Some(cur_src_fmt);
            crate::logger::log("video-thumb: <- ScaleContext::get ok");
        }

        let scaler_ref = self.scaler.as_mut().expect("scaler initialized above");
        let mut rgba = Video::empty();
        scaler_ref
            .run(frame_for_scaler, &mut rgba)
            .map_err(|e| format!("sws_scale failed: {e}"))?;

        let stride = rgba.stride(0);
        let needed = (self.dst_w * 4) as usize;
        let plane = rgba.data(0);
        let buf: Vec<u8> = if stride == needed {
            plane[..needed * self.dst_h as usize].to_vec()
        } else {
            let mut out = Vec::with_capacity(needed * self.dst_h as usize);
            for row in 0..self.dst_h as usize {
                let start = row * stride;
                out.extend_from_slice(&plane[start..start + needed]);
            }
            out
        };

        Ok(DecodeOutcome::Ready(Thumbnail {
            target_secs: frame_pts_secs,
            width: self.dst_w,
            height: self.dst_h,
            rgba: Arc::new(buf),
        }))
    }
}

fn run_worker(
    path: PathBuf,
    hw_decode: bool,
    wake_rx: crossbeam_channel::Receiver<()>,
    pending_target_bits: Arc<AtomicU64>,
    state: Arc<Mutex<ThumbnailState>>,
    cancel: Arc<AtomicBool>,
) {
    let mut decoder: Option<SeekThumbnailDecoder> = None;
    let mut hw_decode_failed = false;

    while !cancel.load(Ordering::Acquire) {
        // 起床通知を 100ms タイムアウトで待つ。タイムアウトでも cancel 再確認のため continue。
        match wake_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(()) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // 念のため pending を確認 (起床通知がなぜか落ちた場合の保険)
                if pending_target_bits.load(Ordering::Acquire) == PENDING_NONE {
                    continue;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
        let bits = pending_target_bits.swap(PENDING_NONE, Ordering::AcqRel);
        if bits == PENDING_NONE {
            continue;
        }
        let target_secs = f64::from_bits(bits);

        // キャッシュヒットなら何もしない
        let key = bucket_key(target_secs);
        if state.lock().unwrap().cache.contains_key(&key) {
            continue;
        }

        if decoder.is_none() {
            let use_hw = hw_decode && !hw_decode_failed;
            decoder = match SeekThumbnailDecoder::open(&path, use_hw) {
                Ok(decoder) => Some(decoder),
                Err(e) => {
                    crate::logger::log(format!("video-thumb: decoder open failed: {e}"));
                    return;
                }
            };
        }

        let decode_t0 = std::time::Instant::now();
        let mut decode_path = decoder
            .as_ref()
            .map(|decoder| decoder.decode_path())
            .unwrap_or("unknown")
            .to_string();
        let decode_result = decoder
            .as_mut()
            .expect("decoder opened above")
            .decode_thumbnail(target_secs, &pending_target_bits, &cancel);
        let outcome = match decode_result {
            Ok(outcome) => outcome,
            Err(e)
                if decoder
                    .as_ref()
                    .is_some_and(|decoder| decoder.hw_decode_active()) =>
            {
                crate::logger::log(format!(
                    "video-thumb: HW decode failed; retrying with SW: {e}"
                ));
                hw_decode_failed = true;
                decoder = None;
                let mut sw_decoder = match SeekThumbnailDecoder::open(&path, false) {
                    Ok(decoder) => decoder,
                    Err(open_err) => {
                        crate::logger::log(format!(
                            "video-thumb: SW fallback open failed: {open_err}"
                        ));
                        continue;
                    }
                };
                decode_path = sw_decoder.decode_path().to_string();
                match sw_decoder.decode_thumbnail(target_secs, &pending_target_bits, &cancel) {
                    Ok(outcome) => {
                        decoder = Some(sw_decoder);
                        outcome
                    }
                    Err(sw_err) => {
                        crate::logger::log(format!("video-thumb: SW fallback failed: {sw_err}"));
                        decoder = Some(sw_decoder);
                        continue;
                    }
                }
            }
            Err(e) => {
                crate::logger::log(format!("video-thumb: decode failed: {e}"));
                continue;
            }
        };

        let DecodeOutcome::Ready(thumb) = outcome else {
            continue;
        };
        if crate::perf::is_enabled() {
            let path_key = path.display().to_string();
            crate::perf::event(
                "video_thumb",
                "ready",
                Some(&path_key),
                0,
                &[
                    ("target_secs", serde_json::Value::from(target_secs)),
                    ("actual_secs", serde_json::Value::from(thumb.target_secs)),
                    (
                        "decode_ms",
                        serde_json::Value::from(decode_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("decode_path", serde_json::Value::from(decode_path)),
                ],
            );
        }
        state.lock().unwrap().insert(key, thumb);
    }
    crate::logger::log("video-thumb: terminated");
}

fn fit_within(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (max_w, max_h);
    }
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let w = ((src_w as f64 * scale).round() as u32).max(1);
    let h = ((src_h as f64 * scale).round() as u32).max(1);
    (w, h)
}
