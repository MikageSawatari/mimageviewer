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
//! FFmpeg LGPLv3-or-later build。動的リンク + ソフトウェア情報への通知 + ソース提供
//! (mikage.to に tarball 配置) で MIT ライセンスの mIV と共存可能。詳細は
//! CLAUDE.md の「FFmpeg ライセンス対応」節を参照。

pub mod audio;
pub mod audio_stretch;
pub mod clock;
pub mod decoder;
#[cfg(windows)]
pub mod dsp;
pub mod engine;
pub mod ffmpeg_loader;
#[cfg(windows)]
pub mod gpu_renderer;
#[cfg(windows)]
pub mod native_presenter;
#[cfg(windows)]
pub mod native_window;
pub mod screenshot;
pub mod thumbnail;
pub mod tile_thumb_cache;
pub mod tile_thumbnails;
pub mod upscale;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use egui::{ColorImage, TextureHandle, TextureOptions};

use clock::AvClock;
use decoder::{DecodeHandles, VideoFrame, VideoFrameData, VideoInfo};
use thumbnail::{Thumbnail, ThumbnailWorker};

use std::sync::Mutex;

use engine::EngineEvent;
use engine::actor::{EngineActor, OpenOptions};

fn engine_state_code_name(code: u8) -> &'static str {
    match code {
        engine::actor::state_code::IDLE => "Idle",
        engine::actor::state_code::LOADING => "Loading",
        engine::actor::state_code::BUFFERING => "Buffering",
        engine::actor::state_code::PLAYING => "Playing",
        engine::actor::state_code::PAUSED => "Paused",
        engine::actor::state_code::SEEKING => "Seeking",
        engine::actor::state_code::EOF => "Eof",
        _ => "Unknown",
    }
}

pub struct VideoPlayer {
    path: PathBuf,
    clock: Arc<AvClock>,
    /// Phase 3b で導入された state machine actor。Phase 3c+ で decoder/audio events を
    /// 流し込み、Phase 3d で AvClock の状態系メソッドを EngineActor 主導に置き換える。
    /// Phase 3b 時点では `begin_loading()` を呼んだ状態で保持されるが、actor の state
    /// 遷移は Phase 3c で配線完了後に有効化する (= 現在は AvClock が引き続き source of truth)。
    #[allow(dead_code)]
    engine: Arc<Mutex<EngineActor>>,
    /// Phase 9.B: EngineActor の `published_state` を Mutex なしで読むための clone。
    /// perf overlay が warmup 区間 (= state ≠ Playing) を表示するときに使う。
    engine_state_atomic: Arc<AtomicU8>,
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
    /// 共有 `seek_serial` が新世代に進む (= 外部 `clock.request_seek` または engine
    /// 内部経路の `av_clock.request_seek` 経由) と engine の latch が reset されるので、
    /// tick 側では「engine.current_seek_epoch() (= seek_serial.load()) を読み取って
    /// 自分の last_seen_epoch と比べる」方式で再発火する。
    first_frame_event_last_epoch: Option<engine::state::SeekEpoch>,
    /// 表示したフレーム数の累積カウンタ。tick で latest_renderable を採用するたびに
    /// +1。GPU/CPU 両経路で更新するので、UI 側の perf overlay が経路に依存せず
    /// 「新フレーム到着」を検知できる (Phase 8.I 修正)。
    displayed_frame_seq: Arc<AtomicU64>,
    /// 最後に実際へ表示したフレームの source pts。スクリーンショットとフレーム送りは
    /// 再生クロックではなく「見えているフレーム」を基準にする。
    last_displayed_pts_bits: Arc<AtomicU64>,
    /// Native overlay HUD が UI thread を経由せず duration を読めるように共有する。
    /// f64::to_bits() / from_bits() で保持し、InfoReceived 時に一度更新する。
    #[cfg(windows)]
    duration_secs_bits: Arc<AtomicU64>,
    /// decoder の video_tx try_send が Full で送信できず捨てた累積数。
    /// decoder thread と共有し、perf overlay では UI 側 dropped_past と色分けする。
    decoder_dropped_full_count: Arc<AtomicU64>,
    /// tick で latest_renderable を上書きし、古い候補を表示前に捨てた累積数。
    ui_dropped_past_count: AtomicU64,
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
    /// front から `pts <= now + small_margin` のものを取り出して表示。FIFO 連続性を保つことで
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
    /// 描画している期間は HANDLE が valid であることを保証する)。
    #[cfg(windows)]
    gpu_latest: Option<crate::video::gpu_renderer::D3d11Frame>,
    #[cfg(windows)]
    native_output: Option<NativeVideoOutput>,
    #[cfg(windows)]
    native_hover_thumbnail_target_secs: Mutex<Option<f64>>,
    #[cfg(windows)]
    native_hover_thumbnail_sent_key: Mutex<Option<NativeHoverThumbnailKey>>,
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeHoverThumbnailKey {
    target_bits: u64,
    width: u32,
    height: u32,
    rgba_ptr: usize,
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
#[derive(Clone, Copy, Debug)]
pub struct NativeVideoOutputConfig {
    pub rect: windows::Win32::Foundation::RECT,
    pub owner_hwnd: u64,
    pub sync_interval: u32,
    pub perf_overlay_visible: bool,
    pub initial_tile_overlay: bool,
    pub vst3_available: bool,
    pub checked: bool,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub enum NativeVideoOutputEvent {
    Window(native_window::NativeVideoWindowEvent),
    Seek {
        target_secs: f64,
    },
    TileSeek {
        target_secs: f64,
    },
    WheelNavigate {
        delta: i32,
    },
    TileColumnsDelta {
        delta: i32,
    },
    RequestSeekThumbnail {
        target_secs: f64,
    },
    ToggleTileMode,
    TogglePerfOverlay,
    ToggleVst3Gui,
    CloseFullscreen,
    SetVst3PanelVisible {
        visible: bool,
    },
    SetVst3VideoCompact {
        compact: bool,
    },
    Vst3ShowSlotGui {
        idx: usize,
        path: String,
    },
    Vst3HideSlotGui {
        idx: usize,
        path: String,
    },
    Vst3SetBypass {
        idx: usize,
        path: String,
        bypass: bool,
    },
    Vst3LoadChainSlot {
        slot_idx: usize,
    },
    Vst3SaveChainSlot {
        slot_idx: usize,
    },
    SeekToStartAndPlay,
    TogglePlay,
    ToggleMute,
    ToggleLoop,
    SetVolume {
        volume: f64,
        persist: bool,
    },
    SetPlaybackSpeed {
        speed: f64,
    },
    CopyFrameToClipboard,
    FrameStep {
        direction: i32,
    },
    AddBookmarkAt {
        target_secs: f64,
    },
    TogglePinAt {
        target_secs: f64,
    },
    SetBookmarkTitle {
        id: i64,
        title: String,
    },
    DeleteBookmark {
        id: i64,
    },
}

#[cfg(windows)]
pub(crate) struct SwitchSourcePayload {
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
    duration_secs_bits: Arc<AtomicU64>,
    source_epoch: u64,
    show_preparing_overlay: bool,
}

#[cfg(windows)]
enum NativeVideoOutputCommand {
    SetHoverThumbnail {
        thumbnail: Option<native_presenter::NativeOverlayThumbnail>,
    },
    SetHoverPreviewPinned {
        pinned: bool,
    },
    SetTimelineMarkers {
        markers: Vec<native_presenter::NativeOverlayTimelineMarker>,
    },
    SetJumpEntries {
        entries: Vec<native_presenter::NativeOverlayJumpEntry>,
    },
    SetMetadata {
        metadata: Option<native_presenter::NativeOverlayMetadata>,
    },
    SetLoopEnabled {
        enabled: bool,
    },
    SetVst3Available {
        available: bool,
    },
    SetChecked {
        checked: bool,
    },
    SetVideoCompact {
        compact: bool,
    },
    SetVst3Panel {
        panel: Option<native_presenter::NativeOverlayVst3Panel>,
    },
    SetPlaybackStatus {
        first_frame_presented: bool,
        error: Option<String>,
    },
    ShowToast {
        text: String,
        centered: bool,
    },
    SetTileOverlay {
        tile_overlay: Option<native_presenter::NativeOverlayTileOverlay>,
    },
    #[allow(dead_code)]
    SwitchSource {
        payload: Box<SwitchSourcePayload>,
    },
}

#[cfg(windows)]
struct PresenterSourceState {
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
    duration_secs_bits: Arc<AtomicU64>,
    source_epoch: u64,
    queue: std::collections::VecDeque<VideoFrame>,
    last_seen_serial: u64,
    first_frame_event_last_epoch: Option<u64>,
    pending_first_frame_event: Option<(u64, f64)>,
    present_stats: NativeFullscreenPresentStats,
    last_present_wall: Option<std::time::Instant>,
    last_present_source_pts: Option<f64>,
    source_pacing_sleep_count: u64,
    source_pacing_sleep_total_ms: f64,
}

#[cfg(windows)]
impl PresenterSourceState {
    fn new(payload: SwitchSourcePayload) -> Self {
        let last_seen_serial = payload.clock.current_seek_serial();
        Self {
            video_rx: payload.video_rx,
            clock: payload.clock,
            engine_event_tx: payload.engine_event_tx,
            displayed_frame_seq: payload.displayed_frame_seq,
            last_displayed_pts_bits: payload.last_displayed_pts_bits,
            duration_secs_bits: payload.duration_secs_bits,
            source_epoch: payload.source_epoch,
            queue: std::collections::VecDeque::new(),
            last_seen_serial,
            first_frame_event_last_epoch: None,
            pending_first_frame_event: None,
            present_stats: NativeFullscreenPresentStats::default(),
            last_present_wall: None,
            last_present_source_pts: None,
            source_pacing_sleep_count: 0,
            source_pacing_sleep_total_ms: 0.0,
        }
    }
}

#[cfg(windows)]
pub(crate) struct NativeVideoOutput {
    cancel: Arc<AtomicBool>,
    hwnd: Arc<AtomicU64>,
    first_presented: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    perf_overlay_visible: Arc<AtomicBool>,
    #[allow(dead_code)]
    source_epoch: Arc<AtomicU64>,
    last_vst3_available: AtomicBool,
    last_checked: AtomicBool,
    command_tx: std::sync::mpsc::Sender<NativeVideoOutputCommand>,
    event_rx: std::sync::Mutex<std::sync::mpsc::Receiver<(u64, NativeVideoOutputEvent)>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl NativeVideoOutput {
    fn spawn(
        video_rx: crossbeam_channel::Receiver<VideoFrame>,
        clock: Arc<AvClock>,
        engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
        displayed_frame_seq: Arc<AtomicU64>,
        last_displayed_pts_bits: Arc<AtomicU64>,
        duration_secs_bits: Arc<AtomicU64>,
        config: NativeVideoOutputConfig,
    ) -> Option<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let hwnd = Arc::new(AtomicU64::new(0));
        let first_presented = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let perf_overlay_visible = Arc::new(AtomicBool::new(config.perf_overlay_visible));
        let source_epoch = Arc::new(AtomicU64::new(0));
        let initial_vst3_available = config.vst3_available;
        let initial_checked = config.checked;
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let thread_cancel = Arc::clone(&cancel);
        let thread_hwnd = Arc::clone(&hwnd);
        let thread_first_presented = Arc::clone(&first_presented);
        let thread_closed = Arc::clone(&closed);
        let thread_perf_overlay_visible = Arc::clone(&perf_overlay_visible);
        let thread = match std::thread::Builder::new()
            .name("native-video-presenter".into())
            .spawn(move || {
                if let Err(err) = run_native_video_output(
                    video_rx,
                    clock,
                    engine_event_tx,
                    displayed_frame_seq,
                    last_displayed_pts_bits,
                    duration_secs_bits,
                    config,
                    command_rx,
                    event_tx,
                    thread_cancel,
                    thread_hwnd,
                    thread_first_presented,
                    thread_closed,
                    thread_perf_overlay_visible,
                ) {
                    crate::logger::log(format!("[native-video] presenter stopped: {err}"));
                }
            }) {
            Ok(thread) => thread,
            Err(err) => {
                crate::logger::log(format!("[native-video] failed to spawn presenter: {err}"));
                return None;
            }
        };
        Some(Self {
            cancel,
            hwnd,
            first_presented,
            closed,
            perf_overlay_visible,
            source_epoch,
            last_vst3_available: AtomicBool::new(initial_vst3_available),
            last_checked: AtomicBool::new(initial_checked),
            command_tx,
            event_rx: std::sync::Mutex::new(event_rx),
            thread: Some(thread),
        })
    }

    fn hwnd(&self) -> u64 {
        self.hwnd.load(Ordering::Acquire)
    }

    fn first_presented(&self) -> bool {
        self.first_presented.load(Ordering::Acquire)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    fn source_epoch(&self) -> u64 {
        self.source_epoch.load(Ordering::Acquire)
    }

    fn set_perf_overlay_visible(&self, visible: bool) {
        self.perf_overlay_visible.store(visible, Ordering::Release);
    }

    fn set_hover_thumbnail(&self, thumbnail: Option<native_presenter::NativeOverlayThumbnail>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetHoverThumbnail { thumbnail });
    }

    fn set_hover_preview_pinned(&self, pinned: bool) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetHoverPreviewPinned { pinned });
    }

    fn set_timeline_markers(&self, markers: Vec<native_presenter::NativeOverlayTimelineMarker>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetTimelineMarkers { markers });
    }

    fn set_jump_entries(&self, entries: Vec<native_presenter::NativeOverlayJumpEntry>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetJumpEntries { entries });
    }

    fn set_metadata(&self, metadata: Option<native_presenter::NativeOverlayMetadata>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetMetadata { metadata });
    }

    fn set_loop_enabled(&self, enabled: bool) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetLoopEnabled { enabled });
    }

    fn set_vst3_available(&self, available: bool) {
        if self.last_vst3_available.swap(available, Ordering::AcqRel) == available {
            return;
        }
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetVst3Available { available });
    }

    fn set_checked(&self, checked: bool) {
        if self.last_checked.swap(checked, Ordering::AcqRel) == checked {
            return;
        }
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetChecked { checked });
    }

    fn set_video_compact(&self, compact: bool) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetVideoCompact { compact });
    }

    fn set_vst3_panel(&self, panel: Option<native_presenter::NativeOverlayVst3Panel>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetVst3Panel { panel });
    }

    fn set_tile_overlay(&self, tile_overlay: Option<native_presenter::NativeOverlayTileOverlay>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetTileOverlay { tile_overlay });
    }

    #[allow(dead_code)]
    fn switch_source(&self, payload: SwitchSourcePayload) {
        self.first_presented.store(false, Ordering::Release);
        self.source_epoch
            .store(payload.source_epoch, Ordering::Release);
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SwitchSource {
                payload: Box::new(payload),
            });
    }

    fn set_playback_status(&self, first_frame_presented: bool, error: Option<String>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetPlaybackStatus {
                first_frame_presented,
                error,
            });
    }

    fn show_toast(&self, text: String, centered: bool) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::ShowToast { text, centered });
    }

    fn drain_events(&self) -> Vec<(u64, NativeVideoOutputEvent)> {
        let Ok(rx) = self.event_rx.lock() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }
}

#[cfg(windows)]
impl Drop for NativeVideoOutput {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = std::thread::Builder::new()
                .name("native-video-output-drop-join".to_string())
                .spawn(move || {
                    let _ = thread.join();
                });
        }
    }
}

#[cfg(windows)]
struct NativeComApartment;

#[cfg(windows)]
impl NativeComApartment {
    fn init() -> Result<Self, String> {
        unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
            .ok()
            .map_err(|e| format!("CoInitializeEx: {e:?}"))?;
        }
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for NativeComApartment {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}

#[cfg(windows)]
#[derive(Default)]
struct NativeFullscreenPresentStats {
    presented: u64,
    gpu: u64,
    cpu: u64,
    late_drop: u64,
    wait_timeout: u64,
    max_late_ms: f64,
    max_total_ms: f64,
    max_interval_ms: f64,
}

#[cfg(windows)]
impl NativeFullscreenPresentStats {
    fn record_present(
        &mut self,
        outcome: &crate::video::native_presenter::NativePresentOutcome,
        late_ms: f64,
        total_ms: f64,
        interval_ms: f64,
    ) {
        self.presented += 1;
        match outcome.path {
            "d3d11_shared" => self.gpu += 1,
            "cpu_upload" => self.cpu += 1,
            _ => {}
        }
        if outcome.wait_timed_out {
            self.wait_timeout += 1;
        }
        self.max_late_ms = self.max_late_ms.max(late_ms);
        self.max_total_ms = self.max_total_ms.max(total_ms);
        self.max_interval_ms = self.max_interval_ms.max(interval_ms);
    }

    #[allow(dead_code)]
    fn record_late_drop(&mut self, pts: f64, late_ms: f64, queue_len: usize) {
        self.late_drop += 1;
        crate::perf::event(
            "native_presenter",
            "late_drop",
            None,
            0,
            &[
                ("pts", serde_json::Value::from(pts)),
                ("late_ms", serde_json::Value::from(late_ms)),
                ("queue_len", serde_json::Value::from(queue_len as i64)),
            ],
        );
    }

    fn emit_summary(&self, duration: std::time::Duration) {
        let actual_fps = if duration.as_secs_f64() > 0.0 {
            self.presented as f64 / duration.as_secs_f64()
        } else {
            0.0
        };
        crate::perf::event(
            "native_presenter",
            "summary",
            None,
            0,
            &[
                ("presented", serde_json::Value::from(self.presented as i64)),
                ("gpu_frames", serde_json::Value::from(self.gpu as i64)),
                ("cpu_frames", serde_json::Value::from(self.cpu as i64)),
                ("late_drop", serde_json::Value::from(self.late_drop as i64)),
                (
                    "wait_timeout",
                    serde_json::Value::from(self.wait_timeout as i64),
                ),
                ("actual_fps", serde_json::Value::from(actual_fps)),
                ("max_late_ms", serde_json::Value::from(self.max_late_ms)),
                ("max_total_ms", serde_json::Value::from(self.max_total_ms)),
                (
                    "max_interval_ms",
                    serde_json::Value::from(self.max_interval_ms),
                ),
            ],
        );
        crate::logger::log(format!(
            "[native-video] fullscreen presenter summary: presented={} fps={:.1} gpu={} cpu={} late_drop={} max_late_ms={:.1} max_interval_ms={:.1}",
            self.presented,
            actual_fps,
            self.gpu,
            self.cpu,
            self.late_drop,
            self.max_late_ms,
            self.max_interval_ms
        ));
    }

    fn overlay_snapshot(
        &self,
        duration: std::time::Duration,
    ) -> crate::video::native_presenter::NativeOverlayPerfSnapshot {
        let elapsed_secs = duration.as_secs_f64();
        let actual_fps = if elapsed_secs > 0.0 {
            self.presented as f64 / elapsed_secs
        } else {
            0.0
        };
        crate::video::native_presenter::NativeOverlayPerfSnapshot {
            elapsed_secs,
            presented: self.presented,
            gpu: self.gpu,
            cpu: self.cpu,
            late_drop: self.late_drop,
            wait_timeout: self.wait_timeout,
            actual_fps,
            max_late_ms: self.max_late_ms,
            max_total_ms: self.max_total_ms,
            max_interval_ms: self.max_interval_ms,
        }
    }
}

#[cfg(windows)]
fn native_reset_unpresented_frame(mut frame: VideoFrame) {
    if let VideoFrameData::Gpu(gpu) = &mut frame.data {
        gpu.reset_unpresented_shared_output();
    }
}

#[cfg(not(windows))]
fn native_reset_unpresented_frame(_frame: VideoFrame) {}

#[cfg(windows)]
fn native_drain_unpresented_queue(queue: &mut std::collections::VecDeque<VideoFrame>) {
    while let Some(frame) = queue.pop_front() {
        native_reset_unpresented_frame(frame);
    }
}

#[cfg(not(windows))]
fn native_drain_unpresented_queue(queue: &mut std::collections::VecDeque<VideoFrame>) {
    queue.clear();
}

#[cfg(windows)]
fn native_video_env_flag_enabled(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|v| {
            let v = v.trim();
            !(v.is_empty()
                || v == "0"
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("no"))
        })
        .unwrap_or(default)
}

#[cfg(windows)]
pub fn native_presenter_enabled_by_env() -> bool {
    native_video_env_flag_enabled("MIV_NATIVE_VIDEO_PRESENTER", true)
}

#[cfg(windows)]
fn native_source_pacing_delay(
    last_pts: Option<f64>,
    last_wall: Option<std::time::Instant>,
    next_pts: f64,
    playback_speed: f64,
) -> Option<std::time::Duration> {
    let (Some(last_pts), Some(last_wall)) = (last_pts, last_wall) else {
        return None;
    };
    let source_delta = next_pts - last_pts;
    if !(0.001..=0.050).contains(&source_delta) {
        return None;
    }
    let elapsed = last_wall.elapsed().as_secs_f64();
    let speed = playback_speed.max(clock::MIN_PLAYBACK_SPEED);
    let target_elapsed = ((source_delta - 0.0002).max(0.0)) / speed;
    if elapsed >= target_elapsed {
        return None;
    }
    Some(std::time::Duration::from_secs_f64(
        (target_elapsed - elapsed).clamp(0.001, 0.012),
    ))
}

#[cfg(windows)]
fn try_send_native_first_frame_ready(
    engine_event_tx: &crossbeam_channel::Sender<EngineEvent>,
    epoch: u64,
    pts: f64,
) -> bool {
    let event = EngineEvent::Decoder(engine::state::DecoderEvent::FirstFrameReady { epoch, pts });
    match engine_event_tx.try_send(event) {
        Ok(()) => {
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "first_frame_ready_send",
                    None,
                    0,
                    &[
                        ("result", serde_json::Value::from("sent")),
                        ("epoch", serde_json::Value::from(epoch as i64)),
                        ("pts", serde_json::Value::from(pts)),
                    ],
                );
            }
            true
        }
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "first_frame_ready_send",
                    None,
                    0,
                    &[
                        ("result", serde_json::Value::from("pending")),
                        ("epoch", serde_json::Value::from(epoch as i64)),
                        ("pts", serde_json::Value::from(pts)),
                    ],
                );
            }
            false
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "first_frame_ready_send",
                    None,
                    0,
                    &[
                        ("result", serde_json::Value::from("disconnected")),
                        ("epoch", serde_json::Value::from(epoch as i64)),
                        ("pts", serde_json::Value::from(pts)),
                    ],
                );
            }
            true
        }
    }
}

#[cfg(windows)]
fn send_native_output_event(
    tx: &std::sync::mpsc::Sender<(u64, NativeVideoOutputEvent)>,
    source_epoch: u64,
    event: NativeVideoOutputEvent,
) {
    let _ = tx.send((source_epoch, event));
}

#[cfg(windows)]
fn send_native_overlay_command(
    tx: &std::sync::mpsc::Sender<(u64, NativeVideoOutputEvent)>,
    source_epoch: u64,
    command: crate::video::native_presenter::NativeOverlayCommand,
) {
    use crate::video::native_presenter::NativeOverlayCommand as Command;
    let event = match command {
        Command::Seek { target_secs } => NativeVideoOutputEvent::Seek { target_secs },
        Command::TileSeek { target_secs } => NativeVideoOutputEvent::TileSeek { target_secs },
        Command::WheelNavigate { delta } => NativeVideoOutputEvent::WheelNavigate { delta },
        Command::TileColumnsDelta { delta } => NativeVideoOutputEvent::TileColumnsDelta { delta },
        Command::RequestSeekThumbnail { target_secs } => {
            NativeVideoOutputEvent::RequestSeekThumbnail { target_secs }
        }
        Command::ToggleTileMode => NativeVideoOutputEvent::ToggleTileMode,
        Command::TogglePerfOverlay => NativeVideoOutputEvent::TogglePerfOverlay,
        Command::ToggleVst3Gui => NativeVideoOutputEvent::ToggleVst3Gui,
        Command::CloseFullscreen => NativeVideoOutputEvent::CloseFullscreen,
        Command::SetVst3PanelVisible { visible } => {
            NativeVideoOutputEvent::SetVst3PanelVisible { visible }
        }
        Command::SetVst3VideoCompact { compact } => {
            NativeVideoOutputEvent::SetVst3VideoCompact { compact }
        }
        Command::Vst3ShowSlotGui { idx, path } => {
            NativeVideoOutputEvent::Vst3ShowSlotGui { idx, path }
        }
        Command::Vst3HideSlotGui { idx, path } => {
            NativeVideoOutputEvent::Vst3HideSlotGui { idx, path }
        }
        Command::Vst3SetBypass { idx, path, bypass } => {
            NativeVideoOutputEvent::Vst3SetBypass { idx, path, bypass }
        }
        Command::Vst3LoadChainSlot { slot_idx } => {
            NativeVideoOutputEvent::Vst3LoadChainSlot { slot_idx }
        }
        Command::Vst3SaveChainSlot { slot_idx } => {
            NativeVideoOutputEvent::Vst3SaveChainSlot { slot_idx }
        }
        Command::SeekToStartAndPlay => NativeVideoOutputEvent::SeekToStartAndPlay,
        Command::TogglePlay => NativeVideoOutputEvent::TogglePlay,
        Command::ToggleMute => NativeVideoOutputEvent::ToggleMute,
        Command::SetVolume { volume, persist } => {
            NativeVideoOutputEvent::SetVolume { volume, persist }
        }
        Command::SetPlaybackSpeed { speed } => NativeVideoOutputEvent::SetPlaybackSpeed { speed },
        Command::CopyFrameToClipboard => NativeVideoOutputEvent::CopyFrameToClipboard,
        Command::FrameStep { direction } => NativeVideoOutputEvent::FrameStep { direction },
        Command::ToggleLoop => NativeVideoOutputEvent::ToggleLoop,
        Command::AddBookmarkAt { target_secs } => {
            NativeVideoOutputEvent::AddBookmarkAt { target_secs }
        }
        Command::TogglePinAt { target_secs } => NativeVideoOutputEvent::TogglePinAt { target_secs },
        Command::SetBookmarkTitle { id, title } => {
            NativeVideoOutputEvent::SetBookmarkTitle { id, title }
        }
        Command::DeleteBookmark { id } => NativeVideoOutputEvent::DeleteBookmark { id },
    };
    send_native_output_event(tx, source_epoch, event);
}

#[cfg(windows)]
fn run_native_video_output(
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
    duration_secs_bits: Arc<AtomicU64>,
    config: NativeVideoOutputConfig,
    command_rx: std::sync::mpsc::Receiver<NativeVideoOutputCommand>,
    ui_event_tx: std::sync::mpsc::Sender<(u64, NativeVideoOutputEvent)>,
    cancel: Arc<AtomicBool>,
    hwnd_out: Arc<AtomicU64>,
    first_presented_out: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    perf_overlay_visible: Arc<AtomicBool>,
) -> Result<(), String> {
    use std::time::{Duration, Instant};

    let _com = NativeComApartment::init()?;
    let width = (config.rect.right - config.rect.left).max(1) as u32;
    let height = (config.rect.bottom - config.rect.top).max(1) as u32;
    let (presenter_event_tx, presenter_event_rx) = std::sync::mpsc::channel();
    let mut window = crate::video::native_window::NativeVideoWindow::create(
        crate::video::native_window::NativeVideoWindowConfig {
            mode: crate::video::native_window::NativeVideoWindowMode::Borderless {
                rect: config.rect,
            },
            owner_hwnd: config.owner_hwnd,
            close_on_escape: false,
            // This HWND lives on the presenter thread, so WM_QUIT only exits
            // this loop and does not affect eframe's main event loop.
            post_quit_on_destroy: true,
            event_tx: Some(presenter_event_tx),
        },
    )?;
    hwnd_out.store(window.hwnd().0 as u64, Ordering::Release);
    let mut presenter = match crate::video::native_presenter::NativeVideoPresenter::new(
        crate::video::native_presenter::NativePresenterConfig {
            hwnd: window.hwnd(),
            width,
            height,
            test_overlay: std::env::var_os("MIV_NATIVE_VIDEO_TEST_OVERLAY").is_some(),
            egui_overlay: native_video_env_flag_enabled("MIV_NATIVE_VIDEO_EGUI_OVERLAY", true),
        },
    ) {
        Ok(presenter) => presenter,
        Err(err) => {
            hwnd_out.store(0, Ordering::Release);
            window.destroy();
            closed.store(true, Ordering::Release);
            return Err(err);
        }
    };
    crate::logger::log(format!(
        "[native-video] fullscreen presenter started hwnd=0x{:x} rect=({},{} {}x{}) sync_interval={}",
        window.hwnd().0 as usize,
        config.rect.left,
        config.rect.top,
        width,
        height,
        config.sync_interval
    ));
    presenter.set_overlay_vst3_available(config.vst3_available);
    presenter.set_overlay_checked(config.checked);
    if config.initial_tile_overlay {
        presenter.set_overlay_tile_overlay(Some(
            crate::video::native_presenter::NativeOverlayTileOverlay::preparing(),
        ));
        if let Err(err) = presenter.tick_overlay_video_state(
            clock.now_secs(),
            f64::from_bits(duration_secs_bits.load(Ordering::Acquire)),
            clock.is_playing(),
            clock.volume(),
            clock.is_muted(),
            clock.playback_speed(),
        ) {
            crate::logger::log(format!(
                "[native-video] initial tile overlay render failed: {err}"
            ));
        }
    }

    let mut source = PresenterSourceState::new(SwitchSourcePayload {
        video_rx,
        clock,
        engine_event_tx,
        displayed_frame_seq,
        last_displayed_pts_bits,
        duration_secs_bits,
        source_epoch: 0,
        show_preparing_overlay: config.initial_tile_overlay,
    });
    let run_started = Instant::now();
    let mut last_summary_log = Instant::now();
    let mut last_present_log = Instant::now();
    let mut last_overlay_tick = Instant::now();
    let mut last_source_state_probe = Instant::now();
    let startup_probe_until = run_started + Duration::from_secs(5);
    let mut last_startup_probe = run_started
        .checked_sub(Duration::from_millis(250))
        .unwrap_or(run_started);
    let mut startup_probe_count = 0_u32;
    let mut first_present_probe_logged = false;
    let mut native_events = Vec::new();
    let trace_every_present = std::env::var_os("MIV_NATIVE_VIDEO_PRESENT_TRACE").is_some();
    while !cancel.load(Ordering::Acquire) {
        // `FirstFrameReady` is what lets the engine leave Buffering after a seek.
        // The engine event channel can be temporarily full during seek bursts, so
        // retry instead of treating a failed try_send as delivered.
        if let Some((epoch, pts)) = source.pending_first_frame_event {
            let clock_serial = source.clock.current_seek_serial();
            if epoch < clock_serial {
                source.pending_first_frame_event = None;
            } else if try_send_native_first_frame_ready(&source.engine_event_tx, epoch, pts) {
                source.first_frame_event_last_epoch = Some(epoch);
                source.pending_first_frame_event = None;
            }
        }

        if crate::video::native_window::pump_thread_messages() {
            closed.store(true, Ordering::Release);
            break;
        }
        let now = Instant::now();
        if now < startup_probe_until
            && now.duration_since(last_startup_probe) >= Duration::from_millis(250)
        {
            last_startup_probe = now;
            startup_probe_count = startup_probe_count.saturating_add(1);
            let first_presented = first_presented_out.load(Ordering::Acquire);
            let displayed_seq = source.displayed_frame_seq.load(Ordering::Acquire);
            let foreground_current_process =
                crate::video::native_window::foreground_belongs_to_current_process();
            let source_queue_len = source.queue.len();
            let source_video_rx_len = source.video_rx.len();
            let source_serial = source.clock.current_seek_serial();
            let source_playing = source.clock.is_playing();
            let source_seeking = source.clock.is_seeking();
            crate::logger::log(format!(
                "[native-video] startup probe #{} elapsed_ms={:.1} first_presented={} displayed_seq={} foreground_current_process={} source_serial={} playing={} seeking={} source_queue_len={} video_rx_len={}",
                startup_probe_count,
                now.duration_since(run_started).as_secs_f64() * 1000.0,
                first_presented,
                displayed_seq,
                foreground_current_process,
                source_serial,
                source_playing,
                source_seeking,
                source_queue_len,
                source_video_rx_len
            ));
            crate::video::native_window::log_state(window.hwnd().0 as u64, "startup-probe");
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "startup_probe",
                    None,
                    0,
                    &[
                        ("probe", serde_json::Value::from(startup_probe_count as i64)),
                        (
                            "elapsed_ms",
                            serde_json::Value::from(
                                now.duration_since(run_started).as_secs_f64() * 1000.0,
                            ),
                        ),
                        ("first_presented", serde_json::Value::from(first_presented)),
                        (
                            "displayed_seq",
                            serde_json::Value::from(displayed_seq as i64),
                        ),
                        (
                            "foreground_current_process",
                            serde_json::Value::from(foreground_current_process),
                        ),
                        (
                            "source_serial",
                            serde_json::Value::from(source_serial as i64),
                        ),
                        ("playing", serde_json::Value::from(source_playing)),
                        ("seeking", serde_json::Value::from(source_seeking)),
                        (
                            "source_queue_len",
                            serde_json::Value::from(source_queue_len as i64),
                        ),
                        (
                            "video_rx_len",
                            serde_json::Value::from(source_video_rx_len as i64),
                        ),
                    ],
                );
            }
        }
        let perf_visible = perf_overlay_visible.load(Ordering::Acquire);
        let perf_visibility_changed = presenter.set_overlay_perf_visible(perf_visible);
        while let Ok(command) = command_rx.try_recv() {
            match command {
                NativeVideoOutputCommand::SetHoverThumbnail { thumbnail } => {
                    presenter.set_overlay_hover_thumbnail(thumbnail);
                }
                NativeVideoOutputCommand::SetHoverPreviewPinned { pinned } => {
                    presenter.set_overlay_hover_preview_pinned(pinned);
                }
                NativeVideoOutputCommand::SetTimelineMarkers { markers } => {
                    presenter.set_overlay_timeline_markers(markers);
                }
                NativeVideoOutputCommand::SetJumpEntries { entries } => {
                    presenter.set_overlay_jump_entries(entries);
                }
                NativeVideoOutputCommand::SetMetadata { metadata } => {
                    presenter.set_overlay_metadata(metadata);
                }
                NativeVideoOutputCommand::SetLoopEnabled { enabled } => {
                    presenter.set_overlay_loop_enabled(enabled);
                }
                NativeVideoOutputCommand::SetVst3Available { available } => {
                    presenter.set_overlay_vst3_available(available);
                }
                NativeVideoOutputCommand::SetChecked { checked } => {
                    presenter.set_overlay_checked(checked);
                }
                NativeVideoOutputCommand::SetVideoCompact { compact } => {
                    if let Err(err) = presenter.set_video_compact(compact) {
                        crate::logger::log(format!(
                            "[native-video] set compact transform failed: {err}"
                        ));
                    }
                }
                NativeVideoOutputCommand::SetVst3Panel { panel } => {
                    presenter.set_overlay_vst3_panel(panel);
                }
                NativeVideoOutputCommand::SetPlaybackStatus {
                    first_frame_presented,
                    error,
                } => {
                    presenter.set_overlay_playback_status(first_frame_presented, error);
                }
                NativeVideoOutputCommand::ShowToast { text, centered } => {
                    presenter.show_overlay_toast(text, centered);
                }
                NativeVideoOutputCommand::SetTileOverlay { tile_overlay } => {
                    presenter.set_overlay_tile_overlay(tile_overlay);
                }
                NativeVideoOutputCommand::SwitchSource { payload } => {
                    native_drain_unpresented_queue(&mut source.queue);
                    let show_preparing_overlay = payload.show_preparing_overlay;
                    source = PresenterSourceState::new(*payload);
                    first_presented_out.store(false, Ordering::Release);
                    presenter.set_overlay_playback_status(false, None);
                    presenter.set_overlay_metadata(None);
                    presenter.set_overlay_timeline_markers(Vec::new());
                    presenter.set_overlay_jump_entries(Vec::new());
                    if source.source_epoch > 0 && source.queue.is_empty() {
                        crate::logger::log(format!(
                            "[native-video] presenter switched source epoch={}",
                            source.source_epoch
                        ));
                    }
                    if source.source_epoch > 0 || source.clock.is_seeking() {
                        last_overlay_tick = Instant::now();
                    }
                    if source.source_epoch > 0 {
                        native_events.clear();
                        while presenter_event_rx.try_recv().is_ok() {}
                    }
                    if show_preparing_overlay {
                        // The new player will resend fresh overlay content; keep the
                        // tile surface visible while its VideoInfo and thumbnails load.
                        presenter.set_overlay_tile_overlay(Some(
                            crate::video::native_presenter::NativeOverlayTileOverlay::preparing(),
                        ));
                    }
                    if !source.clock.is_playing() {
                        source.last_present_wall = None;
                        source.last_present_source_pts = None;
                    }
                }
            }
        }
        native_events.clear();
        while let Ok(event) = presenter_event_rx.try_recv() {
            native_events.push(event);
        }
        if !native_events.is_empty() {
            presenter.update_overlay_video_state(
                source.clock.now_secs(),
                f64::from_bits(source.duration_secs_bits.load(Ordering::Acquire)),
                source.clock.is_playing(),
                source.clock.volume(),
                source.clock.is_muted(),
                source.clock.playback_speed(),
            );
            let overlay_routing = match presenter.handle_window_events(&native_events) {
                Ok(outcome) => {
                    for command in outcome.commands {
                        let event_epoch = source.source_epoch;
                        match command {
                            crate::video::native_presenter::NativeOverlayCommand::Seek {
                                target_secs,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::Seek { target_secs },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::TileSeek {
                                target_secs,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::TileSeek { target_secs },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::WheelNavigate {
                                delta,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::WheelNavigate { delta },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::TileColumnsDelta {
                                delta,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::TileColumnsDelta { delta },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::RequestSeekThumbnail {
                                target_secs,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::RequestSeekThumbnail { target_secs },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleTileMode => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleTileMode,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::TogglePerfOverlay => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::TogglePerfOverlay,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleVst3Gui => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleVst3Gui,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::CloseFullscreen => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::CloseFullscreen,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SetVst3PanelVisible {
                                visible,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetVst3PanelVisible { visible },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SetVst3VideoCompact {
                                compact,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetVst3VideoCompact { compact },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::Vst3ShowSlotGui {
                                idx,
                                path,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::Vst3ShowSlotGui { idx, path },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::Vst3HideSlotGui {
                                idx,
                                path,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::Vst3HideSlotGui { idx, path },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::Vst3SetBypass {
                                idx,
                                path,
                                bypass,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::Vst3SetBypass { idx, path, bypass },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::Vst3LoadChainSlot {
                                slot_idx,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::Vst3LoadChainSlot { slot_idx },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::Vst3SaveChainSlot {
                                slot_idx,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::Vst3SaveChainSlot { slot_idx },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SeekToStartAndPlay => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SeekToStartAndPlay,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::TogglePlay => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::TogglePlay,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleMute => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleMute,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleLoop => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleLoop,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SetVolume {
                                volume,
                                persist,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetVolume { volume, persist },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SetPlaybackSpeed {
                                speed,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetPlaybackSpeed { speed },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::CopyFrameToClipboard => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::CopyFrameToClipboard,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::FrameStep {
                                direction,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::FrameStep { direction },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::AddBookmarkAt {
                                target_secs,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::AddBookmarkAt { target_secs },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::TogglePinAt {
                                target_secs,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::TogglePinAt { target_secs },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SetBookmarkTitle {
                                id,
                                title,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetBookmarkTitle { id, title },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::DeleteBookmark {
                                id,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::DeleteBookmark { id },
                                );
                            }
                        }
                    }
                    outcome.routing
                }
                Err(err) => {
                    crate::logger::log(format!(
                        "[native-video] overlay input render failed: {err}"
                    ));
                    crate::video::native_presenter::NativeOverlayInputRouting::default()
                }
            };
            for event in &native_events {
                if overlay_routing.should_forward_to_ui(*event) {
                    send_native_output_event(
                        &ui_event_tx,
                        source.source_epoch,
                        NativeVideoOutputEvent::Window(*event),
                    );
                }
            }
            last_overlay_tick = Instant::now();
        } else if perf_visibility_changed
            || presenter.overlay_needs_render()
            || (presenter.overlay_wants_periodic_tick()
                && last_overlay_tick.elapsed() >= Duration::from_millis(250))
        {
            match presenter.tick_overlay_video_state(
                source.clock.now_secs(),
                f64::from_bits(source.duration_secs_bits.load(Ordering::Acquire)),
                source.clock.is_playing(),
                source.clock.volume(),
                source.clock.is_muted(),
                source.clock.playback_speed(),
            ) {
                Ok(outcome) => {
                    for command in outcome.commands {
                        send_native_overlay_command(&ui_event_tx, source.source_epoch, command);
                    }
                }
                Err(err) => {
                    crate::logger::log(format!("[native-video] overlay tick render failed: {err}"));
                }
            }
            last_overlay_tick = Instant::now();
        }

        let clock_serial = source.clock.current_seek_serial();
        if clock_serial != source.last_seen_serial {
            native_drain_unpresented_queue(&mut source.queue);
            source.last_seen_serial = clock_serial;
            source.first_frame_event_last_epoch = None;
            source.pending_first_frame_event = None;
            source.last_present_source_pts = None;
        }
        while let Ok(frame) = source.video_rx.try_recv() {
            if frame.seek_serial < clock_serial {
                native_reset_unpresented_frame(frame);
                continue;
            }
            if frame.seek_serial > source.last_seen_serial {
                native_drain_unpresented_queue(&mut source.queue);
                source.last_seen_serial = frame.seek_serial;
                source.first_frame_event_last_epoch = None;
                source.pending_first_frame_event = None;
                source.last_present_source_pts = None;
            }
            source.queue.push_back(frame);
        }

        let waiting_for_first_frame =
            source.first_frame_event_last_epoch != Some(source.last_seen_serial);
        if crate::perf::is_enabled() && last_source_state_probe.elapsed() >= Duration::from_secs(1)
        {
            last_source_state_probe = Instant::now();
            crate::perf::event(
                "native_presenter",
                "source_state",
                None,
                0,
                &[
                    (
                        "source_epoch",
                        serde_json::Value::from(source.source_epoch as i64),
                    ),
                    (
                        "clock_serial",
                        serde_json::Value::from(source.clock.current_seek_serial() as i64),
                    ),
                    (
                        "last_seen_serial",
                        serde_json::Value::from(source.last_seen_serial as i64),
                    ),
                    (
                        "playing",
                        serde_json::Value::from(source.clock.is_playing()),
                    ),
                    (
                        "seeking",
                        serde_json::Value::from(source.clock.is_seeking()),
                    ),
                    (
                        "waiting_for_first_frame",
                        serde_json::Value::from(waiting_for_first_frame),
                    ),
                    (
                        "source_queue_len",
                        serde_json::Value::from(source.queue.len() as i64),
                    ),
                    (
                        "video_rx_len",
                        serde_json::Value::from(source.video_rx.len() as i64),
                    ),
                    (
                        "displayed_seq",
                        serde_json::Value::from(
                            source.displayed_frame_seq.load(Ordering::Acquire) as i64
                        ),
                    ),
                ],
            );
        }
        if !source.clock.is_playing() && !source.clock.is_seeking() && !waiting_for_first_frame {
            source.last_present_wall = None;
            source.last_present_source_pts = None;
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }

        let now = source.clock.now_secs();
        let mut latest_renderable: Option<VideoFrame> = None;
        while let Some(front) = source.queue.front() {
            if front.seek_serial < source.clock.current_seek_serial() {
                if let Some(frame) = source.queue.pop_front() {
                    native_reset_unpresented_frame(frame);
                }
                continue;
            }
            let force_first_frame =
                waiting_for_first_frame && front.seek_serial == source.last_seen_serial;
            let force_display_seek = source.clock.is_seeking()
                && front.seek_serial == source.last_seen_serial
                && clock::pts_clears_seek_override(front.pts_secs, now);
            if force_first_frame
                || force_display_seek
                || front.pts_secs <= now + clock::DISPLAY_LEAD_TOLERANCE_SECS
            {
                let candidate = source
                    .queue
                    .pop_front()
                    .expect("queue.front() returned Some");
                if let Some(dropped) = latest_renderable.replace(candidate) {
                    let late_ms = ((now - dropped.pts_secs) * 1000.0).max(0.0);
                    source.present_stats.record_late_drop(
                        dropped.pts_secs,
                        late_ms,
                        source.queue.len(),
                    );
                    native_reset_unpresented_frame(dropped);
                }
                continue;
            }
            break;
        }

        if let Some(frame) = latest_renderable {
            let pts = frame.pts_secs;
            if let Some(delay) = native_source_pacing_delay(
                source.last_present_source_pts,
                source.last_present_wall,
                pts,
                source.clock.playback_speed(),
            ) {
                source.queue.push_front(frame);
                source.source_pacing_sleep_count =
                    source.source_pacing_sleep_count.saturating_add(1);
                source.source_pacing_sleep_total_ms += delay.as_secs_f64() * 1000.0;
                std::thread::sleep(delay);
                continue;
            }
            let serial = frame.seek_serial;
            let present_t0 = Instant::now();
            match presenter.present(&frame, config.sync_interval) {
                Ok(outcome) => {
                    let total_ms = present_t0.elapsed().as_secs_f64() * 1000.0;
                    let late_ms = ((source.clock.now_secs() - pts) * 1000.0).max(0.0);
                    let source_delta_ms = source
                        .last_present_source_pts
                        .map(|last| (pts - last) * 1000.0)
                        .unwrap_or(0.0);
                    let interval_ms = source
                        .last_present_wall
                        .map(|last| {
                            present_t0.saturating_duration_since(last).as_secs_f64() * 1000.0
                        })
                        .unwrap_or(0.0);
                    source.last_present_wall = Some(present_t0);
                    source.last_present_source_pts = Some(pts);
                    source
                        .last_displayed_pts_bits
                        .store(pts.to_bits(), Ordering::Release);
                    source
                        .present_stats
                        .record_present(&outcome, late_ms, total_ms, interval_ms);
                    presenter.push_overlay_perf_sample(
                        crate::video::native_presenter::NativeOverlayPerfSample {
                            arrival: present_t0,
                            interval_ms: interval_ms as f32,
                            total_ms: total_ms as f32,
                            copy_ms: outcome.copy_ms as f32,
                            present_waitable_ms: outcome.present_waitable_ms as f32,
                            present_call_ms: outcome.present_call_ms as f32,
                            late_ms: late_ms as f32,
                            source_delta_ms: source_delta_ms as f32,
                        },
                        source.present_stats.overlay_snapshot(run_started.elapsed()),
                    );
                    if last_summary_log.elapsed() >= Duration::from_secs(1) {
                        source.present_stats.emit_summary(run_started.elapsed());
                        if source.source_pacing_sleep_count > 0 && crate::perf::is_enabled() {
                            crate::perf::event(
                                "native_presenter",
                                "source_pacing_summary",
                                None,
                                0,
                                &[
                                    (
                                        "count",
                                        serde_json::Value::from(
                                            source.source_pacing_sleep_count as i64,
                                        ),
                                    ),
                                    (
                                        "total_ms",
                                        serde_json::Value::from(
                                            source.source_pacing_sleep_total_ms,
                                        ),
                                    ),
                                ],
                            );
                        }
                        source.source_pacing_sleep_count = 0;
                        source.source_pacing_sleep_total_ms = 0.0;
                        last_summary_log = Instant::now();
                    }
                    source.displayed_frame_seq.fetch_add(1, Ordering::Release);
                    first_presented_out.store(true, Ordering::Release);
                    if !first_present_probe_logged {
                        first_present_probe_logged = true;
                        crate::logger::log(format!(
                            "[native-video] first present probe: pts={:.3} serial={} elapsed_ms={:.1}",
                            pts,
                            serial,
                            present_t0.duration_since(run_started).as_secs_f64() * 1000.0
                        ));
                        crate::video::native_window::log_state(
                            window.hwnd().0 as u64,
                            "first-present",
                        );
                    }
                    if source.first_frame_event_last_epoch != Some(serial) {
                        if try_send_native_first_frame_ready(&source.engine_event_tx, serial, pts) {
                            source.first_frame_event_last_epoch = Some(serial);
                            source.pending_first_frame_event = None;
                        } else {
                            source.pending_first_frame_event = Some((serial, pts));
                        }
                    }
                    let now_for_clear = source.clock.now_secs();
                    if clock::pts_clears_seek_override(pts, now_for_clear)
                        && !source.clock.is_audio_active()
                    {
                        source.clock.set_fallback_anchor(pts);
                        source.clock.clear_seek_target_override(serial);
                    }
                    if crate::perf::is_enabled() {
                        if trace_every_present
                            || total_ms > 4.0
                            || last_present_log.elapsed() > Duration::from_secs(1)
                        {
                            last_present_log = Instant::now();
                            crate::perf::event(
                                "native_presenter",
                                "fullscreen_present",
                                None,
                                0,
                                &[
                                    ("pts", serde_json::Value::from(pts)),
                                    (
                                        "queue_len",
                                        serde_json::Value::from(source.queue.len() as i64),
                                    ),
                                    ("path", serde_json::Value::from(outcome.path)),
                                    (
                                        "shared_handle",
                                        serde_json::Value::from(outcome.shared_handle),
                                    ),
                                    (
                                        "shared_cache_hit",
                                        serde_json::Value::from(outcome.shared_cache_hit),
                                    ),
                                    ("wait_ms", serde_json::Value::from(outcome.wait_ms)),
                                    (
                                        "fence_wait_ms",
                                        serde_json::Value::from(outcome.fence_wait_ms),
                                    ),
                                    (
                                        "open_shared_ms",
                                        serde_json::Value::from(outcome.open_shared_ms),
                                    ),
                                    (
                                        "keyed_mutex_ms",
                                        serde_json::Value::from(outcome.keyed_mutex_ms),
                                    ),
                                    (
                                        "keyed_mutex_cast_ms",
                                        serde_json::Value::from(outcome.keyed_mutex_cast_ms),
                                    ),
                                    (
                                        "keyed_mutex_acquire_ms",
                                        serde_json::Value::from(outcome.keyed_mutex_acquire_ms),
                                    ),
                                    (
                                        "copy_call_ms",
                                        serde_json::Value::from(outcome.copy_call_ms),
                                    ),
                                    ("copy_ms", serde_json::Value::from(outcome.copy_ms)),
                                    (
                                        "present_waitable_ms",
                                        serde_json::Value::from(outcome.present_waitable_ms),
                                    ),
                                    (
                                        "present_call_ms",
                                        serde_json::Value::from(outcome.present_call_ms),
                                    ),
                                    ("present_ms", serde_json::Value::from(outcome.present_ms)),
                                    (
                                        "sync_interval",
                                        serde_json::Value::from(config.sync_interval as i64),
                                    ),
                                    ("total_ms", serde_json::Value::from(total_ms)),
                                    ("source_delta_ms", serde_json::Value::from(source_delta_ms)),
                                ],
                            );
                        }
                    }
                }
                Err(err) => {
                    crate::logger::log(format!("[native-video] present failed: {err}"));
                    native_reset_unpresented_frame(frame);
                    std::thread::sleep(Duration::from_millis(16));
                }
            }
        } else {
            let speed = source.clock.playback_speed().max(clock::MIN_PLAYBACK_SPEED);
            let wait_ms = source
                .queue
                .front()
                .map(|front| (((front.pts_secs - now) / speed) * 500.0).clamp(1.0, 8.0) as u64)
                .unwrap_or(1);
            std::thread::sleep(Duration::from_millis(wait_ms));
        }
    }

    native_drain_unpresented_queue(&mut source.queue);
    source.present_stats.emit_summary(run_started.elapsed());
    crate::logger::log(format!(
        "[native-video] startup probe summary: probes={} first_present_logged={}",
        startup_probe_count, first_present_probe_logged
    ));
    if crate::perf::is_enabled() {
        crate::perf::event(
            "native_presenter",
            "startup_probe_summary",
            None,
            0,
            &[
                (
                    "probes",
                    serde_json::Value::from(startup_probe_count as i64),
                ),
                (
                    "first_present_logged",
                    serde_json::Value::from(first_present_probe_logged),
                ),
            ],
        );
    }
    first_presented_out.store(false, Ordering::Release);
    hwnd_out.store(0, Ordering::Release);
    window.destroy();
    closed.store(true, Ordering::Release);
    crate::logger::log("[native-video] fullscreen presenter stopped".to_string());
    Ok(())
}

#[cfg(windows)]
unsafe impl Sync for GpuLatestFrame {}

/// `future_frames` キューの最大長。decoder の `video_tx` (= 24) と揃える。
/// 1080p RGBA で 24 × ~8MB = 192MB 程度 (CPU 経路の上限)。GPU 経路では
/// 1 frame ≈ HANDLE+メタのみで実コストは無視できる。decoder の burst-stall
/// パターン (~400ms) + HDD random read (~100-300ms) を ~800ms buffer で
/// 吸収して UI tick の空振りを抑える (Phase 8.J)。
pub(crate) const MAX_RENDER_QUEUE: usize = 24;

pub(crate) fn frame_step_interval_secs(avg_fps: f64) -> f64 {
    if avg_fps.is_finite() && avg_fps > 1.0 {
        (1.0 / avg_fps).clamp(1.0 / 240.0, 1.0)
    } else {
        1.0 / 30.0
    }
}

impl VideoPlayer {
    fn repaint_prewake_secs(&self) -> f64 {
        let fps = self.info.as_ref().map(|i| i.avg_fps).unwrap_or(0.0);
        if fps.is_finite() && fps > 1.0 {
            (0.5 / fps).clamp(0.004, 0.020)
        } else {
            0.008
        }
    }

    /// 新しい VideoPlayer を作る。FFmpeg DLL のロードはここで行う (冪等)。
    /// ファイルオープン自体はワーカースレッド内で非同期に行うので、UI スレッドは
    /// ブロックされない。
    ///
    /// `initial_volume` は 0.0-1.5。1.0 超は音声ポンプ側の手動 boost として扱う。
    /// `resume_secs` を指定すると、最初の動画情報受領後に自動的にその位置へシークする。
    /// `hw_decode` が true なら D3D11VA HW デコードを試行 (失敗時は SW にフォールバック)。
    /// VST3 プラグイン処理用の DspBridge は `dsp_bridge` 引数で渡す。
    /// `None` または `is_enabled()=false` なら audio-pump はパススルー。
    /// `is_enabled()=true` のときは pump thread で `bridge.process_block` を呼ぶ。
    pub fn open(
        path: PathBuf,
        initial_volume: f64,
        autoplay: bool,
        resume_secs: Option<f64>,
        hw_decode: bool,
        deinterlace: crate::settings::VideoDeinterlaceMode,
        #[cfg(windows)] gpu_video_device: Option<
            std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
        >,
        #[cfg(windows)] dsp_bridge: Option<std::sync::Arc<crate::video::dsp::DspBridge>>,
        #[cfg(windows)] native_output_config: Option<NativeVideoOutputConfig>,
    ) -> Self {
        // FFmpeg DLL ロード (1 回目のみ実時間の I/O。以降は OnceLock で即返り)
        if let Err(e) = ffmpeg_loader::init() {
            // open 失敗時の dummy engine (Idle のまま)。実 decoder は起きないので、
            // begin_loading は呼ばない (= Phase 3+ で resume 適用も走らない)。
            // 共有 seek_serial を 1 個作り、AvClock と EngineActor 双方に clone を渡す。
            let seek_serial = Arc::new(AtomicU64::new(0));
            let dummy_clock = Arc::new(AvClock::new(initial_volume, seek_serial.clone()));
            let engine = Arc::new(Mutex::new(EngineActor::new(
                OpenOptions {
                    initial_volume,
                    autoplay,
                    resume_secs,
                    ..Default::default()
                },
                seek_serial,
                dummy_clock.clone(),
            )));
            let (engine_event_tx, engine_event_rx) = crossbeam_channel::bounded(64);
            // FFmpeg 初期化失敗時の dummy: engine state は IDLE で固定。
            let engine_state_atomic =
                Arc::new(AtomicU8::new(crate::video::engine::actor::state_code::IDLE));
            return Self {
                path,
                clock: dummy_clock,
                engine,
                engine_state_atomic,
                engine_event_tx,
                engine_event_rx,
                info_event_emitted: false,
                first_frame_event_last_epoch: None,
                displayed_frame_seq: Arc::new(AtomicU64::new(0)),
                last_displayed_pts_bits: Arc::new(AtomicU64::new(f64::NAN.to_bits())),
                decoder_dropped_full_count: Arc::new(AtomicU64::new(0)),
                ui_dropped_past_count: AtomicU64::new(0),
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
                #[cfg(windows)]
                native_output: None,
                #[cfg(windows)]
                duration_secs_bits: Arc::new(AtomicU64::new(0.0_f64.to_bits())),
                #[cfg(windows)]
                native_hover_thumbnail_target_secs: Mutex::new(None),
                #[cfg(windows)]
                native_hover_thumbnail_sent_key: Mutex::new(None),
            };
        }

        // engine event channel: decoder/audio thread が events を push、UI tick が
        // drain して engine.handle_*_event に dispatch する。capacity 64 (= 60fps の
        // ~1 秒分 + audio callback 数件のバッファ余地)。
        let (engine_event_tx, engine_event_rx) = crossbeam_channel::bounded::<EngineEvent>(64);

        // 共有 seek 世代カウンタを 1 個生成し、AvClock と EngineActor の両方に clone を
        // 渡す。これで両者の seek 世代が構造的に常に一致する (= 旧版の「2 つのカウンタを
        // 規律で同期」設計を撤去)。詳細は EngineActor.seek_serial の doc コメント参照。
        let seek_serial = Arc::new(AtomicU64::new(0));
        let clock = Arc::new(AvClock::new(initial_volume, seek_serial.clone()));
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
        let engine = Arc::new(Mutex::new(EngineActor::new(
            opts,
            seek_serial,
            clock.clone(),
        )));
        // begin_loading() を decoder::spawn の **前** に呼ぶ。これにより decoder
        // thread が起動した瞬間から `engine_state_handle` を `Loading` で観察できる
        // (= Idle を一瞬観察する race を排除する)。
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
        // decoder 側から +1)。UI 側 dropped_past は VideoPlayer::tick 内の別 counter。
        let decoder_dropped_full_count = Arc::new(AtomicU64::new(0));

        let decode = decoder::spawn(
            path.clone(),
            clock.clone(),
            cancel.clone(),
            target_rate,
            hw_decode,
            deinterlace,
            #[cfg(windows)]
            gpu_video_device,
            engine_state_handle.clone(),
            engine_event_tx.clone(),
            decoder_dropped_full_count.clone(),
        );

        // 音声出力起動。失敗してもプレイヤーは生きる (映像のみ再生)。
        // 音声を二重に消費するので、decoder の audio_rx を audio.start に渡す。
        // ここで decode.audio_rx を取り出す必要があるので構造体を分解する。
        let DecodeHandles {
            video_rx,
            audio_rx,
            info_rx,
        } = decode;
        #[cfg(windows)]
        let native_video_rx = video_rx.clone();
        let audio = match audio::start(
            audio_rx,
            clock.clone(),
            engine_event_tx.clone(),
            engine_state_handle.clone(),
            #[cfg(windows)]
            dsp_bridge,
        ) {
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
        let displayed_frame_seq = Arc::new(AtomicU64::new(0));
        let last_displayed_pts_bits = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
        #[cfg(windows)]
        let duration_secs_bits = Arc::new(AtomicU64::new(0.0_f64.to_bits()));
        #[cfg(windows)]
        let native_output = native_output_config.and_then(|config| {
            NativeVideoOutput::spawn(
                native_video_rx,
                Arc::clone(&clock),
                engine_event_tx.clone(),
                Arc::clone(&displayed_frame_seq),
                Arc::clone(&last_displayed_pts_bits),
                Arc::clone(&duration_secs_bits),
                config,
            )
        });

        let player = Self {
            path,
            clock,
            engine,
            engine_state_atomic: engine_state_handle,
            engine_event_tx,
            engine_event_rx,
            info_event_emitted: false,
            first_frame_event_last_epoch: None,
            displayed_frame_seq,
            last_displayed_pts_bits,
            #[cfg(windows)]
            duration_secs_bits,
            decoder_dropped_full_count,
            ui_dropped_past_count: AtomicU64::new(0),
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
            #[cfg(windows)]
            native_output,
            #[cfg(windows)]
            native_hover_thumbnail_target_secs: Mutex::new(None),
            #[cfg(windows)]
            native_hover_thumbnail_sent_key: Mutex::new(None),
        };
        crate::logger::log(format!(
            "[video-debug] VideoPlayer::open done path={} autoplay={} volume={:.2} engine_state={} resume_secs={:?} video_rx_len={} audio_rx_len={}",
            player.path.display(),
            autoplay,
            initial_volume,
            player.engine_state_name(),
            player.pending_resume_secs,
            player.decode.video_rx.len(),
            player.decode.audio_rx.len()
        ));
        player
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

    #[cfg(windows)]
    pub fn request_native_hover_thumbnail(&self, target_secs: f64) {
        let target_secs = if target_secs.is_finite() {
            target_secs.max(0.0)
        } else {
            return;
        };
        self.request_seek_thumbnail(target_secs);
        if let Ok(mut target) = self.native_hover_thumbnail_target_secs.lock() {
            let old_bucket = target.map(crate::video::thumbnail::bucket_key);
            let new_bucket = crate::video::thumbnail::bucket_key(target_secs);
            if old_bucket != Some(new_bucket)
                && let Ok(mut sent) = self.native_hover_thumbnail_sent_key.lock()
            {
                *sent = None;
            }
            *target = Some(target_secs);
        }
    }

    /// 直近キャッシュから target_secs に最も近いサムネを取り出す。
    pub fn nearest_seek_thumbnail(&self, target_secs: f64) -> Option<Thumbnail> {
        self.thumb_worker
            .as_ref()
            .and_then(|w| w.nearest(target_secs))
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

    pub fn playback_speed(&self) -> f64 {
        self.clock.playback_speed()
    }

    pub fn set_playback_speed(&self, speed: f64) {
        let speed = clock::clamp_playback_speed(speed);
        self.engine
            .lock()
            .unwrap()
            .apply_command(engine::actor::TransportCommand::SetSpeed { speed });
        self.clock.set_playback_speed(speed);
    }

    /// Phase 9.G: シーク中 (= override 設定中、UI が post-seek 1 枚目を表示する前) か。
    /// perf overlay の graph freeze 判定に使う。
    pub fn is_seeking(&self) -> bool {
        self.clock.is_seeking()
    }

    /// Phase 9.G: perf overlay graph の freeze 判定。pause OR seek 処理中なら true。
    /// `sample_video_perf` と `draw_video_perf_overlay` で同じ条件を見るため
    /// VideoPlayer 側にまとめている (engine_state_atomic load + clock.is_seeking 1 回)。
    pub fn is_paused_or_seeking(&self) -> bool {
        self.engine_state_code() == engine::actor::state_code::PAUSED || self.is_seeking()
    }

    /// Phase 9.C: engine state machine に Play / Pause を伝える。
    /// `toggle_play` / `set_playing` から共有。`apply_command` は idempotent。
    fn dispatch_play_pause(&self, playing: bool) {
        let cmd = if playing {
            engine::actor::TransportCommand::Play
        } else {
            engine::actor::TransportCommand::Pause
        };
        self.engine.lock().unwrap().apply_command(cmd);
    }

    pub fn toggle_play(&self) {
        // EOF で停止中に Space を押されたら 0 から再生し直す (replay)。
        // 通常の再生中は単純トグル。
        if !self.clock.is_playing() && self.clock.is_eof_reached() {
            self.clock.request_seek(0.0);
            self.clear_audio_output_buffer();
            self.clock.set_playing(true);
            // engine 側にも seek を伝えて epoch を同期させる。user 操作の seek は
            // autoplay 強制 (= seek 後に Paused にならないように)。
            //
            // ⚠️ 呼び出し順は handle_seek_request → apply_command(Play):
            // 先に apply_command(Play) を呼ぶと state=Eof なら handle_play が内部で
            // handle_seek_request(0.0) を呼んで epoch++ し、続く明示 handle_seek_request
            // で **epoch が二重 ++** され、decoder からの SeekCompleted{epoch=serial}
            // が stale 判定されて捨てられる。先に handle_seek_request を呼べば
            // state=Seeking{0.0} に遷移し、続く apply_command(Play) は Seeking arm の
            // autoplay=true 設定だけ走る。
            let mut g = self.engine.lock().unwrap();
            g.handle_seek_request(0.0);
            g.apply_command(engine::actor::TransportCommand::Play);
            return;
        }
        // Phase 9.C (2026-04-30): engine state machine も pause/play 同期する。
        // 旧コードは clock.is_playing flag だけ更新していたため、engine state は
        // Playing のまま固定で、perf overlay の warmup 区間表示にも反映されず、
        // EngineState::parks_decoder() が立たないので decoder 側の pause-park 経路と
        // 食い違っていた。
        let new_playing = !self.clock.is_playing();
        self.clock.set_playing(new_playing);
        self.dispatch_play_pause(new_playing);
    }

    pub fn set_playing(&self, p: bool) {
        let prev = self.clock.is_playing();
        crate::logger::log(format!(
            "[video-debug] set_playing({p}) called: prev_playing={prev} engine_state={} seek_serial={} video_rx_len={} audio_rx_len={}",
            self.engine_state_name(),
            self.clock.current_seek_serial(),
            self.decode.video_rx.len(),
            self.decode.audio_rx.len()
        ));
        self.clock.set_playing(p);
        // Phase 9.C: engine 状態も同期。set_playing は外部 API なので呼び出し元が
        // 既に engine.apply_command を呼んでいるケースがあるが、apply_command は
        // idempotent (= 既に Playing で Play を受けても no-op) なので安全。
        if prev != p {
            self.dispatch_play_pause(p);
        }
        crate::logger::log(format!(
            "[video-debug] set_playing({p}) done: engine_state={} playing={} seek_serial={}",
            self.engine_state_name(),
            self.clock.is_playing(),
            self.clock.current_seek_serial()
        ));
    }

    pub(crate) fn clear_audio_output_buffer(&self) {
        if let Some(audio) = &self.audio {
            audio.clear_buffer(&self.clock);
        }
    }

    pub(crate) fn pause_audio_output(&self) {
        if let Some(audio) = &self.audio {
            audio.pause_stream();
        }
    }

    /// 絶対シーク (シークバークリック / ブックマーク等)。
    /// **[`SeekKind::Precise`]**: `..target` のキーフレーム + preroll trim で
    /// target ぴったりに着地。
    /// target は `[0, duration - 0.1s)` にクランプされる。
    /// 一時停止中なら自動的に再生再開する (post-EOF / pause からの seek を
    /// ユーザー操作 1 回で完結させる)。
    pub fn seek(&self, target_secs: f64) {
        let clamped = self.clamp_seek_target(target_secs);
        crate::logger::log(format!(
            "[video-debug] seek({target_secs:.3}) called: clamped={clamped:.3} engine_state={} prev_seek_serial={} playing={} video_rx_len={} audio_rx_len={}",
            self.engine_state_name(),
            self.clock.current_seek_serial(),
            self.clock.is_playing(),
            self.decode.video_rx.len(),
            self.decode.audio_rx.len()
        ));
        self.clock.request_seek(clamped); // = SeekKind::Precise
        self.clear_audio_output_buffer();
        if !self.clock.is_playing() {
            self.clock.set_playing(true);
        }
        // engine の seek_epoch も進めて、AvClock seek_serial と同期させる。
        // user 操作 seek は autoplay 強制 (AvClock 側で playing=true にしているため整合性)。
        // 呼び出し順注意: handle_seek_request → apply_command(Play)
        // (詳細は toggle_play を参照)。
        let mut g = self.engine.lock().unwrap();
        g.handle_seek_request(clamped);
        g.apply_command(engine::actor::TransportCommand::Play);
        drop(g);
        crate::logger::log(format!(
            "[video-debug] seek({target_secs:.3}) dispatched: engine_state={} seek_serial={} playing={} video_rx_len={} audio_rx_len={}",
            self.engine_state_name(),
            self.clock.current_seek_serial(),
            self.clock.is_playing(),
            self.decode.video_rx.len(),
            self.decode.audio_rx.len()
        ));
    }

    /// 相対シーク (←→ ホットキー)。
    /// **[`SeekKind::Fast`]**: keyframe ≤ target に backward seek し、preroll trim
    /// を省略して即時再生開始する。←→ 連打の体感速度を最優先。
    /// 着地位置は target ぴったりではなく keyframe pts (= target - 0〜3 秒程度) で、
    /// 動画 timeline 表示は target を指すが視聴コンテンツは GOP 1 個分だけ先行する形に
    /// なる。0〜3 秒の wall 経過で audio が target に追いつき完全同期する。
    /// 一時停止中なら自動的に再生再開する。
    pub fn seek_relative(&self, delta_secs: f64) {
        let cur = self.position();
        let raw = (cur + delta_secs).max(0.0);
        let target = self.clamp_seek_target(raw);
        self.clock
            .request_seek_with_kind(target, clock::SeekKind::Fast);
        self.clear_audio_output_buffer();
        if !self.clock.is_playing() {
            self.clock.set_playing(true);
        }
        // user 操作 seek は autoplay 強制。
        // 呼び出し順注意: handle_seek_request → apply_command(Play)
        // (詳細は toggle_play を参照)。
        let mut g = self.engine.lock().unwrap();
        g.handle_seek_request(target);
        g.apply_command(engine::actor::TransportCommand::Play);
    }

    /// フレーム送り用の精密シーク。到着後は必ず一時停止状態に保つ。
    pub fn seek_paused(&self, target_secs: f64) {
        let clamped = self.clamp_seek_target(target_secs);
        self.clock.request_seek(clamped);
        self.clear_audio_output_buffer();
        self.clock.set_playing(false);
        let mut g = self.engine.lock().unwrap();
        g.handle_seek_request(clamped);
        g.apply_command(engine::actor::TransportCommand::Pause);
    }

    /// 前後 1 フレームへ移動し、一時停止する。
    pub fn step_frame(&self, direction: i32) {
        if direction == 0 {
            return;
        }
        let base = self.last_displayed_pts().unwrap_or_else(|| self.position());
        let step = frame_step_interval_secs(self.info.as_ref().map(|i| i.avg_fps).unwrap_or(0.0));
        let target = base + step * direction.signum() as f64;
        self.seek_paused(target);
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

    pub fn screenshot_target_secs(&self) -> f64 {
        self.last_displayed_pts().unwrap_or_else(|| self.position())
    }

    fn last_displayed_pts(&self) -> Option<f64> {
        let pts = f64::from_bits(self.last_displayed_pts_bits.load(Ordering::Acquire));
        if pts.is_finite() {
            Some(pts.max(0.0))
        } else {
            None
        }
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

    #[cfg(windows)]
    pub fn native_presenter_hwnd(&self) -> u64 {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::hwnd)
            .unwrap_or(0)
    }

    #[cfg(windows)]
    pub fn native_presenter_pending(&self) -> bool {
        self.native_output
            .as_ref()
            .map(|output| !output.first_presented() && !output.is_closed())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn native_source_epoch(&self) -> Option<u64> {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::source_epoch)
    }

    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn take_native_output(&mut self) -> Option<NativeVideoOutput> {
        self.native_output.take()
    }

    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn attach_native_output(&mut self, output: NativeVideoOutput) {
        self.native_output = Some(output);
    }

    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn build_switch_source_payload(
        &self,
        source_epoch: u64,
        show_preparing_overlay: bool,
    ) -> SwitchSourcePayload {
        SwitchSourcePayload {
            video_rx: self.decode.video_rx.clone(),
            clock: Arc::clone(&self.clock),
            engine_event_tx: self.engine_event_tx.clone(),
            displayed_frame_seq: Arc::clone(&self.displayed_frame_seq),
            last_displayed_pts_bits: Arc::clone(&self.last_displayed_pts_bits),
            duration_secs_bits: Arc::clone(&self.duration_secs_bits),
            source_epoch,
            show_preparing_overlay,
        }
    }

    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn switch_native_source(&self, payload: SwitchSourcePayload) {
        if let Some(output) = self.native_output.as_ref() {
            output.switch_source(payload);
        }
    }

    #[cfg(windows)]
    pub fn native_presenter_closed(&self) -> bool {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::is_closed)
            .unwrap_or(false)
    }

    #[cfg(windows)]
    pub fn set_native_perf_overlay_visible(&self, visible: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_perf_overlay_visible(visible);
        }
    }

    #[cfg(windows)]
    pub fn set_native_hover_thumbnail(
        &self,
        thumbnail: Option<native_presenter::NativeOverlayThumbnail>,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_hover_thumbnail(thumbnail);
        }
    }

    #[cfg(windows)]
    pub fn set_native_hover_preview_pinned(&self, pinned: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_hover_preview_pinned(pinned);
        }
    }

    #[cfg(windows)]
    pub fn set_native_timeline_markers(
        &self,
        markers: Vec<native_presenter::NativeOverlayTimelineMarker>,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_timeline_markers(markers);
        }
    }

    #[cfg(windows)]
    pub fn set_native_jump_entries(&self, entries: Vec<native_presenter::NativeOverlayJumpEntry>) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_jump_entries(entries);
        }
    }

    #[cfg(windows)]
    pub fn set_native_metadata(&self, metadata: Option<native_presenter::NativeOverlayMetadata>) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_metadata(metadata);
        }
    }

    #[cfg(windows)]
    pub fn set_native_loop_enabled(&self, enabled: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_loop_enabled(enabled);
        }
    }

    #[cfg(windows)]
    pub fn set_native_vst3_available(&self, available: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_vst3_available(available);
        }
    }

    #[cfg(windows)]
    pub fn set_native_checked(&self, checked: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_checked(checked);
        }
    }

    #[cfg(windows)]
    pub fn set_native_video_compact(&self, compact: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_video_compact(compact);
        }
    }

    #[cfg(windows)]
    pub fn set_native_vst3_panel(&self, panel: Option<native_presenter::NativeOverlayVst3Panel>) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_vst3_panel(panel);
        }
    }

    #[cfg(windows)]
    pub fn set_native_tile_overlay(
        &self,
        tile_overlay: Option<native_presenter::NativeOverlayTileOverlay>,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_tile_overlay(tile_overlay);
        }
    }

    #[cfg(windows)]
    pub fn set_native_playback_status(&self, first_frame_presented: bool, error: Option<String>) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_playback_status(first_frame_presented, error);
        }
    }

    #[cfg(windows)]
    pub fn show_native_overlay_toast(&self, text: String, centered: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.show_toast(text, centered);
        }
    }

    #[cfg(windows)]
    pub fn drain_native_presenter_events(&self) -> Vec<(u64, NativeVideoOutputEvent)> {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::drain_events)
            .unwrap_or_default()
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
                        #[cfg(windows)]
                        self.duration_secs_bits
                            .store(info.duration_secs.to_bits(), Ordering::Release);
                        // Phase 3c: engine にも InfoReceived event を流す。
                        // resume_secs は **AvClock 経由の旧経路** で処理し続け、
                        // engine 側でも resume_secs を OpenOptions で受領済みなので
                        // engine の InfoReceived ハンドラ内で並行処理される。
                        // (= 二重で seek が走らないよう、Phase 3d で旧経路を撤去する。)
                        if !self.info_event_emitted {
                            // audio output 起動に失敗した場合 (`self.audio.is_none()`)
                            // は has_audio=false で engine に通知する。さもなくば engine が
                            // BufferReady を永久に待ち Buffering で固まる (= audio が決して
                            // 再生されないため audio.rs から BufferReady が出ない)。
                            let has_audio_effective = info.has_audio && self.audio.is_some();
                            let _ = self.engine_event_tx.try_send(EngineEvent::Decoder(
                                engine::state::DecoderEvent::InfoReceived {
                                    epoch: self.engine.lock().unwrap().current_seek_epoch(),
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
                            if resume >= crate::app::VIDEO_RESUME_MIN_POSITION_SECS && !near_end {
                                self.clock.request_seek(resume);
                                // 共有 seek_serial は clock.request_seek で 1 回 bump。
                                // 続く engine.handle_seek_request は adaptive ロジックで
                                // 「外部 bump 検知」となり、自身は bump せず state 更新のみ。
                                // engine の InfoReceived ハンドラ内 resume 経路と二重に
                                // 走ったとしても、後発側は observed (= bump 後) と
                                // last_observed_serial (= 進行済) で外部判定 → 余計な
                                // bump を避ける構造になっている。
                                //
                                // **意図的に apply_command(Play) は呼ばない**:
                                // open-time の resume は user 操作ではなく自動復元
                                // なので、`OpenOptions.autoplay` (= 設定の
                                // video_autoplay) を尊重する。autoplay=false なら
                                // post-seek READY で Paused に遷移する設計。
                                // user 操作の seek/seek_relative/toggle_play は
                                // 別経路で apply_command(Play) を呼ぶ。
                                self.engine.lock().unwrap().handle_seek_request(resume);
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

        #[cfg(windows)]
        self.pump_native_hover_thumbnail();

        #[cfg(windows)]
        if self.native_output.is_some() {
            return if self.is_playing() || self.clock.is_seeking() {
                Some(std::time::Duration::from_millis(16))
            } else {
                None
            };
        }

        // ── 動画フレーム取得・表示判定 ──
        //
        // 設計 (FIFO 連続性を保証):
        //   1. video_rx から取得可能なフレームを `future_frames` キューに push
        //      (キュー上限まで)。channel から取り出したフレームは drop しない。
        //   2. キュー先頭から「pts <= now + 小さな present 見込み余白」のものを順に
        //      latest_renderable に取り出す。最後に残った 1 枚を表示。
        //   3. 最初に出会う「未来フレーム」(pts > now + 小さな余白) で停止し、
        //      next_due = pts - now - 余白 で次 tick を予約。キューに残す。
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
            native_drain_unpresented_queue(&mut self.future_frames);
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
                if let Some(frame) = self.future_frames.pop_front() {
                    native_reset_unpresented_frame(frame);
                }
                dropped_old_serial += 1;
                continue;
            }
            // post-seek 第一フレームは override target で now が凍結するため
            // pts チェックを免除して強制表示する。audio active 時の override 解除は
            // fill_output が実際に post-seek 音声を出した時点で行う。
            // is_seeking() を再確認 (audio が間に override をクリアした場合の stale 防止)。
            let force_display_seek = seek_in_flight_for_display
                && latest_renderable.is_none()
                && front.seek_serial == clock_serial
                && self.clock.is_seeking()
                && clock::pts_clears_seek_override(front.pts_secs, now);
            if force_display_seek || front.pts_secs <= now + lead_tol {
                let frame = self.future_frames.pop_front().unwrap();
                if let Some(previous) = latest_renderable.replace(frame) {
                    native_reset_unpresented_frame(previous);
                    dropped_past += 1;
                }
                continue;
            }
            // 最初の真の未来フレーム → そのまま残し、次 tick を予約。
            // request_repaint_after は厳密なタイマーではないため、表示許容と同じ
            // 小さな margin だけ早めに起こし、displayable 判定側でまだ早ければ残す。
            let speed = self.clock.playback_speed().max(clock::MIN_PLAYBACK_SPEED);
            let until = ((front.pts_secs - now - self.repaint_prewake_secs()).max(0.001) / speed)
                .max(0.001);
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
            if self.loop_enabled.load(std::sync::atomic::Ordering::Acquire) {
                // ループ再生 ON: 先頭にシークし続行 (= 設定の video_loop)。
                self.clock.request_seek(0.0);
                self.clear_audio_output_buffer();
                self.clock.set_playing(true);
                // engine 側の epoch も同期 (= AvClock seek_serial と engine
                // current_seek_epoch の不整合を防ぐ)。loop 周回も autoplay 強制。
                // 呼び出し順注意: handle_seek_request → apply_command(Play)
                // (詳細は toggle_play を参照)。
                let mut g = self.engine.lock().unwrap();
                g.handle_seek_request(0.0);
                g.apply_command(engine::actor::TransportCommand::Play);
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
            match frame.data {
                #[cfg(windows)]
                decoder::VideoFrameData::Gpu(d3d) => {
                    let pts = frame.pts_secs;
                    let serial = frame.seek_serial;
                    let was_none = self.gpu_latest.is_none();
                    if let Some(mut previous) = self.gpu_latest.take() {
                        previous.reset_unpresented_shared_output();
                    }
                    self.gpu_latest = Some(d3d);
                    if was_none {
                        crate::logger::log(format!(
                            "VideoPlayer::tick: GPU frame received and stored in gpu_latest \
                         (pts={pts:.3}, serial={serial})"
                        ));
                    }
                    let now_for_clear = self.clock.now_secs();
                    if clock::pts_clears_seek_override(pts, now_for_clear) {
                        if self.clock.is_audio_active() {
                            if self.clock.is_seeking() && crate::perf::is_enabled() {
                                crate::perf::event(
                                    "video",
                                    "seek_override_wait_audio_gpu",
                                    None,
                                    0,
                                    &[
                                        ("frame_pts", serde_json::Value::from(pts)),
                                        ("now", serde_json::Value::from(now_for_clear)),
                                        ("frame_serial", serde_json::Value::from(serial as i64)),
                                    ],
                                );
                            }
                        } else {
                            self.clock.set_fallback_anchor(pts);
                            self.clock.clear_seek_target_override(serial);
                        }
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
                    self.last_displayed_pts_bits
                        .store(pts.to_bits(), Ordering::Release);
                    self.displayed_frame_seq.fetch_add(1, Ordering::Release);
                    displayed_pts = Some(pts);
                    // GPU frames do not need a CPU texture upload, but they still
                    // must flow through the common dropped_past accounting and
                    // perf event below. Returning here used to hide UI-side frame
                    // batching from perf logs on the D3D11VA path.
                }
                decoder::VideoFrameData::Cpu(b) => {
                    let cpu_bytes = b.as_slice();
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
                        // Audio-active playback must let fill_output clear the override only
                        // when the first audible post-seek samples actually reach the output.
                        // Clearing from video here starts the visual clock before audio is ready
                        // and produces AV drift on high-rate files with deep audio queues.
                        if self.clock.is_audio_active() {
                            if self.clock.is_seeking() && crate::perf::is_enabled() {
                                crate::perf::event(
                                    "video",
                                    "seek_override_wait_audio_cpu",
                                    None,
                                    0,
                                    &[
                                        ("frame_pts", serde_json::Value::from(frame.pts_secs)),
                                        ("now", serde_json::Value::from(now_after)),
                                        (
                                            "frame_serial",
                                            serde_json::Value::from(frame.seek_serial as i64),
                                        ),
                                    ],
                                );
                            }
                        } else {
                            self.clock.set_fallback_anchor(frame.pts_secs);
                            self.clock.clear_seek_target_override(frame.seek_serial);
                        }
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
                                (
                                    "frame_serial",
                                    serde_json::Value::from(frame.seek_serial as i64),
                                ),
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
                            self.texture =
                                Some(ctx.load_texture(label, color, TextureOptions::LINEAR));
                        }
                    }
                    upload_ms = upload_t0.elapsed().as_secs_f64() * 1000.0;
                    self.emit_first_frame_event(pts_for_log);
                    self.last_displayed_pts_bits
                        .store(pts_for_log.to_bits(), Ordering::Release);
                    self.displayed_frame_seq.fetch_add(1, Ordering::Release);
                    displayed_pts = Some(pts_for_log);
                }
            }
        }

        // UI 側 skip 計上: latest_renderable を上書きした際に古い候補を捨てた
        // 累積数 (= dropped_past) を perf overlay 用カウンタに反映。
        if dropped_past > 0 {
            self.ui_dropped_past_count
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
                    (
                        "displayed_pts",
                        serde_json::Value::from(displayed_pts.unwrap_or(f64::NAN)),
                    ),
                    ("lateness_ms", serde_json::Value::from(lateness_ms)),
                    ("pulled", serde_json::Value::from(pulled)),
                    (
                        "dropped_old_serial",
                        serde_json::Value::from(dropped_old_serial),
                    ),
                    ("dropped_past", serde_json::Value::from(dropped_past)),
                    ("upload_ms", serde_json::Value::from(upload_ms)),
                ],
            );
        }

        #[cfg(windows)]
        self.pump_native_hover_thumbnail();

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

    #[cfg(windows)]
    fn pump_native_hover_thumbnail(&self) {
        let Some(output) = self.native_output.as_ref() else {
            return;
        };
        let target_secs = self
            .native_hover_thumbnail_target_secs
            .lock()
            .ok()
            .and_then(|target| *target);
        let Some(target_secs) = target_secs else {
            return;
        };
        let Some(thumb) = self.nearest_seek_thumbnail(target_secs) else {
            self.request_seek_thumbnail(target_secs);
            return;
        };
        let key = NativeHoverThumbnailKey {
            target_bits: thumb.target_secs.to_bits(),
            width: thumb.width,
            height: thumb.height,
            rgba_ptr: Arc::as_ptr(&thumb.rgba) as usize,
        };
        if let Ok(mut sent) = self.native_hover_thumbnail_sent_key.lock() {
            if *sent == Some(key) {
                return;
            }
            *sent = Some(key);
        }
        output.set_hover_thumbnail(Some(native_presenter::NativeOverlayThumbnail {
            target_secs: thumb.target_secs,
            width: thumb.width,
            height: thumb.height,
            rgba: thumb.rgba,
        }));
    }

    /// 表示済フレーム数の累積値。perf overlay が経路 (GPU/CPU) に依存せず
    /// 「新フレームが届いたか」を 1 atomic load で検知できる。
    pub fn displayed_frame_seq(&self) -> u64 {
        self.displayed_frame_seq.load(Ordering::Acquire)
    }

    /// skip された frame の累積数 (decoder 側 video_tx Full + UI 側 dropped_past)。
    /// 互換用の合算値。perf overlay は `skip_counters()` で色分け表示する。
    pub fn skipped_frame_count(&self) -> u64 {
        self.decoder_dropped_full_count.load(Ordering::Acquire)
            + self.ui_dropped_past_count.load(Ordering::Acquire)
    }

    /// skip された frame の累積数を原因別に返す。
    /// `(decoder_dropped_full, ui_dropped_past)`。
    pub fn skip_counters(&self) -> (u64, u64) {
        (
            self.decoder_dropped_full_count.load(Ordering::Acquire),
            self.ui_dropped_past_count.load(Ordering::Acquire),
        )
    }

    /// UI 側 future_frames に並んでいる frame 数 (= 表示待ち バッファ残量)。
    /// perf overlay で skip 発生時のコンテキスト (= starvation か overflow か) を
    /// 見極めるために使う。
    pub fn pending_frames(&self) -> usize {
        self.future_frames.len()
    }

    /// EngineActor の現 state code (Phase 9.B: perf overlay の warmup 区間表示用)。
    /// `Playing` 以外 (= Buffering / Loading / Seeking / Paused / Eof) なら cpal
    /// 出力が silent になっており、UI tick の表示も target 位置で凍結している。
    /// `published_state_handle` 経由なので Mutex を取らない atomic load で済む。
    pub fn engine_state_code(&self) -> u8 {
        self.engine_state_atomic
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn engine_state_name(&self) -> &'static str {
        engine_state_code_name(self.engine_state_code())
    }

    pub fn current_seek_serial(&self) -> u64 {
        self.clock.current_seek_serial()
    }

    pub fn video_rx_len(&self) -> usize {
        self.decode.video_rx.len()
    }

    pub fn audio_rx_len(&self) -> usize {
        self.decode.audio_rx.len()
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
    /// 現象が観測される。先に shutdown() を呼ぶことで、entry を
    /// fs_cache から消す瞬間に音声が止まる。
    pub fn shutdown(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
        #[cfg(windows)]
        {
            if let Some(mut frame) = self.gpu_latest.take() {
                frame.reset_unpresented_shared_output();
            }
            native_drain_unpresented_queue(&mut self.future_frames);
        }
        // AudioOutput を先に drop して cpal stream を止める。
        // Drop で pump も join される。
        self.pause_audio_output();
        self.clear_audio_output_buffer();
        self.audio.take();
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // shutdown() が事前に呼ばれていなければここで stop。
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
        #[cfg(windows)]
        {
            if let Some(mut frame) = self.gpu_latest.take() {
                frame.reset_unpresented_shared_output();
            }
            native_drain_unpresented_queue(&mut self.future_frames);
        }
        self.pause_audio_output();
        self.clear_audio_output_buffer();
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

#[cfg(test)]
mod tests {
    #[test]
    fn frame_step_interval_uses_average_fps() {
        let step = super::frame_step_interval_secs(60.0);
        assert!((step - (1.0 / 60.0)).abs() < 1.0e-9);
    }

    #[test]
    fn frame_step_interval_falls_back_to_30fps() {
        let step = super::frame_step_interval_secs(0.0);
        assert!((step - (1.0 / 30.0)).abs() < 1.0e-9);
    }
}
