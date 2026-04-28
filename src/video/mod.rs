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
    /// 直近に表示したフレームの PTS (秒)。デバッグ・HUD 用。
    last_displayed_pts: f64,
    /// open 失敗 / DLL ロード失敗のメッセージ。Some なら UI は赤字エラー表示する。
    error: Option<String>,
    /// シーク先サムネ抽出ワーカー。Drop で停止する。
    thumb_worker: Option<ThumbnailWorker>,
}

impl VideoPlayer {
    /// 新しい VideoPlayer を作る。FFmpeg DLL のロードはここで行う (冪等)。
    /// ファイルオープン自体はワーカースレッド内で非同期に行うので、UI スレッドは
    /// ブロックされない。
    ///
    /// `initial_volume` は 0.0-1.0。
    pub fn open(path: PathBuf, initial_volume: f64, autoplay: bool) -> Self {
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
                last_displayed_pts: 0.0,
                error: Some(format!("FFmpeg DLL のロードに失敗しました: {e}")),
                thumb_worker: None,
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
            last_displayed_pts: 0.0,
            error: None,
            thumb_worker,
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
        self.clock.set_playing(!self.clock.is_playing());
    }

    pub fn set_playing(&self, p: bool) {
        self.clock.set_playing(p);
    }

    /// 絶対シーク (シークバークリック等)。`direction = 0` で `..target` のキーフレーム
    /// にスナップ。target は `[0, duration - 0.1s)` にクランプされる。
    pub fn seek(&self, target_secs: f64) {
        let clamped = self.clamp_seek_target(target_secs);
        self.clock.request_seek(clamped, 0);
    }

    /// 相対シーク。`delta_secs > 0` なら前方 (`target..` のキーフレーム = preroll なし)、
    /// `delta_secs < 0` なら後方 (`..target` のキーフレーム + preroll で target に進む)。
    /// target は `[0, duration - 0.1s)` にクランプされる (Codex P2 指摘:
    /// EOF 越え seek でフレームが届かず override が永久 stuck になるのを回避)。
    pub fn seek_relative(&self, delta_secs: f64) {
        let cur = self.position();
        let raw = (cur + delta_secs).max(0.0);
        let target = self.clamp_seek_target(raw);
        let direction: i8 = if delta_secs > 0.0 { 1 } else { -1 };
        self.clock.request_seek(target, direction);
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

        // 動画フレームを必要なだけ pull
        // - PTS が now より小さければ「過ぎている」 → 上書きしながら捨て進む (最後の 1 枚を表示)
        // - PTS が now より大きければ「未来」 → そのフレームを採用し、再描画予約
        let mut latest_renderable: Option<VideoFrame> = None;
        let mut next_due: Option<std::time::Duration> = None;

        let clock_serial = self.clock.current_seek_serial();
        loop {
            match self.decode.video_rx.try_recv() {
                Ok(frame) => {
                    // 古い seek 世代のフレーム (pre-seek) は捨てる。
                    // decoder 側でも target 前を drop しているが、bounded(4) channel に
                    // 既に積まれていた pre-seek フレームが受信される可能性がある。
                    if frame.seek_serial < clock_serial {
                        continue;
                    }
                    if frame.pts_secs <= now + 0.005 {
                        // 表示時刻に達した。次のフレームも見て更に進むか試す
                        latest_renderable = Some(frame);
                        continue;
                    } else {
                        // 未来のフレーム — 採用して、次回再描画を予約
                        let until = (frame.pts_secs - now).max(0.001);
                        latest_renderable = Some(frame);
                        next_due = Some(std::time::Duration::from_secs_f64(until));
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    // 動画チャネル切断 = デコーダースレッドが EOF に達した。
                    // 音声側はまだ pump バッファに数百ms 分残っている可能性があるため、
                    // ここで即 set_playing(false) すると音声 PTS が凍結して
                    // シークバーが duration まで届かないまま止まって見える
                    // (ユーザー報告: 「シークバーが右端まで進む前に停止」)。
                    //
                    // 代わりに duration 位置に PTS をジャンプさせてから停止する:
                    // 視覚的には「最後まで再生 → 停止」の自然な動作に見える。
                    if let Some(info) = &self.info {
                        if info.duration_secs > 0.0 {
                            self.clock.set_audio_pts(info.duration_secs);
                        }
                    }
                    self.clock.set_playing(false);
                    break;
                }
            }
        }

        // 最新フレームをテクスチャに反映
        if let Some(frame) = latest_renderable {
            let color = ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.bgra,
            );
            self.last_displayed_pts = frame.pts_secs;
            // override は frame.pts が override target 近傍のときだけ解除する。
            // backward seek が外れて pts ≈ 元位置のフレームが新世代 serial で来た
            // 場合に override を消すと、シークバーが target → 元位置にスナップバック
            // する (= 「← シークが効かない」現象の本質)。target 近傍チェックを
            // 入れて「シークが物理的に成功した」ときだけ通常クロックに戻す。
            let now_after = self.clock.now_secs();
            if (frame.pts_secs - now_after).abs() <= clock::SEEK_TARGET_TOLERANCE_SECS {
                self.clock.clear_seek_target_override(frame.seek_serial);
            }
            match self.texture.as_mut() {
                Some(tex) => {
                    tex.set(color, TextureOptions::LINEAR);
                }
                None => {
                    let label = format!("video:{}", self.path.display());
                    self.texture = Some(ctx.load_texture(label, color, TextureOptions::LINEAR));
                }
            }
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

    pub fn last_displayed_pts(&self) -> f64 {
        self.last_displayed_pts
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
        // audio output は drop で停止
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
