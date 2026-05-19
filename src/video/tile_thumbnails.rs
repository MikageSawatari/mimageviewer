//! 動画タイル モード (Phase 5.5) 用のサムネイル一括抽出ワーカー。
//!
//! 既存の `crate::video::thumbnail::ThumbnailWorker` はホバー時の単発リクエスト用
//! (= `request()` は毎回上書きで最新のみ処理) で、タイルモードが要求する
//! 「N 個のタイムスタンプを順に処理して全部保持」という用途には合わない。
//!
//! 本モジュールは:
//! - `spawn(path, timestamps, max_w, max_h, hw_decode, ...)` で N 個
//!   (例: 10x10 = 100) のフレームをバックグラウンドで順番に抽出する。
//! - メインデコーダー (= 再生用) と独立した `ffmpeg::format::Input` を別途 open
//!   するので、再生中の動画を停めずに動く。
//! - `hw_decode` が true ならシークバーのサムネイルと同じ補助 D3D11VA デコーダを
//!   優先し、初期化 / decode 失敗時はワーカー内で SW デコードにフォールバックする。
//! - 結果は `Arc<Mutex<Vec<Option<TileThumbnail>>>>` に蓄積され、UI は `snapshot()`
//!   で共有 read。
//! - 完了 (= 全 timestamps 処理) または cancel で thread 終了。Drop は cancel を立て、
//!   UI スレッドを止めないよう worker の自然終了に任せる。
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

#[derive(Clone, Copy)]
enum CacheWriteKind {
    Tile,
    Resume,
}

#[derive(Clone)]
struct CacheTarget {
    cache: Arc<TileThumbCache>,
    write_kind: CacheWriteKind,
}

impl CacheTarget {
    fn tile(cache: Arc<TileThumbCache>) -> Self {
        Self {
            cache,
            write_kind: CacheWriteKind::Tile,
        }
    }

    fn resume(cache: Arc<TileThumbCache>) -> Self {
        Self {
            cache,
            write_kind: CacheWriteKind::Resume,
        }
    }
}

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
        hw_decode: bool,
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
        let thread = match std::thread::Builder::new()
            .name("video-tile-thumbs".into())
            .spawn(move || {
                run_worker(
                    path,
                    timestamps,
                    max_w,
                    max_h,
                    hw_decode,
                    worker_state,
                    worker_cancel.clone(),
                    cache.map(CacheTarget::tile),
                    video_mtime,
                    "video-tile-thumb",
                );
                worker_finished.store(true, Ordering::Release);
            }) {
            Ok(handle) => Some(handle),
            Err(e) => {
                // T33 (Codex R-VTT-001): spawn 失敗時に finished=false のままだと
                // tile overlay が unfinished 状態のまま 80ms ごとに repaint を要求し続ける
                // (= 永久再描画ループ)。finished=true でループを終わらせ、ログで原因を残す。
                // state は空のまま (= 全 slot None) で「タイル無し」表示になる。
                crate::logger::log(format!(
                    "[tile thumbnails] worker spawn failed: {e} — overlay will stay empty"
                ));
                finished.store(true, Ordering::Release);
                None
            }
        };

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

    /// 指定スロットだけを clone して返す。P キーで選択タイルを pin するような
    /// 単発操作では、全スロット snapshot よりこちらを使う。
    pub fn get(&self, idx: usize) -> Option<TileThumbnail> {
        self.state.lock().unwrap().get(idx).cloned().flatten()
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

/// `TileThumbnailWorker` の永続キャッシュ書き込み部分だけを使う one-shot worker。
///
/// Resume プレビュー用に「最後に表示した 1 フレーム」を後追い保存する用途。呼び出し側は
/// JoinHandle を保持しないが、通常の tile worker と同じ `run_worker` を使うため、既に
/// キャッシュ済みなら FFmpeg input を開かず即終了する。
pub fn spawn_resume_cache_warmup(
    path: PathBuf,
    target_secs: f64,
    max_w: u32,
    max_h: u32,
    cache: Arc<TileThumbCache>,
    video_mtime: i64,
) {
    if !target_secs.is_finite() || target_secs < 0.0 {
        return;
    }
    let state = Arc::new(Mutex::new(vec![None]));
    let cancel = Arc::new(AtomicBool::new(false));
    let _ = std::thread::Builder::new()
        .name("video-resume-thumb".into())
        .spawn(move || {
            run_worker(
                path,
                vec![target_secs],
                max_w,
                max_h,
                // Resume preview saves a single frame opportunistically; keep it on SW
                // decode so a background warmup does not pay HW setup cost or contend with playback.
                false,
                state,
                cancel,
                Some(CacheTarget::resume(cache)),
                video_mtime,
                "video-resume-thumb",
            );
        });
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
    hw_decode: bool,
    state: Arc<Mutex<Vec<Option<TileThumbnail>>>>,
    cancel: Arc<AtomicBool>,
    cache: Option<CacheTarget>,
    video_mtime: i64,
    log_label: &'static str,
) {
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
        let hits: Vec<Option<Vec<u8>>> = match c.write_kind {
            CacheWriteKind::Tile => c.cache.lookup_webp_batch(&path, &ts_ms, video_mtime, max_w),
            CacheWriteKind::Resume => {
                let resume_hit = c.cache.lookup_resume_webp(&path, video_mtime, max_w);
                ts_ms
                    .iter()
                    .map(|&ts| {
                        resume_hit
                            .as_ref()
                            .and_then(|(hit_ts, webp)| (*hit_ts == ts).then(|| webp.clone()))
                    })
                    .collect()
            }
        };
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
        crate::logger::log(format!("{log_label}: ffmpeg init failed: {e}"));
        return;
    }
    // 全スロット既にキャッシュから埋まっているなら ffmpeg open 自体スキップ。
    {
        let s = state.lock().unwrap();
        if s.iter().all(|t| t.is_some()) {
            return;
        }
    }
    let mut decoder: Option<TileThumbnailDecoder> = None;
    let mut hw_decode_failed = false;

    'timestamps: for (idx, &target_secs) in timestamps.iter().enumerate() {
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
        let thumb = loop {
            if decoder.is_none() {
                let use_hw = hw_decode && !hw_decode_failed;
                decoder = match TileThumbnailDecoder::open(&path, max_w, max_h, use_hw, log_label) {
                    Ok(decoder) => Some(decoder),
                    Err(e) => {
                        crate::logger::log(format!("{log_label}: decoder open failed: {e}"));
                        return;
                    }
                };
            }
            let decode_result = decoder
                .as_mut()
                .expect("decoder opened above")
                .decode_thumbnail(target_secs, &cancel, log_label);
            match decode_result {
                Ok(Some(thumb)) => break Some(thumb),
                Ok(None) => break None,
                Err(e)
                    if decoder
                        .as_ref()
                        .is_some_and(TileThumbnailDecoder::hw_decode_active) =>
                {
                    crate::logger::log(format!(
                        "{log_label}: HW decode failed; retrying with SW: {e}"
                    ));
                    hw_decode_failed = true;
                    decoder = None;
                    continue;
                }
                Err(e) => {
                    crate::logger::log(format!("{log_label}: decode failed: {e}"));
                    break None;
                }
            }
        };
        let Some(thumb) = thumb else {
            continue;
        };
        // Phase 6.D-2: 抽出済 RGBA を WebP に encode してキャッシュに書く
        // (失敗しても extraction 経路は止まらない)。Phase 8.C: 絶対 PTS キー化。
        if let Some(c) = cache.as_ref() {
            let encoder =
                webp::Encoder::from_rgba(thumb.rgba.as_slice(), thumb.width, thumb.height);
            // q=70: グリッドサムネと同等品位、サイズ優先
            let webp_bytes = encoder.encode(70.0).to_vec();
            let timestamp_ms = (target_secs * 1000.0).round() as i64;
            let store_result = match c.write_kind {
                CacheWriteKind::Tile => c.cache.store_webp(
                    &path,
                    max_w,
                    timestamp_ms,
                    video_mtime,
                    thumb.height,
                    &webp_bytes,
                ),
                CacheWriteKind::Resume => c.cache.store_resume_webp(
                    &path,
                    max_w,
                    timestamp_ms,
                    video_mtime,
                    thumb.height,
                    &webp_bytes,
                ),
            };
            if let Err(e) = store_result {
                crate::logger::log(format!("{log_label}: cache store failed: {e}"));
            }
        }

        if let Ok(mut s) = state.lock() {
            if idx < s.len() {
                s[idx] = Some(thumb);
            }
        }
        if cancel.load(Ordering::Acquire) {
            break 'timestamps;
        }
    }
}

struct TileThumbnailDecoder {
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

impl TileThumbnailDecoder {
    fn open(
        path: &std::path::Path,
        max_w: u32,
        max_h: u32,
        hw_preferred: bool,
        log_label: &str,
    ) -> Result<Self, String> {
        use ffmpeg::media::Type as MediaType;
        use ffmpeg_the_third as ffmpeg;

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
            log_label,
        )?;
        let src_w = decoder.width();
        let src_h = decoder.height();
        let (dst_w, dst_h) = fit_within(src_w, src_h, max_w, max_h);
        crate::logger::log(format!(
            "{log_label}: decoder ready codec={} decoder={} decode_path={} d3d11va_supported={} d3d11va_config={} src_size={}x{} dst_size={}x{}",
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

    fn decode_thumbnail(
        &mut self,
        target_secs: f64,
        cancel: &AtomicBool,
        log_label: &str,
    ) -> Result<Option<TileThumbnail>, String> {
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
            return Ok(None);
        }
        self.decoder.decoder_mut().flush();

        let mut got_frame: Option<Video> = None;
        let mut last_frame: Option<Video> = None;
        let hw_decode_active = self.hw_decode_active();
        // backward seek 後の keyframe から target_secs に到達する frame まで decode
        // し続ける。decode 数に上限は設けない: 長い GOP (実測 5.5s ≈ 165 frame の
        // 動画あり) でも必ず target のフレームを採用するため。上限を置くと GOP 長に
        // よってサムネが実位置からずれる。worker は cancel フラグを 1 パケットごとに
        // 確認するので、別 interval / 動画への切替時は自然終了する。
        for item in self.input.packets() {
            if cancel.load(Ordering::Acquire) {
                return Ok(None);
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
                // 再生デコーダと同じ best-effort timestamp を使う。PTS 欠落系の
                // AVI/ASF/古い DivX で `frame.pts()` が None になり、判定が壊れて
                // 全タイルが EOF まで走るのを防ぐ。timestamp が全く取れない壊れた
                // ストリームでは seek 直後の最初の frame をそのまま採用する。
                let Some(ts) = crate::video::decoder::video_frame_timestamp(&frame) else {
                    got_frame = Some(frame);
                    break;
                };
                let pts_secs = ts as f64 * self.tb_num / self.tb_den;
                // クリックで target_secs にシークしたとき表示される frame と一致
                // させるため「target_secs 以降の最初の frame」を採用する。以前は
                // `target_secs - 0.5` で 0.5s 手前の frame を拾っていた。
                if pts_secs >= target_secs {
                    got_frame = Some(frame);
                    break;
                }
                // target 未到達の frame は last_frame として保持。動画末尾付近の
                // タイムスタンプで target が最終フレームの pts を超えるケースの
                // fallback に使う。
                last_frame = Some(frame);
                frame = Video::empty();
            }
            if got_frame.is_some() {
                break;
            }
        }
        let Some(frame) = got_frame.or(last_frame) else {
            return Ok(None);
        };
        // HW (D3D11) frame は SW download してから scaler に渡す。SW frame はそのまま。
        let mut sw_holder: Option<Video> = None;
        let frame_for_scaler =
            crate::video::swscale_helpers::prepare_frame_for_swscale(&frame, &mut sw_holder)
                .map_err(|e| e.to_string())?;
        // `decoder.format()` ベースの事前 scaler 構築は HW accel attach 時に
        // `Pixel::D3D11` を返して swscale `av_assert0` → `abort()` を踏むため、
        // **最初の frame を取った後の `frame.format()` で scaler を lazy 構築** する。
        let cur_src_fmt = frame_for_scaler.format();
        if self.scaler.is_none() || self.scaler_src_fmt != Some(cur_src_fmt) {
            crate::logger::log(format!(
                "{log_label}: -> ScaleContext::get src_fmt={cur_src_fmt:?} src_size={}x{} dst_size={}x{}",
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
            crate::logger::log(format!("{log_label}: <- ScaleContext::get ok"));
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

        Ok(Some(TileThumbnail {
            pts_secs: target_secs,
            width: self.dst_w,
            height: self.dst_h,
            rgba: Arc::new(buf),
        }))
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
