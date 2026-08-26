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
//! と許容秒数を通知し、[`ThumbnailWorker::nearest`] で結果を受け取る。worker は busy なら新しい
//! request で **古い request を捨てて最新だけ処理** する (drain semantics)。
//!
//! ## キャッシュキー
//!
//! 実際に得られた frame の PTS をキーにした `BTreeMap` を使う。要求ごとの許容秒数で
//! range 検索するため、粗い Remote 要求が exact な marker 要求を汚染しない。
//!
//! ## メモリ
//!
//! The cache has no fixed entry-count cap. A `VideoPlayer` owns one worker, so
//! generated thumbnails live only while that video player is alive and are
//! released together with it. For scale, 400 thumbnails at 320x180 RGBA are
//! about 92 MB.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Sender, bounded};

/// サムネ画像のピクセルサイズ (16:9)。HUD で表示する際は egui 側で更にスケールできる。
/// Phase 5.2 で 160x90 → 320x180 に 2x 拡大 (見た目改善)。
pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;
const PTS_KEY_UNITS_PER_SECOND: f64 = 1_000_000_000.0;

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
    cache: BTreeMap<i64, Thumbnail>,
    resolved_requests: HashMap<ThumbnailRequestKey, i64>,
    attempted_requests: HashSet<ThumbnailRequestKey>,
}

impl ThumbnailState {
    fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            resolved_requests: HashMap::new(),
            attempted_requests: HashSet::new(),
        }
    }

    fn get_nearest(&self, target_secs: f64, tolerance_secs: f64) -> Option<(i64, Thumbnail)> {
        if !valid_target_and_tolerance(target_secs, tolerance_secs) {
            return None;
        }

        let target_key = pts_key(target_secs);
        let min_key = pts_key((target_secs - tolerance_secs).max(0.0)).saturating_sub(1);
        let max_key = pts_key(target_secs + tolerance_secs).saturating_add(1);

        // 時間軸を進めたときに表示時刻が戻りにくいよう、許容範囲内に過去側が
        // 1 枚でもあれば、target に最も近い過去側を優先する。
        if let Some((key, thumbnail)) = self
            .cache
            .range(min_key..=target_key.saturating_add(1))
            .rev()
            .find(|(_, thumbnail)| {
                thumbnail.target_secs <= target_secs
                    && is_within_tolerance(target_secs, thumbnail.target_secs, tolerance_secs)
            })
        {
            return Some((*key, thumbnail.clone()));
        }

        self.cache
            .range(target_key.saturating_sub(1)..=max_key)
            .find(|(_, thumbnail)| {
                thumbnail.target_secs >= target_secs
                    && is_within_tolerance(target_secs, thumbnail.target_secs, tolerance_secs)
            })
            .map(|(key, thumbnail)| (*key, thumbnail.clone()))
    }

    fn lookup(&mut self, request: ThumbnailRequest) -> Option<Thumbnail> {
        let request_key = request.key();
        if let Some(cache_key) = self.resolved_requests.get(&request_key).copied() {
            if let Some(thumbnail) = self.cache.get(&cache_key) {
                return Some(thumbnail.clone());
            }
            self.resolved_requests.remove(&request_key);
        }

        let (cache_key, thumbnail) =
            self.get_nearest(request.target_secs, request.lookup_tolerance_secs)?;
        self.resolved_requests.insert(request_key, cache_key);
        Some(thumbnail)
    }

    fn insert_for_request(&mut self, request: ThumbnailRequest, thumbnail: Thumbnail) {
        let cache_key = pts_key(thumbnail.target_secs);
        self.cache.insert(cache_key, thumbnail);
        self.resolved_requests.insert(request.key(), cache_key);
        self.attempted_requests.insert(request.key());
    }

    fn mark_attempted(&mut self, request: ThumbnailRequest) {
        self.attempted_requests.insert(request.key());
    }

    fn was_attempted(&self, request: ThumbnailRequest) -> bool {
        self.attempted_requests.contains(&request.key())
    }
}

fn pts_key(secs: f64) -> i64 {
    (secs.max(0.0) * PTS_KEY_UNITS_PER_SECOND).round() as i64
}

fn valid_target_and_tolerance(target_secs: f64, tolerance_secs: f64) -> bool {
    target_secs.is_finite()
        && target_secs >= 0.0
        && tolerance_secs.is_finite()
        && tolerance_secs >= 0.0
}

pub(crate) fn is_within_tolerance(target_secs: f64, actual_secs: f64, tolerance_secs: f64) -> bool {
    valid_target_and_tolerance(target_secs, tolerance_secs)
        && actual_secs.is_finite()
        && actual_secs >= 0.0
        && (actual_secs - target_secs).abs() <= tolerance_secs
}

/// 設定値とシークバー 1 物理 px 相当の秒数から、要求ごとの実効許容を求める。
pub(crate) fn effective_tolerance_from_physical_pixels(
    setting_secs: f64,
    duration_secs: f64,
    bar_width_points: f64,
    pixels_per_point: f64,
) -> f64 {
    let setting_secs = if setting_secs.is_finite() {
        setting_secs.clamp(0.0, 30.0)
    } else {
        1.0
    };
    let physical_width = bar_width_points * pixels_per_point;
    let seconds_per_pixel = if duration_secs.is_finite()
        && duration_secs > 0.0
        && physical_width.is_finite()
        && physical_width > 0.0
    {
        duration_secs / physical_width
    } else {
        0.0
    };
    setting_secs.max(seconds_per_pixel)
}

/// 0.0 要求だけは同一フレームとみなせる幅を cache reuse に認める。
pub(crate) fn cache_lookup_tolerance(tolerance_secs: f64, avg_fps: f64) -> f64 {
    if tolerance_secs == 0.0 {
        if avg_fps.is_finite() && avg_fps > 1.0 {
            (1.0 / avg_fps).clamp(1.0 / 1000.0, 1.0)
        } else {
            1.0 / 30.0
        }
    } else {
        tolerance_secs
    }
}

/// backward seek で着地した frame をそのまま採用できるかを決める純関数。
pub(crate) fn should_adopt_seek_keyframe(
    target_secs: f64,
    keyframe_secs: f64,
    tolerance_secs: f64,
) -> bool {
    valid_target_and_tolerance(target_secs, tolerance_secs)
        && keyframe_secs.is_finite()
        && keyframe_secs >= 0.0
        && keyframe_secs <= target_secs
        && target_secs - keyframe_secs <= tolerance_secs
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ThumbnailRequest {
    target_secs: f64,
    /// backward seek の着地点を採用してよい距離。利用者の要求をそのまま保持する。
    tolerance_secs: f64,
    /// cache hit と seeking indicator に使う距離。要求が 0.0 の場合だけ 1 frame 再利用を含む。
    lookup_tolerance_secs: f64,
}

impl ThumbnailRequest {
    fn new(target_secs: f64, tolerance_secs: f64, lookup_tolerance_secs: f64) -> Option<Self> {
        if !valid_target_and_tolerance(target_secs, tolerance_secs)
            || !valid_target_and_tolerance(target_secs, lookup_tolerance_secs)
            || lookup_tolerance_secs < tolerance_secs
        {
            return None;
        }
        Some(Self {
            target_secs,
            tolerance_secs,
            lookup_tolerance_secs,
        })
    }

    fn key(self) -> ThumbnailRequestKey {
        ThumbnailRequestKey {
            target_bits: self.target_secs.to_bits(),
            tolerance_bits: self.tolerance_secs.to_bits(),
            lookup_tolerance_bits: self.lookup_tolerance_secs.to_bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ThumbnailRequestKey {
    target_bits: u64,
    tolerance_bits: u64,
    lookup_tolerance_bits: u64,
}

#[derive(Default)]
struct ThumbnailRequestState {
    pending: Option<ThumbnailRequest>,
    in_flight: Option<ThumbnailRequest>,
}

impl ThumbnailRequestState {
    fn enqueue_latest(&mut self, request: ThumbnailRequest) -> bool {
        if self.pending == Some(request) {
            return false;
        }
        if self.in_flight == Some(request) {
            self.pending = None;
            return false;
        }
        self.pending = Some(request);
        true
    }

    fn take_pending(&mut self) -> Option<ThumbnailRequest> {
        let request = self.pending.take()?;
        self.in_flight = Some(request);
        Some(request)
    }

    fn supersedes(&self, request: ThumbnailRequest) -> bool {
        self.pending.is_some_and(|pending| pending != request)
    }

    fn finish(&mut self, request: ThumbnailRequest) {
        if self.in_flight == Some(request) {
            self.in_flight = None;
        }
    }
}

/// VideoPlayer 1 つにつき 1 つ作る。Drop で worker thread が停止する。
pub struct ThumbnailWorker {
    /// target と要求ごとの許容を一体で所有する latest-wins state。
    requests: Arc<Mutex<ThumbnailRequestState>>,
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
    pub(crate) fn spawn(
        path: PathBuf,
        hw_decode: bool,
        ui_wake: Arc<crate::video::VideoUiWake>,
    ) -> Self {
        let (wake_tx, wake_rx) = bounded::<()>(1);
        let requests = Arc::new(Mutex::new(ThumbnailRequestState::default()));
        let state = Arc::new(Mutex::new(ThumbnailState::new()));
        let cancel = Arc::new(AtomicBool::new(false));

        let worker_requests = requests.clone();
        let worker_state = state.clone();
        let worker_cancel = cancel.clone();
        let worker_ui_wake = Arc::clone(&ui_wake);
        let thread = std::thread::Builder::new()
            .name("video-thumb".into())
            .spawn(move || {
                run_worker(
                    path,
                    hw_decode,
                    wake_rx,
                    worker_requests,
                    worker_state,
                    worker_cancel,
                    worker_ui_wake,
                );
            })
            .ok();

        Self {
            requests,
            wake_tx,
            state,
            cancel,
            thread,
        }
    }

    /// 「この `target_secs` のサムネが欲しい」と要求ごとの許容付きで通知する。
    /// 上書きセマンティクスなので、マウス連続移動でも最新の要求だけが残る。
    pub fn request(&self, target_secs: f64, tolerance_secs: f64, lookup_tolerance_secs: f64) {
        let Some(request) =
            ThumbnailRequest::new(target_secs, tolerance_secs, lookup_tolerance_secs)
        else {
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.lookup(request).is_some() || state.was_attempted(request) {
                return;
            }
        }
        if self.requests.lock().unwrap().enqueue_latest(request) {
            let _ = self.wake_tx.try_send(());
        }
    }

    /// 実 PTS cache を要求ごとの許容範囲で検索する。過去側を優先し、同じ要求について
    /// 一度選んだ picture は後から別 consumer が cache を増やしても差し替えない。
    pub fn nearest(
        &self,
        target_secs: f64,
        tolerance_secs: f64,
        lookup_tolerance_secs: f64,
    ) -> Option<Thumbnail> {
        let request = ThumbnailRequest::new(target_secs, tolerance_secs, lookup_tolerance_secs)?;
        self.state.lock().unwrap().lookup(request)
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
    scaled_w: u32,
    scaled_h: u32,
    dst_w: u32,
    dst_h: u32,
    orientation: crate::video::display_metadata::VideoOrientation,
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
        let orientation = crate::video::display_metadata::orientation_from_stream(&video_stream);
        let params_ref = video_stream.parameters();
        let sar = params_ref.sample_aspect_ratio();
        let (sar_num, sar_den) =
            crate::video::decoder::normalize_sar(sar.numerator(), sar.denominator());
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
        let (dst_w, dst_h) = crate::video::display_metadata::fit_display_within(
            src_w,
            src_h,
            sar_num,
            sar_den,
            orientation,
            THUMB_W,
            THUMB_H,
        );
        let (scaled_w, scaled_h) = if orientation.swaps_axes() {
            (dst_h, dst_w)
        } else {
            (dst_w, dst_h)
        };
        crate::logger::log(format!(
            "video-thumb: decoder ready codec={} decoder={} decode_path={} d3d11va_supported={} d3d11va_config={} src_size={}x{} scale_size={}x{} display_size={}x{} orientation={:?}",
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
            scaled_w,
            scaled_h,
            dst_w,
            dst_h,
            orientation,
        ));

        Ok(Self {
            input,
            stream_idx,
            tb_num,
            tb_den,
            src_w,
            src_h,
            scaled_w,
            scaled_h,
            dst_w,
            dst_h,
            orientation,
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
        request: ThumbnailRequest,
        requests: &Mutex<ThumbnailRequestState>,
        cancel: &AtomicBool,
    ) -> Result<DecodeOutcome, String> {
        use ffmpeg::format::Pixel;
        use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
        use ffmpeg::util::frame::video::Video;
        use ffmpeg_the_third as ffmpeg;

        let target_secs = request.target_secs;
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
        let mut landed_frame_checked = false;
        let hw_decode_active = self.hw_decode_active();

        // backward seek 後の keyframe から target_secs に到達する frame まで decode
        // し続ける。長い GOP でも正しい時刻に近いサムネを返すため decode 数に
        // 固定上限は置かず、cancel と新 request による supersede で止める。
        for item in self.input.packets() {
            if cancel.load(Ordering::Acquire) {
                return Ok(DecodeOutcome::Superseded);
            }
            if requests.lock().unwrap().supersedes(request) {
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
                if !landed_frame_checked {
                    landed_frame_checked = true;
                    if should_adopt_seek_keyframe(target_secs, pts_secs, request.tolerance_secs) {
                        got_frame = Some(frame);
                        break;
                    }
                }
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
        if !is_within_tolerance(target_secs, frame_pts_secs, request.lookup_tolerance_secs) {
            return Ok(DecodeOutcome::NoFrame);
        }

        // HW (D3D11) frame は SW download してから scaler に渡す。SW frame はそのまま。
        let mut sw_holder: Option<Video> = None;
        let frame_for_scaler =
            crate::video::swscale_helpers::prepare_frame_for_swscale(&frame, &mut sw_holder)
                .map_err(|e| e.to_string())?;

        let cur_src_fmt = frame_for_scaler.format();
        if self.scaler.is_none() || self.scaler_src_fmt != Some(cur_src_fmt) {
            crate::logger::log(format!(
                "video-thumb: -> ScaleContext::get src_fmt={cur_src_fmt:?} src_size={}x{} dst_size={}x{}",
                self.src_w, self.src_h, self.scaled_w, self.scaled_h
            ));
            let scaler = ScaleContext::get(
                cur_src_fmt,
                self.src_w,
                self.src_h,
                Pixel::RGBA,
                self.scaled_w,
                self.scaled_h,
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
        let needed = (self.scaled_w * 4) as usize;
        let plane = rgba.data(0);
        let buf: Vec<u8> = if stride == needed {
            plane[..needed * self.scaled_h as usize].to_vec()
        } else {
            let mut out = Vec::with_capacity(needed * self.scaled_h as usize);
            for row in 0..self.scaled_h as usize {
                let start = row * stride;
                out.extend_from_slice(&plane[start..start + needed]);
            }
            out
        };

        let (oriented_w, oriented_h, buf) = crate::video::display_metadata::orient_rgba(
            self.scaled_w,
            self.scaled_h,
            &buf,
            self.orientation,
        )?;
        debug_assert_eq!((oriented_w, oriented_h), (self.dst_w, self.dst_h));

        Ok(DecodeOutcome::Ready(Thumbnail {
            target_secs: frame_pts_secs,
            width: oriented_w,
            height: oriented_h,
            rgba: Arc::new(buf),
        }))
    }
}

fn run_worker(
    path: PathBuf,
    hw_decode: bool,
    wake_rx: crossbeam_channel::Receiver<()>,
    requests: Arc<Mutex<ThumbnailRequestState>>,
    state: Arc<Mutex<ThumbnailState>>,
    cancel: Arc<AtomicBool>,
    ui_wake: Arc<crate::video::VideoUiWake>,
) {
    let mut decoder: Option<SeekThumbnailDecoder> = None;
    let mut hw_decode_failed = false;

    while !cancel.load(Ordering::Acquire) {
        let request = requests.lock().unwrap().take_pending();
        let Some(request) = request else {
            // 起床通知を 100ms タイムアウトで待つ。タイムアウトでも cancel を再確認する。
            match wake_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        };
        let target_secs = request.target_secs;

        // キャッシュヒットなら何もしない
        if state.lock().unwrap().lookup(request).is_some() {
            requests.lock().unwrap().finish(request);
            ui_wake.wake();
            continue;
        }

        if decoder.is_none() {
            let use_hw = hw_decode && !hw_decode_failed;
            decoder = match SeekThumbnailDecoder::open(&path, use_hw) {
                Ok(decoder) => Some(decoder),
                Err(e) => {
                    crate::logger::log(format!("video-thumb: decoder open failed: {e}"));
                    ui_wake.wake();
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
            .decode_thumbnail(request, &requests, &cancel);
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
                        state.lock().unwrap().mark_attempted(request);
                        requests.lock().unwrap().finish(request);
                        ui_wake.wake();
                        continue;
                    }
                };
                decode_path = sw_decoder.decode_path().to_string();
                match sw_decoder.decode_thumbnail(request, &requests, &cancel) {
                    Ok(outcome) => {
                        decoder = Some(sw_decoder);
                        outcome
                    }
                    Err(sw_err) => {
                        crate::logger::log(format!("video-thumb: SW fallback failed: {sw_err}"));
                        decoder = Some(sw_decoder);
                        state.lock().unwrap().mark_attempted(request);
                        requests.lock().unwrap().finish(request);
                        ui_wake.wake();
                        continue;
                    }
                }
            }
            Err(e) => {
                crate::logger::log(format!("video-thumb: decode failed: {e}"));
                state.lock().unwrap().mark_attempted(request);
                requests.lock().unwrap().finish(request);
                ui_wake.wake();
                continue;
            }
        };

        let thumb = match outcome {
            DecodeOutcome::Ready(thumb) => thumb,
            DecodeOutcome::NoFrame => {
                state.lock().unwrap().mark_attempted(request);
                requests.lock().unwrap().finish(request);
                ui_wake.wake();
                continue;
            }
            DecodeOutcome::Superseded => {
                requests.lock().unwrap().finish(request);
                continue;
            }
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
                    (
                        "tolerance_secs",
                        serde_json::Value::from(request.tolerance_secs),
                    ),
                    ("actual_secs", serde_json::Value::from(thumb.target_secs)),
                    (
                        "decode_ms",
                        serde_json::Value::from(decode_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("decode_path", serde_json::Value::from(decode_path)),
                ],
            );
        }
        state.lock().unwrap().insert_for_request(request, thumb);
        requests.lock().unwrap().finish(request);
        ui_wake.wake();
    }
    crate::logger::log("video-thumb: terminated");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumbnail(actual_secs: f64) -> Thumbnail {
        Thumbnail {
            target_secs: actual_secs,
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0, 0, 0, 255]),
        }
    }

    fn request(target_secs: f64, tolerance_secs: f64) -> ThumbnailRequest {
        ThumbnailRequest::new(target_secs, tolerance_secs, tolerance_secs).unwrap()
    }

    #[test]
    fn consecutive_drag_requests_keep_only_the_latest_pending_target() {
        let mut requests = ThumbnailRequestState::default();
        requests.enqueue_latest(request(1.0, 1.0));
        requests.enqueue_latest(request(8.0, 2.0));
        requests.enqueue_latest(request(21.5, 4.0));

        assert_eq!(requests.take_pending(), Some(request(21.5, 4.0)));
    }

    #[test]
    fn per_request_tolerance_keeps_consumers_separate() {
        let mut state = ThumbnailState::new();
        state.cache.insert(pts_key(84.0), thumbnail(84.0));

        assert!(state.lookup(request(100.0, 1.0)).is_none());
        assert!(state.lookup(request(100.0, 0.0)).is_none());
        assert_eq!(
            state.lookup(request(100.0, 16.0)).unwrap().target_secs,
            84.0
        );
        assert!(state.lookup(request(100.0, 1.0)).is_none());
        assert!(state.lookup(request(100.0, 0.0)).is_none());
    }

    #[test]
    fn range_search_prefers_nearest_frame_at_or_before_target_and_rejects_outside() {
        let mut state = ThumbnailState::new();
        state.cache.insert(pts_key(8.9), thumbnail(8.9));
        state.cache.insert(pts_key(9.4), thumbnail(9.4));
        state.cache.insert(pts_key(10.1), thumbnail(10.1));

        assert_eq!(state.lookup(request(10.0, 1.0)).unwrap().target_secs, 9.4);
        assert!(state.lookup(request(12.0, 1.0)).is_none());
    }

    #[test]
    fn resolved_request_keeps_its_first_picture() {
        let mut state = ThumbnailState::new();
        state.cache.insert(pts_key(96.0), thumbnail(96.0));
        let coarse = request(100.0, 5.0);
        assert_eq!(state.lookup(coarse).unwrap().target_secs, 96.0);

        state.cache.insert(pts_key(99.0), thumbnail(99.0));
        assert_eq!(state.lookup(coarse).unwrap().target_secs, 96.0);
    }

    #[test]
    fn keyframe_adoption_is_decided_from_target_landing_and_tolerance() {
        assert!(should_adopt_seek_keyframe(100.0, 93.0, 7.0));
        assert!(!should_adopt_seek_keyframe(100.0, 92.999, 7.0));
        assert!(!should_adopt_seek_keyframe(100.0, 100.001, 7.0));
        assert!(should_adopt_seek_keyframe(100.0, 100.0, 0.0));
    }

    #[test]
    fn effective_tolerance_uses_physical_bar_pixels() {
        assert_eq!(
            effective_tolerance_from_physical_pixels(0.5, 600.0, 300.0, 2.0),
            1.0
        );
        assert_eq!(
            effective_tolerance_from_physical_pixels(2.0, 600.0, 300.0, 2.0),
            2.0
        );
    }

    #[test]
    fn zero_tolerance_reuses_only_within_one_frame() {
        let lookup = cache_lookup_tolerance(0.0, 25.0);
        assert_eq!(lookup, 0.04);
        assert!(is_within_tolerance(10.0, 9.961, lookup));
        assert!(!is_within_tolerance(10.0, 9.959, lookup));
    }

    #[test]
    fn seeking_indicator_match_follows_request_tolerance() {
        assert!(!is_within_tolerance(10.0, 9.2, 0.5));
        assert!(is_within_tolerance(10.0, 9.2, 1.0));
    }
}
