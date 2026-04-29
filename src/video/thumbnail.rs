//! 動画シークバー上のホバープレビュー用サムネイル抽出。
//!
//! ## 設計
//!
//! メインの `decoder` とは **完全に独立** したワーカースレッドが、同じ動画ファイルを
//! 別の [`ffmpeg::format::Input`] で開き、要求された `target_secs` の前後の keyframe を
//! 1 枚だけデコード → `swscale` で `THUMB_W x THUMB_H` の RGBA に変換 → LRU キャッシュ
//! に格納する。
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
//! `MAX_ENTRIES * THUMB_W * THUMB_H * 4` バイト = 32 × 320 × 180 × 4 ≈ 7.4 MB
//! (Phase 5.2 でサイズを 2x に拡大、見た目を改善)。VideoPlayer 1 つにつき 1 worker
//! なので、フルスクリーンで 1 動画開いている間だけ確保される。Drop で worker と
//! 一緒に解放。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Sender, bounded};

/// サムネ画像のピクセルサイズ (16:9)。HUD で表示する際は egui 側で更にスケールできる。
/// Phase 5.2 で 160x90 → 320x180 に 2x 拡大 (見た目改善)。
pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;
/// LRU 容量。これ以上は古い順に捨てる。
const MAX_ENTRIES: usize = 32;
/// キャッシュ key の粒度 (秒)。
const SECONDS_PER_BUCKET: f64 = 0.5;

/// 1 件のサムネイル (RGBA、`THUMB_W x THUMB_H` 固定とは限らないので w/h を持つ)。
#[derive(Clone)]
pub struct Thumbnail {
    pub target_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// LRU キャッシュ + 最新リクエスト追跡。worker と UI で共有する。
struct ThumbnailState {
    cache: HashMap<i64, Thumbnail>,
    lru: VecDeque<i64>,
}

impl ThumbnailState {
    fn new() -> Self {
        Self {
            cache: HashMap::with_capacity(MAX_ENTRIES),
            lru: VecDeque::with_capacity(MAX_ENTRIES),
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
        if self.cache.contains_key(&key) {
            // すでにある → LRU 順だけ更新 (新しい方の rgba で上書き)
            self.lru.retain(|k| *k != key);
        } else if self.cache.len() >= MAX_ENTRIES {
            if let Some(old) = self.lru.pop_front() {
                self.cache.remove(&old);
            }
        }
        self.cache.insert(key, thumb);
        self.lru.push_back(key);
    }
}

fn bucket_key(secs: f64) -> i64 {
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
    pub fn spawn(path: PathBuf) -> Self {
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
                run_worker(path, wake_rx, worker_pending, worker_state, worker_cancel);
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
    /// 最新が落ちる」(Codex 指摘) ことが起きない。
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
        if let Some(t) = self.thread.take() {
            // worker は recv_timeout で 100ms 以内に cancel をチェックする想定
            let _ = t.join();
        }
    }
}

fn run_worker(
    path: PathBuf,
    wake_rx: crossbeam_channel::Receiver<()>,
    pending_target_bits: Arc<AtomicU64>,
    state: Arc<Mutex<ThumbnailState>>,
    cancel: Arc<AtomicBool>,
) {
    use ffmpeg_the_third as ffmpeg;
    use ffmpeg::format::Pixel;
    use ffmpeg::media::Type as MediaType;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::video::Video;

    if let Err(e) = ffmpeg::init() {
        crate::logger::log(format!("video-thumb: ffmpeg init failed: {e}"));
        return;
    }

    let mut input = match ffmpeg::format::input(&path) {
        Ok(i) => i,
        Err(e) => {
            crate::logger::log(format!("video-thumb: open input failed: {e}"));
            return;
        }
    };
    let video_stream = match input.streams().best(MediaType::Video) {
        Some(s) => s,
        None => return,
    };
    let stream_idx = video_stream.index();
    let time_base = video_stream.time_base();
    let tb_num = time_base.numerator() as f64;
    let tb_den = time_base.denominator() as f64;
    let params = video_stream.parameters();

    let codec_ctx = match ffmpeg::codec::context::Context::from_parameters(params) {
        Ok(c) => c,
        Err(e) => {
            crate::logger::log(format!("video-thumb: codec ctx failed: {e}"));
            return;
        }
    };
    let mut decoder = match codec_ctx.decoder().video() {
        Ok(d) => d,
        Err(e) => {
            crate::logger::log(format!("video-thumb: decoder open failed: {e}"));
            return;
        }
    };
    let src_w = decoder.width();
    let src_h = decoder.height();
    let src_fmt = decoder.format();

    // アスペクト比を保ちつつ最大 THUMB_W x THUMB_H に収める
    let (dst_w, dst_h) = fit_within(src_w, src_h, THUMB_W, THUMB_H);

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
            crate::logger::log(format!("video-thumb: sws_scale init failed: {e}"));
            return;
        }
    };

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

        // backward seek + 1 フレームだけデコード
        let target_pts = (target_secs * 1_000_000.0) as i64;
        let seek_ok = unsafe {
            use ffmpeg::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
            av_seek_frame(input.as_mut_ptr(), -1, target_pts, AVSEEK_FLAG_BACKWARD as i32) >= 0
        };
        if !seek_ok {
            continue;
        }
        decoder.flush();

        let mut got_frame: Option<Video> = None;
        let mut video_packets_seen = 0;
        let mut frames_tried = 0;
        // video packet を最大 120 個まで処理する (= keyframe → target が
        // 数秒先でも届く余裕)。subtitle/data ストリームのパケットは数えない
        // (Codex 指摘: take(120) を全 packet で数えると音声/字幕で枠を食い潰す)。
        for item in input.packets() {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let (stream, packet) = match item {
                Ok(sp) => sp,
                Err(_) => break,
            };
            if stream.index() != stream_idx {
                continue;
            }
            video_packets_seen += 1;
            if video_packets_seen > 120 {
                break;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            let mut frame = Video::empty();
            while decoder.receive_frame(&mut frame).is_ok() {
                let pts = frame.pts().unwrap_or(0);
                let pts_secs = pts as f64 * tb_num / tb_den;
                if pts_secs >= target_secs - SECONDS_PER_BUCKET {
                    got_frame = Some(frame);
                    break;
                }
                frames_tried += 1;
                if frames_tried > 60 {
                    got_frame = Some(frame);
                    break;
                }
                frame = Video::empty();
            }
            if got_frame.is_some() {
                break;
            }
        }

        let Some(frame) = got_frame else {
            continue;
        };

        let mut rgba = Video::empty();
        if scaler.run(&frame, &mut rgba).is_err() {
            continue;
        }
        let stride = rgba.stride(0);
        let needed = (dst_w * 4) as usize;
        let plane = rgba.data(0);
        let buf: Vec<u8> = if stride == needed {
            plane[..needed * dst_h as usize].to_vec()
        } else {
            let mut out = Vec::with_capacity(needed * dst_h as usize);
            for row in 0..dst_h as usize {
                let start = row * stride;
                out.extend_from_slice(&plane[start..start + needed]);
            }
            out
        };

        let thumb = Thumbnail {
            target_secs,
            width: dst_w,
            height: dst_h,
            rgba: Arc::new(buf),
        };
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
