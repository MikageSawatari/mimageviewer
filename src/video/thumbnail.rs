//! 動画シークバー上のホバープレビュー用サムネイル抽出。
//!
//! ## 設計
//!
//! メインの `decoder` とは **完全に独立** したワーカースレッドが、同じ動画ファイルを
//! 別の [`ffmpeg::format::Input`] で開き、要求された `target_secs` の前後の keyframe を
//! 1 枚だけデコード → `swscale` で `THUMB_W x THUMB_H` の RGBA に変換 → キャッシュ
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
//! The cache has no fixed entry-count cap. A `VideoPlayer` owns one worker, so
//! generated thumbnails live only while that video player is alive and are
//! released together with it. For scale, 400 thumbnails at 320x180 RGBA are
//! about 92 MB.

use std::collections::HashMap;
use std::path::PathBuf;
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

fn run_worker(
    path: PathBuf,
    wake_rx: crossbeam_channel::Receiver<()>,
    pending_target_bits: Arc<AtomicU64>,
    state: Arc<Mutex<ThumbnailState>>,
    cancel: Arc<AtomicBool>,
) {
    use ffmpeg::format::Pixel;
    use ffmpeg::media::Type as MediaType;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::video::Video;
    use ffmpeg_the_third as ffmpeg;

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

    // アスペクト比を保ちつつ最大 THUMB_W x THUMB_H に収める
    let (dst_w, dst_h) = fit_within(src_w, src_h, THUMB_W, THUMB_H);

    // `decoder.format()` ベースで scaler を事前構築すると、HW accel が auto-attach
    // された場合に `Pixel::D3D11` が返って swscale の `av_assert0` → `abort()` で
    // プロセスごと落ちる (2026-05-12 crash 解析、`ucrtbase!abort` で fast fail)。
    // 代わりに **最初の frame を取った後、`frame.format()` で scaler を lazy 構築**
    // する方式に切り替える。HW frame は `prepare_frame_for_swscale` で
    // `av_hwframe_transfer_data` 経由で SW download してから swscale に渡す。
    // これにより HW decode の高速性を維持しつつ swscale av_assert0 を回避できる。
    let mut scaler: Option<ScaleContext> = None;
    let mut scaler_src_fmt: Option<Pixel> = None;

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
            av_seek_frame(
                input.as_mut_ptr(),
                -1,
                target_pts,
                AVSEEK_FLAG_BACKWARD as i32,
            ) >= 0
        };
        if !seek_ok {
            continue;
        }
        decoder.flush();

        let mut got_frame: Option<Video> = None;
        let mut last_frame: Option<Video> = None;
        // backward seek 後の keyframe から target_secs に到達する frame まで decode
        // し続ける。decode 数に上限は設けない: 長い GOP (実測 5.5s ≈ 165 frame の
        // 動画あり) でも必ず target のフレームを採用するため。上限を置くと GOP 長に
        // よってサムネが再生開始位置からずれる。
        // 暴走対策は 2 つ: (1) cancel フラグ (= Drop) で worker 自体を止める、
        // (2) より新しい hover request (`pending_target_bits` が更新された) を検知
        // したら現在の decode を捨てて最新 target に乗り換える。上限撤廃後はこの
        // (2) が無いと、長 GOP / PTS 欠落ファイルで 1 request が EOF まで走り、
        // スクラブ中の後続 request が全部詰まる。
        let mut superseded = false;
        for item in input.packets() {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            if pending_target_bits.load(Ordering::Acquire) != PENDING_NONE {
                // より新しい hover request が来た。現在の decode を捨てて outer
                // loop に戻り、最新 target を処理する (drain semantics の維持)。
                superseded = true;
                break;
            }
            let (stream, packet) = match item {
                Ok(sp) => sp,
                Err(_) => break,
            };
            if stream.index() != stream_idx {
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            let mut frame = Video::empty();
            while decoder.receive_frame(&mut frame).is_ok() {
                // 再生デコーダと同じ best-effort timestamp を使う。PTS 欠落系の
                // AVI/ASF/古い DivX で `frame.pts()` が None になり、判定が壊れて
                // EOF まで走るのを防ぐ。timestamp が全く取れない壊れたストリーム
                // では seek 直後の最初の frame をそのまま採用する。
                let Some(ts) = crate::video::decoder::video_frame_timestamp(&frame) else {
                    got_frame = Some(frame);
                    break;
                };
                let pts_secs = ts as f64 * tb_num / tb_den;
                // 再生で target_secs にシークしたとき表示される frame と一致させる
                // ため「target_secs 以降の最初の frame」を採用する。以前は
                // `target_secs - SECONDS_PER_BUCKET` で 0.5s 手前の frame を拾って
                // いた (= ホバーサムネが再生開始位置とずれる一因)。
                if pts_secs >= target_secs {
                    got_frame = Some(frame);
                    break;
                }
                // target 未到達の frame は last_frame として保持。動画末尾付近の
                // hover で target が最終フレームの pts を超え、EOF まで到達しない
                // ケースの fallback に使う。
                last_frame = Some(frame);
                frame = Video::empty();
            }
            if got_frame.is_some() {
                break;
            }
        }
        if superseded {
            continue;
        }

        let Some(frame) = got_frame.or(last_frame) else {
            continue;
        };
        // 実際にデコードできた frame の PTS をサムネに記録する。要求 target_secs を
        // そのまま保存すると、長い GOP で decode 上限に当たって target 手前の frame
        // しか取れなかった場合に、UI 側の一致判定 (thumbnail_matches) が「正しい
        // 時刻のサムネ」と誤認してしまう。実 PTS を持たせれば、ずれている間は
        // 「シーク中」box が出て誤表示にならない。
        let frame_pts_secs = frame
            .pts()
            .map(|pts| pts as f64 * tb_num / tb_den)
            .unwrap_or(target_secs);

        // HW (D3D11) frame は SW download してから scaler に渡す。SW frame はそのまま。
        let mut sw_holder: Option<Video> = None;
        let frame_for_scaler = match crate::video::swscale_helpers::prepare_frame_for_swscale(
            &frame,
            &mut sw_holder,
        ) {
            Ok(f) => f,
            Err(e) => {
                crate::logger::log(format!("video-thumb: {e}"));
                continue;
            }
        };
        // scaler を lazy 構築 / src_fmt 変化時に再構築。
        let cur_src_fmt = frame_for_scaler.format();
        if scaler.is_none() || scaler_src_fmt != Some(cur_src_fmt) {
            crate::logger::log(format!(
                "video-thumb: -> ScaleContext::get src_fmt={cur_src_fmt:?} src_size={src_w}x{src_h} dst_size={dst_w}x{dst_h}"
            ));
            match ScaleContext::get(
                cur_src_fmt,
                src_w,
                src_h,
                Pixel::RGBA,
                dst_w,
                dst_h,
                ScaleFlags::BILINEAR,
            ) {
                Ok(s) => {
                    scaler = Some(s);
                    scaler_src_fmt = Some(cur_src_fmt);
                    crate::logger::log("video-thumb: <- ScaleContext::get ok");
                }
                Err(e) => {
                    crate::logger::log(format!("video-thumb: sws_scale init failed: {e}"));
                    continue;
                }
            }
        }
        let scaler_ref = scaler.as_mut().expect("scaler initialized above");
        let mut rgba = Video::empty();
        if scaler_ref.run(frame_for_scaler, &mut rgba).is_err() {
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
            target_secs: frame_pts_secs,
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
