//! 動画インライン再生 (FFmpeg LGPL DLL バックエンド)。
//!
//! `GridItem::Video` をフルスクリーンで開いたときに [`VideoPlayer`] を生成し、
//! 1 動画 = 1 プレイヤーとして所有する。プレイヤーは内部に:
//!
//! - **デコーダワーカー** (`std::thread`、bounded mpsc で UI に video/audio フレーム送出)
//! - **音声出力** (`cpal` Stream)
//! - **AV マスタークロック** ([`clock::AvClock`])
//!
//! を持つ。UI スレッドは [`VideoPlayer::tick`] を毎フレーム呼んで、再生位置に応じた
//! 動画フレームを GPU テクスチャ ([`egui::TextureHandle`]) に in-place で書き込む。
//!
//! ## 配布要件
//! `vendor/ffmpeg/bin/*.dll` (BtbN LGPL shared build) を `include_bytes!` で
//! exe に埋め込み、初回起動時に `%APPDATA%/mimageviewer/ffmpeg/` へ展開する
//! ([`ffmpeg_loader::init`])。VC++ 再頒布可能パッケージ非依存。
//!
//! ## ライセンス
//! FFmpeg LGPLv2.1 build。動的リンク + ソフトウェア情報への通知 + ソース提供
//! (mikage.to に tarball 配置) で MIT ライセンスの mIV と共存可能。詳細は
//! CLAUDE.md の「FFmpeg ライセンス対応」節を参照。

pub mod audio;
pub mod clock;
pub mod decoder;
pub mod ffmpeg_loader;
pub mod thumbnail;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use egui::{ColorImage, TextureHandle, TextureOptions};

use clock::AvClock;
use decoder::{DecodeHandles, VideoFrame, VideoInfo};
use thumbnail::{Thumbnail, ThumbnailWorker};

pub struct VideoPlayer {
    path: PathBuf,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    decode: DecodeHandles,
    /// 保持目的のフィールド (Drop で cpal Stream が停止する)。読み取りはしない。
    #[allow(dead_code)]
    audio: Option<audio::AudioOutput>,
    info: Option<VideoInfo>,
    /// 最新フレームのテクスチャ。最初の有効フレーム到着時に作成、その後は in-place set。
    texture: Option<TextureHandle>,
    /// open 失敗 / DLL ロード失敗のメッセージ。Some なら UI は赤字エラー表示する。
    error: Option<String>,
    /// シーク先サムネ抽出ワーカー。Drop で停止する。
    thumb_worker: Option<ThumbnailWorker>,
    /// 未来フレーム (pts > clock now) のキュー。channel から pull した順に末尾に push、
    /// front から `pts <= now` のものを取り出して表示。FIFO 連続性を保つことで
    /// 高 fps コンテンツでも display が channel head の far-future にジャンプしない。
    future_frames: std::collections::VecDeque<VideoFrame>,
    /// 起動時 resume 用の保留シーク target (秒)。info 到着後に 1 度だけ実行する。
    pending_resume_secs: Option<f64>,
    /// 直前 tick で観測した seek_serial。新世代に変わったら future_frames を一掃する。
    last_seen_seek_serial: u64,
}

/// `future_frames` キューの最大長。channel(8) と同じ値。
/// 1080p RGBA で 8 * ~8MB = 64MB。decoder pacing とチャネル背圧で実質
/// この上限に達することは稀。
const MAX_RENDER_QUEUE: usize = 8;

impl VideoPlayer {
    /// 新しい VideoPlayer を作る。FFmpeg DLL のロードはここで行う (冪等)。
    /// ファイルオープン自体はワーカースレッド内で非同期に行うので、UI スレッドは
    /// ブロックされない。
    ///
    /// `initial_volume` は 0.0-1.0。
    /// `resume_secs` を指定すると、最初の動画情報受領後に自動的にその位置へシークする。
    pub fn open(
        path: PathBuf,
        initial_volume: f64,
        autoplay: bool,
        resume_secs: Option<f64>,
    ) -> Self {
        // FFmpeg DLL ロード (1 回目のみ実時間の I/O。以降は OnceLock で即返り)
        if let Err(e) = ffmpeg_loader::init() {
            return Self {
                path,
                clock: Arc::new(AvClock::new(initial_volume)),
                cancel: Arc::new(AtomicBool::new(true)),
                decode: dummy_decode_handles(),
                audio: None,
                info: None,
                texture: None,
                error: Some(format!("FFmpeg DLL のロードに失敗しました: {e}")),
                thumb_worker: None,
                future_frames: std::collections::VecDeque::new(),
                pending_resume_secs: None,
                last_seen_seek_serial: 0,
            };
        }

        let clock = Arc::new(AvClock::new(initial_volume));
        let cancel = Arc::new(AtomicBool::new(false));

        // 音声出力デバイスのサンプルレートを先に取得し、デコーダーの swresample
        // 出力レートと cpal ストリームレートを **同じ値** に揃える。
        // 揃えないとデバイスが期待するレートと違うレートのサンプルが届き、
        // 「ピッチが下がってスロー再生」になる (ユーザー報告のバグ)。
        // デバイスが取れなければ 48kHz をフォールバック。
        let target_rate = audio::default_output_sample_rate().unwrap_or(48_000);

        let decode = decoder::spawn(path.clone(), clock.clone(), cancel.clone(), target_rate);

        // 音声出力起動。失敗してもプレイヤーは生きる (映像のみ再生)。
        // 音声を二重に消費するので、decoder の audio_rx を audio.start に渡す。
        // ここで decode.audio_rx を取り出す必要があるので構造体を分解する。
        let DecodeHandles { video_rx, audio_rx, info_rx } = decode;
        let audio = match audio::start(audio_rx, clock.clone()) {
            Ok(a) => Some(a),
            Err(e) => {
                crate::logger::log(format!("audio output failed: {e} (映像のみ再生)"));
                // audio 出力失敗 → fallback wall clock で進行させる
                clock.mark_audio_inactive();
                None
            }
        };

        clock.set_playing(autoplay);

        // シーク先サムネ抽出ワーカー (失敗してもメイン再生は続行)
        let thumb_worker = Some(ThumbnailWorker::spawn(path.clone()));

        Self {
            path,
            clock,
            cancel,
            decode: DecodeHandles {
                video_rx,
                audio_rx: dummy_audio_rx(),
                info_rx,
            },
            audio,
            info: None,
            texture: None,
            error: None,
            thumb_worker,
            future_frames: std::collections::VecDeque::new(),
            pending_resume_secs: resume_secs,
            last_seen_seek_serial: 0,
        }
    }

    /// シークホバー位置のサムネを要求する。debounce はワーカー側 (drain) で実施。
    pub fn request_seek_thumbnail(&self, target_secs: f64) {
        if let Some(w) = &self.thumb_worker {
            w.request(target_secs);
        }
    }

    /// 直近キャッシュから target_secs に最も近いサムネを取り出す。
    pub fn nearest_seek_thumbnail(&self, target_secs: f64) -> Option<Thumbnail> {
        self.thumb_worker.as_ref().and_then(|w| w.nearest(target_secs))
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn info(&self) -> Option<&VideoInfo> {
        self.info.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_playing(&self) -> bool {
        self.clock.is_playing()
    }

    pub fn toggle_play(&self) {
        // EOF で停止中に Space を押されたら 0 から再生し直す (replay)。
        // 通常の再生中は単純トグル。
        if !self.clock.is_playing() && self.clock.is_eof_reached() {
            self.clock.request_seek(0.0, 0);
            self.clock.set_playing(true);
            return;
        }
        self.clock.set_playing(!self.clock.is_playing());
    }

    pub fn set_playing(&self, p: bool) {
        self.clock.set_playing(p);
    }

    /// 絶対シーク (シークバークリック等)。`direction = 0` で `..target` のキーフレーム
    /// にスナップ。target は `[0, duration - 0.1s)` にクランプされる。
    /// 一時停止中なら自動的に再生再開する (post-EOF / pause からの seek を
    /// ユーザー操作 1 回で完結させる)。
    pub fn seek(&self, target_secs: f64) {
        let clamped = self.clamp_seek_target(target_secs);
        self.clock.request_seek(clamped, 0);
        if !self.clock.is_playing() {
            self.clock.set_playing(true);
        }
    }

    /// 相対シーク。`delta_secs > 0` なら前方 (`target..` のキーフレーム = preroll なし)、
    /// `delta_secs < 0` なら後方 (`..target` のキーフレーム + preroll で target に進む)。
    /// 一時停止中なら自動的に再生再開する。
    pub fn seek_relative(&self, delta_secs: f64) {
        let cur = self.position();
        let raw = (cur + delta_secs).max(0.0);
        let target = self.clamp_seek_target(raw);
        let direction: i8 = if delta_secs > 0.0 { 1 } else { -1 };
        self.clock.request_seek(target, direction);
        if !self.clock.is_playing() {
            self.clock.set_playing(true);
        }
    }

    /// シーク target を `[0, duration - 0.1s)` にクランプする。duration が
    /// 不明な (= info まだ来ていない) 場合は target をそのまま通す。
    fn clamp_seek_target(&self, target_secs: f64) -> f64 {
        let lower = target_secs.max(0.0);
        if let Some(info) = &self.info {
            if info.duration_secs > 0.2 {
                return lower.min(info.duration_secs - 0.1);
            }
        }
        lower
    }

    pub fn position(&self) -> f64 {
        self.clock.now_secs()
    }

    pub fn duration(&self) -> f64 {
        self.info.as_ref().map(|i| i.duration_secs).unwrap_or(0.0)
    }

    pub fn volume(&self) -> f64 {
        self.clock.volume()
    }

    pub fn set_volume(&self, v: f64) {
        self.clock.set_volume(v);
    }

    pub fn is_muted(&self) -> bool {
        self.clock.is_muted()
    }

    pub fn set_muted(&self, m: bool) {
        self.clock.set_muted(m);
    }

    /// UI スレッドが毎フレーム呼ぶ。新しい info / video frame があれば反映する。
    /// 戻り値は次回再描画推奨時刻 (秒) — `ctx.request_repaint_after` に渡す目安。
    pub fn tick(&mut self, ctx: &egui::Context) -> Option<std::time::Duration> {
        // info を取り込む
        if self.info.is_none() {
            if let Ok(result) = self.decode.info_rx.try_recv() {
                match result {
                    Ok(info) => {
                        // resume 指定があれば最初の info 到着時に 1 度だけ実行。
                        // 末尾近く (残り 5 秒以下) なら 0 から再生 (= 完走済みと見なす)。
                        // 保存側 (`save_video_resume_position`) と同じ閾値で gate する。
                        if let Some(resume) = self.pending_resume_secs.take() {
                            let dur = info.duration_secs;
                            let near_end = dur > 0.0
                                && resume >= dur - crate::app::VIDEO_RESUME_END_GUARD_SECS;
                            if resume >= crate::app::VIDEO_RESUME_MIN_POSITION_SECS
                                && !near_end
                            {
                                self.clock.request_seek(resume, 0);
                            }
                        }
                        self.info = Some(info);
                    }
                    Err(e) => {
                        self.error = Some(e);
                        return None;
                    }
                }
            }
        }

        if self.error.is_some() {
            return None;
        }

        // クロックの今時刻
        let now = self.clock.now_secs();

        // ── 動画フレーム取得・表示判定 ──
        //
        // 設計 (Codex 指摘の「FIFO 連続性」を保証):
        //   1. video_rx から取得可能なフレームを `future_frames` キューに push
        //      (キュー上限まで)。channel から取り出したフレームは drop しない。
        //   2. キュー先頭から「pts <= now + 許容差」のものを順に latest_renderable
        //      に取り出す。最後に残った 1 枚を表示。
        //   3. 最初に出会う「未来フレーム」(pts > now + 許容差) で停止し、
        //      next_due = pts - now で次 tick を予約。キューに残す。
        let mut latest_renderable: Option<VideoFrame> = None;
        let mut next_due: Option<std::time::Duration> = None;
        let mut pulled = 0u64;
        let mut dropped_old_serial = 0u64;
        let mut dropped_past = 0u64;

        let clock_serial = self.clock.current_seek_serial();
        let lead_tol = clock::DISPLAY_LEAD_TOLERANCE_SECS;

        // seek_serial が前回 tick から変わっていれば、queue 全部一掃 (Codex 助言)。
        // 個別の `seek_serial < clock_serial` 1 ずつ pop だと N tick かかるが、
        // 一括で消すことで post-seek 反応が速くなる。
        if self.last_seen_seek_serial != clock_serial {
            self.future_frames.clear();
            self.last_seen_seek_serial = clock_serial;
        }

        // Step 1: video_rx を future_frames に drain (上限まで)
        // EOF 後は decoder が wait ループに入るため channel は disconnect しない。
        // EOF 検出は clock.is_eof_reached() で行う (= decoder thread alive のまま
        // post-EOF seek が可能)。
        while self.future_frames.len() < MAX_RENDER_QUEUE {
            match self.decode.video_rx.try_recv() {
                Ok(frame) => {
                    pulled += 1;
                    self.future_frames.push_back(frame);
                }
                Err(_) => break,
            }
        }

        // Step 2: 先頭から displayable なものを順に取る
        while let Some(front) = self.future_frames.front() {
            if front.seek_serial < clock_serial {
                self.future_frames.pop_front();
                dropped_old_serial += 1;
                continue;
            }
            if front.pts_secs <= now + lead_tol {
                let frame = self.future_frames.pop_front().unwrap();
                if latest_renderable.is_some() {
                    dropped_past += 1;
                }
                latest_renderable = Some(frame);
                continue;
            }
            // 最初の真の未来フレーム → そのまま残し、次 tick を予約
            let until = (front.pts_secs - now).max(0.001);
            next_due = Some(std::time::Duration::from_secs_f64(until));
            break;
        }

        // EOF 処理: clock.is_eof_reached() (= decoder が EOF wait に入った)
        // + queue 空 + 今 tick 表示なし + 進行中の seek なし → 本当に最後と判定。
        // 「seek 進行中」は seek_target_override が立っている時。
        let seek_in_flight = self.clock.is_seeking();
        if self.clock.is_eof_reached()
            && self.future_frames.is_empty()
            && latest_renderable.is_none()
            && !seek_in_flight
            && self.is_playing()
        {
            // 末端到達 → duration 位置に進めて停止 (シークバー右端を確実にする)。
            if let Some(info) = &self.info {
                if info.duration_secs > 0.0 {
                    self.clock.set_position_at_eof(info.duration_secs);
                }
            }
            self.clock.set_playing(false);
        }

        // 最新フレームをテクスチャに反映
        let mut displayed_pts: Option<f64> = None;
        let mut upload_ms: f64 = 0.0;
        if let Some(frame) = latest_renderable {
            let pts_for_log = frame.pts_secs;
            let color = ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.bgra,
            );
            // override は frame.pts が override target 近傍のときだけ解除する。
            // backward seek が外れて pts ≈ 元位置のフレームが新世代 serial で来た
            // 場合に override を消すと、シークバーが target → 元位置にスナップバック
            // する (= 「← シークが効かない」現象の本質)。target 近傍チェックを
            // 入れて「シークが物理的に成功した」ときだけ通常クロックに戻す。
            let now_after = self.clock.now_secs();
            if (frame.pts_secs - now_after).abs() <= clock::SEEK_TARGET_TOLERANCE_SECS {
                self.clock.clear_seek_target_override(frame.seek_serial);
            }
            let upload_t0 = std::time::Instant::now();
            match self.texture.as_mut() {
                Some(tex) => {
                    tex.set(color, TextureOptions::LINEAR);
                }
                None => {
                    let label = format!("video:{}", self.path.display());
                    self.texture = Some(ctx.load_texture(label, color, TextureOptions::LINEAR));
                }
            }
            upload_ms = upload_t0.elapsed().as_secs_f64() * 1000.0;
            displayed_pts = Some(pts_for_log);
        }

        // perf: tick 1 回ごとの状況を記録 (channel 詰まり / 描画詰まりの可視化)
        if crate::perf::is_enabled()
            && (pulled > 0 || dropped_old_serial > 0 || dropped_past > 0 || displayed_pts.is_some())
        {
            let lateness_ms = displayed_pts.map(|p| (now - p) * 1000.0).unwrap_or(0.0);
            crate::perf::event(
                "video",
                "tick",
                None,
                0,
                &[
                    ("now", serde_json::Value::from(now)),
                    ("displayed_pts", serde_json::Value::from(displayed_pts.unwrap_or(f64::NAN))),
                    ("lateness_ms", serde_json::Value::from(lateness_ms)),
                    ("pulled", serde_json::Value::from(pulled)),
                    ("dropped_old_serial", serde_json::Value::from(dropped_old_serial)),
                    ("dropped_past", serde_json::Value::from(dropped_past)),
                    ("upload_ms", serde_json::Value::from(upload_ms)),
                ],
            );
        }

        // 再生中なら次フレームに合わせて再描画
        if self.is_playing() {
            // 上のループで未来フレームが見えていればその時刻、そうでなければ
            // 30fps 想定で 33ms 後を目安にする
            Some(next_due.unwrap_or_else(|| std::time::Duration::from_millis(33)))
        } else {
            None
        }
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    /// Drop 前に明示的に呼ぶと、AudioOutput の停止 (cpal Stream pause/drop +
    /// pump スレッド join) を Drop 順より早く実行できる。
    ///
    /// マウスホイール等で次の動画に瞬時に切り替わる時、 fs_cache 上の旧 entry の
    /// Drop が「フィールド宣言順」(audio が後ろのため最後) で行われると、cpal stream
    /// が止まるまでの数百 ms のあいだ前動画の音声が hardware buffer から流れ続ける
    /// 現象が観測される (Codex 指摘)。先に shutdown() を呼ぶことで、entry を
    /// fs_cache から消す瞬間に音声が止まる。
    pub fn shutdown(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
        // AudioOutput を先に drop して cpal stream を止める。
        // Drop で pump も join される。
        self.audio.take();
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // shutdown() が事前に呼ばれていなければここで stop。
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
        self.audio.take();
        // decoder thread は cancel フラグを見て終了
    }
}

fn dummy_decode_handles() -> DecodeHandles {
    let (_, video_rx) = crossbeam_channel::bounded(0);
    let (_, audio_rx) = crossbeam_channel::bounded(0);
    let (_, info_rx) = crossbeam_channel::bounded(0);
    DecodeHandles {
        video_rx,
        audio_rx,
        info_rx,
    }
}

fn dummy_audio_rx() -> crossbeam_channel::Receiver<decoder::AudioFrame> {
    let (_, rx) = crossbeam_channel::bounded(0);
    rx
}
