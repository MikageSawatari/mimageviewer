//! 動画タイル モード (Phase 5.5) 用のサムネイル一括抽出ワーカー。
//!
//! 既存の `crate::video::thumbnail::ThumbnailWorker` はホバー時の単発リクエスト用
//! (= `request()` は毎回上書きで最新のみ処理) で、タイルモードが要求する
//! 「N 個のタイムスタンプを順に処理して全部保持」という用途には合わない。
//!
//! 本モジュールは:
//! - `spawn(path, timestamps, max_w, max_h)` で N 個 (例: 10x10 = 100) のフレームを
//!   バックグラウンドで順番に抽出する。
//! - メインデコーダー (= 再生用) と独立した `ffmpeg::format::Input` を別途 open
//!   するので、再生中の動画を停めずに動く。
//! - 結果は `Arc<Mutex<Vec<Option<TileThumbnail>>>>` に蓄積され、UI は `snapshot()`
//!   で共有 read。
//! - 完了 (= 全 timestamps 処理) または cancel で thread 終了。Drop で確実に join。
//!
//! ## キャッシュ寿命
//! 1 つの `TileThumbnailWorker` は (動画 path, interval_secs, max_w/h) のキーで
//! 一意に対応する。VideoPlayer 内 (= フルスクリーン中) で生存し、Drop で worker と
//! ピクセルデータをまとめて解放する。複数の interval を切り替えたいときは
//! 新しい worker を spawn し直す (= 旧 worker の Drop で thread を終わらせる)。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::tile_thumb_cache::TileThumbCache;

/// 1 タイル分の抽出結果。
#[derive(Clone)]
pub struct TileThumbnail {
    /// 元タイムスタンプ (秒)。クリック時の seek 先に使う。
    pub pts_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// タイルモード抽出ワーカー。
pub struct TileThumbnailWorker {
    /// 結果ストレージ: spawn 時に与えた timestamps と同じ長さの Vec。各要素は
    /// 抽出完了済なら Some、未済 / 失敗なら None。
    state: Arc<Mutex<Vec<Option<TileThumbnail>>>>,
    cancel: Arc<AtomicBool>,
    /// 完了フラグ (= worker thread が処理を終えたら true)。
    finished: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TileThumbnailWorker {
    /// `timestamps` 長の結果スロットを 0 埋めで用意し、worker thread を起動する。
    /// `max_w` / `max_h` でアスペクト保持リサイズの上限を指定。
    /// `cache` が Some なら DB ヒット → WebP デコード経路で抽出を skip し、ミスは
    /// 抽出後に WebP エンコードして書き戻す (= Phase 6.D-2、Phase 8.C で絶対 PTS 化)。
    /// `video_mtime` はキャッシュ無効化判定用。
    pub fn spawn(
        path: PathBuf,
        timestamps: Vec<f64>,
        max_w: u32,
        max_h: u32,
        cache: Option<Arc<TileThumbCache>>,
        video_mtime: i64,
    ) -> Self {
        let n = timestamps.len();
        let state = Arc::new(Mutex::new(vec![None; n]));
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));

        let worker_state = state.clone();
        let worker_cancel = cancel.clone();
        let worker_finished = finished.clone();
        let thread = std::thread::Builder::new()
            .name("video-tile-thumbs".into())
            .spawn(move || {
                run_worker(
                    path,
                    timestamps,
                    max_w,
                    max_h,
                    worker_state,
                    worker_cancel.clone(),
                    cache,
                    video_mtime,
                );
                worker_finished.store(true, Ordering::Release);
            })
            .ok();

        Self {
            state,
            cancel,
            finished,
            thread,
        }
    }

    /// 現在までに抽出済の結果を借用なしで返す (= clone)。UI が毎フレーム呼ぶ。
    /// 進捗表示用に「完了済み数 / 総数」の取得は `progress()` を使うとよい。
    pub fn snapshot(&self) -> Vec<Option<TileThumbnail>> {
        self.state.lock().unwrap().clone()
    }

    /// (完了済み数, 総数) を返す。UI のプログレスバー用。
    pub fn progress(&self) -> (usize, usize) {
        let s = self.state.lock().unwrap();
        let total = s.len();
        let done = s.iter().filter(|t| t.is_some()).count();
        (done, total)
    }

    /// worker が走り終わっているか。
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

impl Drop for TileThumbnailWorker {
    fn drop(&mut self) {
        // UI スレッドから join を待つと数百 ms ブロックする
        // (= ffmpeg の seek/decode/swscale が cancel チェック粒度より長い)。
        // worker thread は cancel フラグを Acquire で 1 タイムスタンプ毎に確認するので、
        // 切り替え後は数百 ms 以内に自然終了する。Drop では join せず detach 扱いに
        // する: JoinHandle を捨てるだけで thread は最後まで走って終わる。スレッドが
        // 所有する `ffmpeg::format::Input` も終了時に解放される。
        self.cancel.store(true, Ordering::Release);
        let _ = self.thread.take();
    }
}

fn run_worker(
    path: PathBuf,
    timestamps: Vec<f64>,
    max_w: u32,
    max_h: u32,
    state: Arc<Mutex<Vec<Option<TileThumbnail>>>>,
    cancel: Arc<AtomicBool>,
    cache: Option<Arc<TileThumbCache>>,
    video_mtime: i64,
) {
    use ffmpeg::format::Pixel;
    use ffmpeg::media::Type as MediaType;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::video::Video;
    use ffmpeg_the_third as ffmpeg;

    // 起動直後にキャッシュをまとめてチェックして state に load。残った None スロット
    // だけが ffmpeg 抽出の対象。キーは絶対 PTS なので間隔切替時にも再利用される。
    // 1 度の Mutex 取得で全スロットを照会する (simplify P2)。
    if let Some(c) = cache.as_ref() {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let ts_ms: Vec<i64> = timestamps
            .iter()
            .map(|&p| (p * 1000.0).round() as i64)
            .collect();
        let hits = c.lookup_webp_batch(&path, &ts_ms, video_mtime);
        for (idx, (&pts, webp_opt)) in timestamps.iter().zip(hits.into_iter()).enumerate() {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            if let Some(webp) = webp_opt {
                if let Some((w, h, rgba)) = crate::catalog::decode_thumb_to_rgba(&webp) {
                    let thumb = TileThumbnail {
                        pts_secs: pts,
                        width: w,
                        height: h,
                        rgba: Arc::new(rgba),
                    };
                    if let Ok(mut s) = state.lock() {
                        if idx < s.len() {
                            s[idx] = Some(thumb);
                        }
                    }
                }
            }
        }
    }

    if let Err(e) = ffmpeg::init() {
        crate::logger::log(format!("video-tile-thumb: ffmpeg init failed: {e}"));
        return;
    }
    // 全スロット既にキャッシュから埋まっているなら ffmpeg open 自体スキップ。
    {
        let s = state.lock().unwrap();
        if s.iter().all(|t| t.is_some()) {
            return;
        }
    }
    let mut input = match ffmpeg::format::input(&path) {
        Ok(i) => i,
        Err(e) => {
            crate::logger::log(format!("video-tile-thumb: open input failed: {e}"));
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
            crate::logger::log(format!("video-tile-thumb: codec ctx failed: {e}"));
            return;
        }
    };
    let mut decoder = match codec_ctx.decoder().video() {
        Ok(d) => d,
        Err(e) => {
            crate::logger::log(format!("video-tile-thumb: decoder open failed: {e}"));
            return;
        }
    };
    let src_w = decoder.width();
    let src_h = decoder.height();
    let (dst_w, dst_h) = fit_within(src_w, src_h, max_w, max_h);
    // `decoder.format()` ベースの事前 scaler 構築は HW accel attach 時に
    // `Pixel::D3D11` を返して swscale `av_assert0` → `abort()` を踏むため、
    // **最初の frame を取った後の `frame.format()` で scaler を lazy 構築** する
    // 方式に切り替える。HW frame は `prepare_frame_for_swscale` で SW download。
    // 詳細は `src/video/swscale_helpers.rs` のドキュメントコメント参照。
    let mut scaler: Option<ScaleContext> = None;
    let mut scaler_src_fmt: Option<Pixel> = None;

    for (idx, &target_secs) in timestamps.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        // 既にキャッシュからロード済みのスロットは skip
        {
            let s = state.lock().unwrap();
            if s.get(idx).map(|t| t.is_some()).unwrap_or(false) {
                continue;
            }
        }
        // backward seek + 1 frame
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
        let mut video_packets_seen = 0;
        let mut frames_tried = 0;
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
                if pts_secs >= target_secs - 0.5 {
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
        // HW (D3D11) frame は SW download してから scaler に渡す。SW frame はそのまま。
        let mut sw_holder: Option<Video> = None;
        let frame_for_scaler = match crate::video::swscale_helpers::prepare_frame_for_swscale(
            &frame,
            &mut sw_holder,
        ) {
            Ok(f) => f,
            Err(e) => {
                crate::logger::log(format!("video-tile-thumb: {e}"));
                continue;
            }
        };
        // scaler を lazy 構築 / src_fmt 変化時に再構築。
        let cur_src_fmt = frame_for_scaler.format();
        if scaler.is_none() || scaler_src_fmt != Some(cur_src_fmt) {
            crate::logger::log(format!(
                "video-tile-thumb: -> ScaleContext::get src_fmt={cur_src_fmt:?} src_size={src_w}x{src_h} dst_size={dst_w}x{dst_h}"
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
                    crate::logger::log("video-tile-thumb: <- ScaleContext::get ok");
                }
                Err(e) => {
                    crate::logger::log(format!("video-tile-thumb: sws_scale init failed: {e}"));
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
        // Phase 6.D-2: 抽出済 RGBA を WebP に encode してキャッシュに書く
        // (失敗しても extraction 経路は止まらない)。Phase 8.C: 絶対 PTS キー化。
        if let Some(c) = cache.as_ref() {
            let encoder = webp::Encoder::from_rgba(&buf, dst_w, dst_h);
            // q=70: グリッドサムネと同等品位、サイズ優先
            let webp_bytes = encoder.encode(70.0).to_vec();
            let timestamp_ms = (target_secs * 1000.0).round() as i64;
            if let Err(e) =
                c.store_webp(&path, max_w, timestamp_ms, video_mtime, dst_h, &webp_bytes)
            {
                crate::logger::log(format!("video-tile-thumb cache store failed: {e}"));
            }
        }

        let thumb = TileThumbnail {
            pts_secs: target_secs,
            width: dst_w,
            height: dst_h,
            rgba: Arc::new(buf),
        };
        if let Ok(mut s) = state.lock() {
            if idx < s.len() {
                s[idx] = Some(thumb);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_within_preserves_aspect() {
        // 16:9 source, fit into 320x180 → (320, 180)
        let (w, h) = fit_within(1920, 1080, 320, 180);
        assert_eq!((w, h), (320, 180));
        // 4:3 source, fit into 320x180 → (240, 180)
        let (w, h) = fit_within(800, 600, 320, 180);
        assert_eq!((w, h), (240, 180));
        // tall source, fit into 320x180 → (101, 180)
        let (w, h) = fit_within(360, 640, 320, 180);
        assert_eq!(h, 180);
        assert!((w as i32 - 101).abs() <= 1);
    }
}
