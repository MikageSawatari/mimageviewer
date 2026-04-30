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
pub mod engine;
pub mod ffmpeg_loader;
#[cfg(windows)]
pub mod gpu_renderer;
pub mod thumbnail;
pub mod tile_thumb_cache;
pub mod tile_thumbnails;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use egui::{ColorImage, TextureHandle, TextureOptions};

use clock::AvClock;
use decoder::{DecodeHandles, VideoFrame, VideoInfo};
use thumbnail::{Thumbnail, ThumbnailWorker};

use std::sync::Mutex;

use engine::actor::{EngineActor, OpenOptions};
use engine::EngineEvent;

pub struct VideoPlayer {
    path: PathBuf,
    clock: Arc<AvClock>,
    /// Phase 3b で導入された state machine actor。Phase 3c+ で decoder/audio events を
    /// 流し込み、Phase 3d で AvClock の状態系メソッドを EngineActor 主導に置き換える。
    /// Phase 3b 時点では `begin_loading()` を呼んだ状態で保持されるが、actor の state
    /// 遷移は Phase 3c で配線完了後に有効化する (= 現在は AvClock が引き続き source of truth)。
    #[allow(dead_code)]
    engine: Arc<Mutex<EngineActor>>,
    /// Phase 3c で追加。decoder/audio thread から push される events を tick で
    /// drain して engine に dispatch する。capacity 64 (= burst tolerance、
    /// drop 不可なので unbounded 寄りの bounded)。
    engine_event_rx: crossbeam_channel::Receiver<EngineEvent>,
    /// 同 channel の sender (decoder/audio に clone して渡す)。
    /// VideoPlayer 自身が `tick` 内で UI thread からも push する経路を持つために保持。
    #[allow(dead_code)]
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    /// `InfoReceived` を engine に 1 度だけ流すためのフラグ。tick で info_rx から
    /// info を取り出した直後に発火し、以降は false で抑止する。
    info_event_emitted: bool,
    /// `FirstFrameReady` を engine に 1 度だけ流すためのフラグ (= 現 epoch 内)。
    /// `notify_seek_completed` 経路 (= seek 後再 buffering) では engine 側で
    /// epoch++ するので、tick 側では「engine.current_seek_epoch を読み取って
    /// 自分の last_seen_epoch と比べる」方式で再発火する。
    first_frame_event_last_epoch: Option<engine::state::SeekEpoch>,
    /// 表示したフレーム数の累積カウンタ。tick で latest_renderable を採用するたびに
    /// +1。GPU/CPU 両経路で更新するので、UI 側の perf overlay が経路に依存せず
    /// 「新フレーム到着」を検知できる (Phase 8.I 修正)。
    displayed_frame_seq: AtomicU64,
    /// 「skip された frame」の累積カウンタ。次の 2 経路で +1:
    ///   - decoder の video_tx try_send が Full → 送信できず捨てた (Arc 共有 atomic)
    ///   - tick で latest_renderable を上書き = 古い候補は dropped_past として
    ///     表示前に捨てた (= UI 側 skip)
    /// perf overlay はこの累積を per-sample で diff して赤縦線を出す。
    skipped_frame_count: Arc<AtomicU64>,
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
    /// EOF 到達時に先頭から再生し直すか (= 設定の video_loop)。App が
    /// `set_loop_enabled` で更新する。
    loop_enabled: AtomicBool,
    /// 現在進行中のシークが開始された壁時計時刻。シーク中は UI tick が短周期で
    /// repaint を予約してデコーダ完成を polling 待ちする。長引いたら back off する。
    /// シーク完了 (override が 1 度クリア) で None に戻す。
    seek_inflight_since: Option<std::time::Instant>,
    /// GPU 経路で最新表示フレームの **所有** (= D3d11Frame)。`ui_fullscreen` は
    /// `gpu_latest_info()` 経由で view-only 情報 (handle, dims) を得る。
    /// 次の GPU フレームが到着して置き換わるまで本フィールドが保持し、
    /// 置換時に旧 D3d11Frame::Drop で HANDLE を CloseHandle する (= UI が同 handle を
    /// 描画している期間は HANDLE が valid であることを保証、Codex P1 反映)。
    #[cfg(windows)]
    gpu_latest: Option<crate::video::gpu_renderer::D3d11Frame>,
}

/// `VideoPlayer::gpu_latest_info()` が返す view-only 情報。Copy なので
/// `FsFrameState` などの構造体に値で持たせて safe。HANDLE の寿命は
/// `VideoPlayer.gpu_latest` (= D3d11Frame) が保証する。
#[cfg(windows)]
#[derive(Clone, Copy)]
pub struct GpuLatestFrame {
    pub shared_handle: windows::Win32::Foundation::HANDLE,
    pub width: u32,
    pub height: u32,
    pub ten_bit: bool,
    /// このフレームの GPU 完了に対応する fence 値。wgpu 側で
    /// `ID3D12CommandQueue::Wait(fence, fence_value)` してから sample する。
    pub fence_value: u64,
    /// fence の NT shared handle (`GpuVideoDevice` 寿命中は同じ値)。
    /// wgpu 側で `OpenSharedHandle` するが、所有権は `GpuVideoDevice` が持っているので
    /// このフィールドからは close しない。
    pub fence_shared_handle: windows::Win32::Foundation::HANDLE,
    /// プロセス内ユニークな fence 世代 ID。wgpu 側のキャッシュキー (Codex P1)。
    pub fence_gen: u64,
}

// HANDLE は thread を渡って良い (D3d11Frame と同様の論理)。
#[cfg(windows)]
unsafe impl Send for GpuLatestFrame {}
#[cfg(windows)]
unsafe impl Sync for GpuLatestFrame {}

/// `future_frames` キューの最大長。decoder の `video_tx` (= 24) と揃える。
/// 1080p RGBA で 24 × ~8MB = 192MB 程度 (CPU 経路の上限)。GPU 経路では
/// 1 frame ≈ HANDLE+メタのみで実コストは無視できる。decoder の burst-stall
/// パターン (~400ms) + HDD random read (~100-300ms) を ~800ms buffer で
/// 吸収して UI tick の空振りを抑える (Phase 8.J)。
pub(crate) const MAX_RENDER_QUEUE: usize = 24;

impl VideoPlayer {
    /// 新しい VideoPlayer を作る。FFmpeg DLL のロードはここで行う (冪等)。
    /// ファイルオープン自体はワーカースレッド内で非同期に行うので、UI スレッドは
    /// ブロックされない。
    ///
    /// `initial_volume` は 0.0-1.0。
    /// `resume_secs` を指定すると、最初の動画情報受領後に自動的にその位置へシークする。
    /// `hw_decode` が true なら D3D11VA HW デコードを試行 (失敗時は SW にフォールバック)。
    pub fn open(
        path: PathBuf,
        initial_volume: f64,
        autoplay: bool,
        resume_secs: Option<f64>,
        hw_decode: bool,
        #[cfg(windows)] gpu_video_device: Option<
            std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
        >,
    ) -> Self {
        // FFmpeg DLL ロード (1 回目のみ実時間の I/O。以降は OnceLock で即返り)
        if let Err(e) = ffmpeg_loader::init() {
            // open 失敗時の dummy engine (Idle のまま)。実 decoder は起きないので、
            // begin_loading は呼ばない (= Phase 3+ で resume 適用も走らない)。
            let engine = Arc::new(Mutex::new(EngineActor::new(OpenOptions {
                initial_volume,
                autoplay,
                resume_secs,
                ..Default::default()
            })));
            let (engine_event_tx, engine_event_rx) = crossbeam_channel::bounded(64);
            return Self {
                path,
                clock: Arc::new(AvClock::new(initial_volume)),
                engine,
                engine_event_tx,
                engine_event_rx,
                info_event_emitted: false,
                first_frame_event_last_epoch: None,
                displayed_frame_seq: AtomicU64::new(0),
                skipped_frame_count: Arc::new(AtomicU64::new(0)),
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
                loop_enabled: AtomicBool::new(false),
                seek_inflight_since: None,
                #[cfg(windows)]
                gpu_latest: None,
            };
        }

        // engine event channel: decoder/audio thread が events を push、UI tick が
        // drain して engine.handle_*_event に dispatch する。capacity 64 (= 60fps の
        // ~1 秒分 + audio callback 数件のバッファ余地)。
        let (engine_event_tx, engine_event_rx) =
            crossbeam_channel::bounded::<EngineEvent>(64);

        let clock = Arc::new(AvClock::new(initial_volume));
        let cancel = Arc::new(AtomicBool::new(false));

        // EngineActor 構築。Phase 3b 時点では `engine` は `tick`/`apply_command` から
        // 触られておらず、AvClock が引き続き source of truth。Phase 3c 以降で
        // decoder/audio events 経路を配線したときに actor の state 機械が活性化する。
        let opts = OpenOptions {
            initial_volume,
            autoplay,
            resume_secs,
            loop_enabled: false, // VideoPlayer 側で個別管理 (Phase 3+ で統合予定)
            hw_decode,
        };
        let engine = Arc::new(Mutex::new(EngineActor::new(opts)));
        // begin_loading() を decoder::spawn の **前** に呼ぶ。これにより decoder
        // thread が起動した瞬間から `engine_state_handle` を `Loading` で観察できる
        // (= Idle を一瞬観察する race を排除、Codex Phase 3d P2 反映)。
        let engine_state_handle = {
            let mut g = engine.lock().unwrap();
            g.begin_loading();
            g.published_state_handle()
        };

        // 音声出力デバイスのサンプルレートを先に取得し、デコーダーの swresample
        // 出力レートと cpal ストリームレートを **同じ値** に揃える。
        // 揃えないとデバイスが期待するレートと違うレートのサンプルが届き、
        // 「ピッチが下がってスロー再生」になる (ユーザー報告のバグ)。
        // デバイスが取れなければ 48kHz をフォールバック。
        let target_rate = audio::default_output_sample_rate().unwrap_or(48_000);

        // perf overlay 用 skip counter を decoder スレッドと共有 (= dropped_full 時に
        // decoder 側から +1)。UI 側 dropped_past の +1 は VideoPlayer::tick 内で行う。
        let skipped_frame_count = Arc::new(AtomicU64::new(0));

        let decode = decoder::spawn(
            path.clone(),
            clock.clone(),
            cancel.clone(),
            target_rate,
            hw_decode,
            #[cfg(windows)]
            gpu_video_device,
            engine_state_handle,
            engine_event_tx.clone(),
            skipped_frame_count.clone(),
        );

        // 音声出力起動。失敗してもプレイヤーは生きる (映像のみ再生)。
        // 音声を二重に消費するので、decoder の audio_rx を audio.start に渡す。
        // ここで decode.audio_rx を取り出す必要があるので構造体を分解する。
        let DecodeHandles { video_rx, audio_rx, info_rx } = decode;
        let audio = match audio::start(audio_rx, clock.clone(), engine_event_tx.clone()) {
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
            engine,
            engine_event_tx,
            engine_event_rx,
            info_event_emitted: false,
            first_frame_event_last_epoch: None,
            displayed_frame_seq: AtomicU64::new(0),
            skipped_frame_count,
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
            loop_enabled: AtomicBool::new(false),
            seek_inflight_since: None,
            #[cfg(windows)]
            gpu_latest: None,
        }
    }

    /// Phase 3c: engine event channel から events を drain して engine actor に
    /// dispatch する。tick の冒頭で呼ぶ。
    /// 1 tick 内で全 events を処理する (= UI 60fps なら遅くても 16ms 内に decoder/
    /// audio events が actor に届く)。channel が full のときは decoder/audio 側で
    /// drop されるが、Phase 3c では非クリティカル events のみ流すので問題ない。
    fn drain_engine_events(&mut self) {
        let mut engine = self.engine.lock().unwrap();
        while let Ok(ev) = self.engine_event_rx.try_recv() {
            match ev {
                EngineEvent::Decoder(d) => engine.handle_decoder_event(d),
                EngineEvent::Audio(a) => engine.handle_audio_event(a),
            }
        }
    }

    /// Phase 3c: 表示済み frame の `FirstFrameReady` を engine に流す。
    /// 同 epoch 内では 1 度だけ発火する。新世代 (= seek 後) では再発火する。
    /// engine.current_seek_epoch を読み取って自分の last 値と比較。
    fn emit_first_frame_event(&mut self, pts: f64) {
        let cur_epoch = self.engine.lock().unwrap().current_seek_epoch();
        if self.first_frame_event_last_epoch == Some(cur_epoch) {
            return;
        }
        self.first_frame_event_last_epoch = Some(cur_epoch);
        let _ = self.engine_event_tx.try_send(EngineEvent::Decoder(
            engine::state::DecoderEvent::FirstFrameReady {
                epoch: cur_epoch,
                pts,
            },
        ));
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
            // engine 側にも seek を伝えて epoch を同期させる (Codex Phase 3d P2 反映)。
            // user 操作の seek は autoplay 強制 (Codex Phase 3e P2 反映: seek 後に
            // Paused にならないように)。
            let mut g = self.engine.lock().unwrap();
            g.apply_command(engine::actor::TransportCommand::Play);
            g.handle_seek_request(0.0);
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
        // engine の seek_epoch も進めて、AvClock seek_serial と同期させる
        // (Codex Phase 3d P2 反映)。user 操作 seek は autoplay 強制
        // (Codex Phase 3e P2 反映: AvClock 側で playing=true にしているため整合性)。
        let mut g = self.engine.lock().unwrap();
        g.apply_command(engine::actor::TransportCommand::Play);
        g.handle_seek_request(clamped);
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
        // user 操作 seek は autoplay 強制 (Codex Phase 3e P2 反映)。
        let mut g = self.engine.lock().unwrap();
        g.apply_command(engine::actor::TransportCommand::Play);
        g.handle_seek_request(target);
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

    /// ループ再生 ON/OFF を更新。App は毎 poll_video で settings 値を反映する。
    pub fn set_loop_enabled(&self, enabled: bool) {
        self.loop_enabled
            .store(enabled, std::sync::atomic::Ordering::Release);
    }

    /// UI スレッドが毎フレーム呼ぶ。新しい info / video frame があれば反映する。
    /// 戻り値は次回再描画推奨時刻 (秒) — `ctx.request_repaint_after` に渡す目安。
    pub fn tick(&mut self, ctx: &egui::Context) -> Option<std::time::Duration> {
        // Phase 3c: engine_event channel の drain を **tick の冒頭** で行う。
        // decoder/audio thread から push された events を engine.handle_*_event に
        // dispatch する。EngineActor は state machine のみ更新し、AvClock の挙動には
        // まだ影響しない (= Phase 3d までは AvClock が引き続き source of truth)。
        self.drain_engine_events();

        // info を取り込む
        if self.info.is_none() {
            if let Ok(result) = self.decode.info_rx.try_recv() {
                match result {
                    Ok(info) => {
                        // Phase 3c: engine にも InfoReceived event を流す。
                        // resume_secs は **AvClock 経由の旧経路** で処理し続け、
                        // engine 側でも resume_secs を OpenOptions で受領済みなので
                        // engine の InfoReceived ハンドラ内で並行処理される。
                        // (= 二重で seek が走らないよう、Phase 3d で旧経路を撤去する。)
                        if !self.info_event_emitted {
                            // Codex Phase 3d P1 反映: audio output 起動に失敗した場合
                            // (`self.audio.is_none()`) は has_audio=false で engine に
                            // 通知する。さもなくば engine が BufferReady を永久に待ち
                            // Buffering で固まる (= audio が決して再生されないため
                            // audio.rs から BufferReady が出ない)。
                            let has_audio_effective =
                                info.has_audio && self.audio.is_some();
                            let _ = self.engine_event_tx.try_send(EngineEvent::Decoder(
                                engine::state::DecoderEvent::InfoReceived {
                                    epoch: self
                                        .engine
                                        .lock()
                                        .unwrap()
                                        .current_seek_epoch(),
                                    duration_secs: info.duration_secs,
                                    has_audio: has_audio_effective,
                                },
                            ));
                            self.info_event_emitted = true;
                        }

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
                                // engine の epoch も同時に進めて AvClock と同期
                                // (Codex Phase 3d P2 反映: pre-info user seek 経路で
                                // ズレるリスクは pending_resume_secs.take() の他に
                                // 経路がないので、ここで二重 seek を許容しても重複
                                // epoch++ 1 回分の副作用のみで害はない)。
                                //
                                // **意図的に apply_command(Play) は呼ばない**:
                                // open-time の resume は user 操作ではなく自動復元
                                // なので、`OpenOptions.autoplay` (= 設定の
                                // video_autoplay) を尊重する。autoplay=false なら
                                // post-seek READY で Paused に遷移する設計。
                                // user 操作の seek/seek_relative/toggle_play は
                                // 別経路で apply_command(Play) を呼ぶ。
                                self.engine
                                    .lock()
                                    .unwrap()
                                    .handle_seek_request(resume);
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
        let seek_in_flight_for_display = self.clock.is_seeking();
        // seek 開始時刻トラッキング: 末尾 repaint 計算で「2 秒以上長引いたら 100ms に
        // back off」する (decoder 故障時に CPU 100% で polling し続ける事故防止)。
        if seek_in_flight_for_display && self.seek_inflight_since.is_none() {
            self.seek_inflight_since = Some(std::time::Instant::now());
        } else if !seek_in_flight_for_display {
            self.seek_inflight_since = None;
        }

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
            // post-seek 第一フレームは override target で now が凍結するため
            // pts チェックを免除して強制表示する (= UI が表示することで
            // clear_seek_target_override が呼ばれ、override がやっと外れる)。
            // is_seeking() を再確認 (audio が間に override をクリアした場合の stale 防止)。
            let force_display_seek = seek_in_flight_for_display
                && latest_renderable.is_none()
                && front.seek_serial == clock_serial
                && self.clock.is_seeking()
                && clock::pts_clears_seek_override(front.pts_secs, now);
            if force_display_seek || front.pts_secs <= now + lead_tol {
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
            if self
                .loop_enabled
                .load(std::sync::atomic::Ordering::Acquire)
            {
                // ループ再生 ON: 先頭にシークし続行 (= 設定の video_loop)。
                self.clock.request_seek(0.0, 0);
                self.clock.set_playing(true);
                // engine 側の epoch も同期 (= AvClock seek_serial と engine
                // current_seek_epoch の不整合を防ぐ、Codex Phase 3d P2 反映)。
                // loop 周回も autoplay 強制 (Codex Phase 3e P2 反映)。
                let mut g = self.engine.lock().unwrap();
                g.apply_command(engine::actor::TransportCommand::Play);
                g.handle_seek_request(0.0);
            } else {
                // 末端到達 → duration 位置に進めて停止 (シークバー右端を確実にする)。
                if let Some(info) = &self.info {
                    if info.duration_secs > 0.0 {
                        self.clock.set_position_at_eof(info.duration_secs);
                    }
                }
                self.clock.set_playing(false);
            }
        }

        // 最新フレームをテクスチャに反映
        let mut displayed_pts: Option<f64> = None;
        let mut upload_ms: f64 = 0.0;
        if let Some(frame) = latest_renderable {
            let pts_for_log = frame.pts_secs;
            // GPU フレームは `gpu_latest` に **所有権ごと** 引き取り、UI は handle を
            // view で参照する。これで前フレームの HANDLE が次フレーム到着まで保持され、
            // 描画中に CloseHandle される race を防ぐ。
            #[cfg(windows)]
            if matches!(frame.data, decoder::VideoFrameData::Gpu(_)) {
                let pts = frame.pts_secs;
                let serial = frame.seek_serial;
                let was_none = self.gpu_latest.is_none();
                if let decoder::VideoFrameData::Gpu(d3d) = frame.data {
                    self.gpu_latest = Some(d3d);
                }
                if was_none {
                    crate::logger::log(format!(
                        "VideoPlayer::tick: GPU frame received and stored in gpu_latest \
                         (pts={pts:.3}, serial={serial})"
                    ));
                }
                let now_for_clear = self.clock.now_secs();
                if clock::pts_clears_seek_override(pts, now_for_clear) {
                    if self.clock.is_audio_active() {
                        self.clock.set_audio_pts(pts);
                    } else {
                        self.clock.set_fallback_anchor(pts);
                    }
                    self.clock.clear_seek_target_override(serial);
                } else if self.clock.is_seeking() && crate::perf::is_enabled() {
                    // override 中で frame pts < target - 0.75 (= preroll 不足で
                    // 解除条件を満たさない経路) を perflog から特定するための診断。
                    crate::perf::event(
                        "video",
                        "seek_override_skip_clear_gpu",
                        None,
                        0,
                        &[
                            ("frame_pts", serde_json::Value::from(pts)),
                            ("now", serde_json::Value::from(now_for_clear)),
                            ("frame_serial", serde_json::Value::from(serial as i64)),
                        ],
                    );
                }
                self.emit_first_frame_event(pts);
                self.displayed_frame_seq.fetch_add(1, Ordering::Release);
                if dropped_past > 0 {
                    self.skipped_frame_count
                        .fetch_add(dropped_past, Ordering::Relaxed);
                }
                let _ = pts_for_log;
                return next_due;
            }

            let cpu_bytes = match &frame.data {
                decoder::VideoFrameData::Cpu(b) => b.as_slice(),
                #[cfg(windows)]
                decoder::VideoFrameData::Gpu(_) => unreachable!("handled above"),
            };
            let color = ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                cpu_bytes,
            );
            // override は frame.pts が override target 近傍のときだけ解除する。
            // backward seek が外れて pts ≈ 元位置のフレームが新世代 serial で来た
            // 場合に override を消すと、シークバーが target → 元位置にスナップバック
            // する (= 「← シークが効かない」現象の本質)。target 近傍チェックを
            // 入れて「シークが物理的に成功した」ときだけ通常クロックに戻す。
            let now_after = self.clock.now_secs();
            if clock::pts_clears_seek_override(frame.pts_secs, now_after) {
                // post-seek フリッカー対策: override クリア前に anchor を「今 = frame.pts」
                // に巻き戻す。これをしないと clear 直後に notify_seek_completed 時点の
                // 古い anchor + 経過 wall でクロックが一気に進み、続くフレームが pts
                // ジャンプ表示 (= ちらつき) になる。audio あり時は set_audio_pts の
                // 単調性ガード経由で安全。audio path 任せにすると pause / 末尾近傍 etc.
                // で override が永久残留するケースがあるので UI 側でも明示 clear する。
                if self.clock.is_audio_active() {
                    self.clock.set_audio_pts(frame.pts_secs);
                } else {
                    self.clock.set_fallback_anchor(frame.pts_secs);
                }
                self.clock.clear_seek_target_override(frame.seek_serial);
            } else if self.clock.is_seeking() && crate::perf::is_enabled() {
                // CPU 経路の同診断 (GPU 経路と分けて経路別の発火頻度を追える)。
                crate::perf::event(
                    "video",
                    "seek_override_skip_clear_cpu",
                    None,
                    0,
                    &[
                        ("frame_pts", serde_json::Value::from(frame.pts_secs)),
                        ("now", serde_json::Value::from(now_after)),
                        ("frame_serial", serde_json::Value::from(frame.seek_serial as i64)),
                    ],
                );
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
            self.emit_first_frame_event(pts_for_log);
            self.displayed_frame_seq.fetch_add(1, Ordering::Release);
            displayed_pts = Some(pts_for_log);
        }

        // UI 側 skip 計上: latest_renderable を上書きした際に古い候補を捨てた
        // 累積数 (= dropped_past) を perf overlay 用カウンタに反映。
        if dropped_past > 0 {
            self.skipped_frame_count
                .fetch_add(dropped_past, Ordering::Relaxed);
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

        // 再生中 / seek 中なら repaint 予約。
        // seek 中も polling 必須: post-seek 第一フレームが channel に積まれても
        // egui に repaint 要求が無いと UI が起きず channel が drain されない。
        if self.is_playing() || seek_in_flight_for_display {
            let mut due = next_due.unwrap_or_else(|| std::time::Duration::from_millis(33));
            if seek_in_flight_for_display && displayed_pts.is_none() {
                // 2 秒以内は vsync 周期 (16ms) で polling、超えたら decoder 故障を
                // 疑って 100ms に back off (CPU 100% 連発を抑制)。
                let elapsed = self
                    .seek_inflight_since
                    .map(|t| t.elapsed())
                    .unwrap_or_default();
                let poll = if elapsed > std::time::Duration::from_secs(2) {
                    std::time::Duration::from_millis(100)
                } else {
                    std::time::Duration::from_millis(16)
                };
                due = due.min(poll);
            }
            Some(due)
        } else {
            None
        }
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    /// 表示済フレーム数の累積値。perf overlay が経路 (GPU/CPU) に依存せず
    /// 「新フレームが届いたか」を 1 atomic load で検知できる。
    pub fn displayed_frame_seq(&self) -> u64 {
        self.displayed_frame_seq.load(Ordering::Acquire)
    }

    /// skip された frame の累積数 (decoder 側 video_tx Full + UI 側 dropped_past)。
    /// perf overlay が delta を取って赤縦線を出す。
    pub fn skipped_frame_count(&self) -> u64 {
        self.skipped_frame_count.load(Ordering::Acquire)
    }

    /// UI 側 future_frames に並んでいる frame 数 (= 表示待ち バッファ残量)。
    /// perf overlay で skip 発生時のコンテキスト (= starvation か overflow か) を
    /// 見極めるために使う。
    pub fn pending_frames(&self) -> usize {
        self.future_frames.len()
    }

    /// GPU 経路で最新表示フレームの view-only 情報 (handle / dims)。
    /// Some なら UI は `egui::PaintCallback` 経由で `VideoPaintCallback` を発行する。
    /// HANDLE の寿命は本構造体内の `D3d11Frame` が保証する (= 次フレームに置換される
    /// まで close されない)。
    #[cfg(windows)]
    pub fn gpu_latest(&self) -> Option<GpuLatestFrame> {
        self.gpu_latest.as_ref().map(|d| GpuLatestFrame {
            shared_handle: d.shared_handle,
            width: d.width,
            height: d.height,
            ten_bit: d.ten_bit,
            fence_value: d.fence_value,
            fence_shared_handle: d.fence_shared_handle,
            fence_gen: d.fence_gen,
        })
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
