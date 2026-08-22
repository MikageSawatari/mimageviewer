//! 動画インライン再生 (FFmpeg LGPL DLL バックエンド)。
//!
//! `GridItem::Video` をフルスクリーンで開いたときに [`VideoPlayer`] を生成し、
//! 1 動画 = 1 プレイヤーとして所有する。プレイヤーは内部に:
//!
//! - **デコーダワーカー** (`std::thread`、bounded mpsc で UI に video/audio フレーム送出)
//! - **音声出力** (`cpal` Stream)
//! - **AV マスタークロック** ([`clock::AvClock`])
//!
//! を持つ。UI スレッドは [`VideoPlayer::tick`] を毎フレーム呼ぶが、フレームの実体描画は
//! native presenter (`crate::video::native_presenter`、独立 Win32 HWND + D3D11 swap chain) が
//! デコーダ出力を直接受け取って行う。`tick` は再生制御ステート / ホバーサムネイル要求 /
//! repaint hint だけを扱う。
//!
//! ## 配布要件
//! `vendor/ffmpeg/bin/*.dll` (BtbN LGPL shared build、6 DLL) は配布用 launcher
//! (`crates/launcher/`) が `include_bytes!` で内包し、初回起動時に
//! `%APPDATA%/mimageviewer/runtime/<version>/` へ展開してから本体 (`mimageviewer-core.exe`)
//! を spawn する。本モジュールの [`ffmpeg_loader::init`] は exe と同じディレクトリに
//! DLL が存在するかをログで確認するだけで、ロード自体は Windows ローダが行う
//! (詳細は CLAUDE.md「FFmpeg LGPL DLL 管理」節)。VC++ 再頒布可能パッケージ非依存。
//!
//! ## ライセンス
//! FFmpeg LGPLv3-or-later build。動的リンク + ソフトウェア情報への通知 + ソース提供
//! (mikage.to に tarball 配置) で MIT ライセンスの mIV と共存可能。詳細は
//! CLAUDE.md の「FFmpeg ライセンス対応」節を参照。

pub mod audio;
pub mod audio_diagnostics;
pub mod audio_stretch;
pub mod avio_progress;
pub mod clock;
pub mod clockless_transcode;
pub mod decoder;
pub mod display_metadata;
#[cfg(windows)]
pub mod dsp;
pub mod engine;
pub mod ffmpeg_loader;
mod frame_selection;
#[cfg(windows)]
pub mod gpu_renderer;
#[cfg(windows)]
pub(crate) mod native_cursor;
#[cfg(windows)]
pub mod native_presenter;
pub(crate) mod native_touch;
#[cfg(windows)]
pub mod native_window;
#[cfg(windows)]
pub(crate) mod native_window_health;
#[cfg(windows)]
pub(crate) mod native_window_host;
#[cfg(windows)]
mod native_window_pump;
#[cfg(all(test, windows))]
mod native_window_thread_spike;
pub mod normalize_scanner;
pub mod normalize_types;
pub mod screenshot;
pub(crate) mod stream;
pub mod swscale_helpers;
pub mod thumbnail;
pub mod tile_thumb_cache;
pub mod tile_thumbnails;
pub mod upscale;
#[allow(dead_code)]
pub(crate) mod window_host_contract;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use clock::AvClock;
use decoder::{DecodeHandles, VideoFrame, VideoFrameData, VideoInfo};
use thumbnail::{Thumbnail, ThumbnailWorker};

use std::sync::Mutex;

use engine::EngineEvent;
use engine::actor::{EngineActor, OpenOptions};

/// Selects the owner that consumes the decoder's normal video output.
/// Remote-only players do not own a display, so a dedicated worker drains their queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoOutputConsumer {
    Presentation,
    RemoteHeadless,
}

/// Inputs frozen at the boundary between the paused remote metadata player and a clockless
/// streaming generation.
///
/// The generation must use this snapshot rather than consulting the player's transport later:
/// the browser owns play intent, while the metadata player deliberately remains paused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RemoteStreamStartInputs {
    pub(crate) duration_secs: f64,
    pub(crate) has_video: bool,
    pub(crate) has_audio: bool,
    pub(crate) source_origin_secs: f64,
    pub(crate) normalize_gain: f64,
}

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

/// duration が判明した時点で open-time resume を安全な途中位置だけに絞る。
/// コンテナ情報到着後に呼び、末端 guard 内なら先頭から再生する。
pub(crate) fn sanitize_resume_for_duration(resume: Option<f64>, duration: f64) -> Option<f64> {
    let resume = resume?;
    let near_end = duration > 0.0 && resume >= duration - crate::app::VIDEO_RESUME_END_GUARD_SECS;
    (resume.is_finite() && resume >= crate::app::VIDEO_RESUME_MIN_POSITION_SECS && !near_end)
        .then_some(resume)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VideoContinuousMode {
    #[default]
    Off,
    Continuous,
    ContinuousLoop,
}

impl VideoContinuousMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Continuous,
            Self::Continuous => Self::ContinuousLoop,
            Self::ContinuousLoop => Self::Off,
        }
    }

    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn wraps(self) -> bool {
        matches!(self, Self::ContinuousLoop)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "連続再生: OFF",
            Self::Continuous => "連続再生",
            Self::ContinuousLoop => "連続再生 + ループ",
        }
    }
}

pub struct VideoPlayer {
    path: PathBuf,
    clock: Arc<AvClock>,
    /// 動画再生の state machine actor。state (Loading / Buffering / Playing / Paused /
    /// EOF) の source of truth は本 actor。tick で `engine_event_rx` を drain して
    /// `apply_command` を呼ぶ。再生クロック (current_secs / wall extrapolation) は
    /// 引き続き `AvClock` 側が持つ。
    engine: Arc<Mutex<EngineActor>>,
    /// EngineActor の `published_state` を Mutex なしで読むための clone。perf overlay
    /// が warmup 区間 (= state ≠ Playing) を表示するときに使う。
    engine_state_atomic: Arc<AtomicU8>,
    /// decoder/audio thread から push される events を tick で drain して engine に
    /// dispatch する。capacity 64 (= burst tolerance、drop 不可なので unbounded 寄りの bounded)。
    engine_event_rx: crossbeam_channel::Receiver<EngineEvent>,
    /// 同 channel の sender (decoder/audio に clone して渡す)。
    /// VideoPlayer 自身が `tick` 内で UI thread からも push する経路で保持。
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
    /// フレーム送りで最後に基準にした表示 PTS。repeat で次 target を出すとき、
    /// 表示済み PTS がまだ追いついていなくても発行済み base を基準にできる。
    frame_step_base_bits: AtomicU64,
    /// フレーム送り操作による一時停止中なら true。通常 pause の中央再生 UI と分ける。
    frame_step_active: Arc<AtomicBool>,
    /// 最後の frame-step seek 発行時点での表示済みフレーム sequence。
    /// 長押し repeat はこの値から `displayed_frame_seq` が進むまで次 target を出さない。
    frame_step_issued_display_seq: AtomicU64,
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
    /// Owns the selected decoder output consumer lifecycle as one typed state.
    video_output: VideoOutputState,
    /// 保持目的に加え、Remote streaming session が audio tap controller を取得する。
    audio: Option<audio::AudioOutput>,
    info: Option<VideoInfo>,
    /// open 失敗 / DLL ロード失敗のメッセージ。Some なら UI は赤字エラー表示する。
    error: Option<String>,
    /// シーク先サムネ抽出ワーカー。Drop で停止する。
    thumb_worker: Option<ThumbnailWorker>,
    /// Remote seek/drag preview currently requested by the browser. Marker warmup shares the
    /// thumbnail worker, so it must not replace an unsatisfied interactive remote request.
    remote_seek_thumbnail_request: Mutex<Option<SeekThumbnailRequest>>,
    /// 未来フレーム (pts > clock now) のキュー。channel から pull した順に末尾に push、
    /// front から `pts <= now + small_margin` のものを取り出して表示。FIFO 連続性を保つことで
    /// 高 fps コンテンツでも display が channel head の far-future にジャンプしない。
    future_frames: std::collections::VecDeque<VideoFrame>,
    /// 起動時 resume 用の保留シーク target (秒)。info 到着後に 1 度だけ実行する。
    pending_resume_secs: Option<f64>,
    /// 直前 tick で観測した seek_serial。新世代に変わったら future_frames を一掃する。
    last_seen_seek_serial: u64,
    /// EOF 到達時に先頭から再生し直すか (= 設定の video_loop_mode != Off の effective)。
    /// App が `set_loop_enabled` で更新する。`true` のとき EOF 経路は
    /// `loop_target_bits` を seek 先として使う。
    loop_enabled: AtomicBool,
    /// EOF 到達時 (および将来的にチャプター/ブックマーク境界 tick から要求された seek 用)
    /// の seek 先 (秒、`f64::to_bits`)。Full ループでは `0.0` 固定だが、CH/BM ループでは
    /// app 側で「現区間の開始秒」を書き戻す。
    loop_target_bits: AtomicU64,
    /// EOF ループ発火前の「drain 完了」連続観測カウンタ。tick 毎に audio buffer 群
    /// (processed / raw_pending / tx_queued) が全て quiet 閾値未満で、かつ rx channel が
    /// 両方空なら +1。1 つでも条件破りで 0 にリセット。指定 tick 数連続で観測したら
    /// pump handoff race (= 一瞬すべてが 0 になる) を吸収済みとみなしてループ seek 発火。
    eof_loop_quiet_ticks: AtomicU32,
    /// 現在進行中のシークが開始された壁時計時刻。シーク中は UI tick が短周期で
    /// repaint を予約してデコーダ完成を polling 待ちする。長引いたら back off する。
    /// シーク完了 (override が 1 度クリア) で None に戻す。
    seek_inflight_since: Option<std::time::Instant>,
    /// 「seek が EOF に達したまま完了しない」状態を最初に観測した壁時計時刻。
    /// `is_seeking() && is_eof_reached()` が継続して true の間だけ `Some`。
    /// `info.duration_secs` (コンテナ尺) が最終フレーム PTS より後ろのことが多く、
    /// その付近を target にした seek は backward seek 自体は成功する (= seek 失敗
    /// 経路の override clear が走らない) のに、video decoder が target 以降の
    /// フレームを 1 枚も返せず post-seek frame / 音声による override clear 経路も
    /// 発火しない。結果 `seek_target_override` が固着して「シーク中...」が出続ける。
    /// この時刻から一定時間 (`SEEK_STUCK_EOF_TIMEOUT`) 経過しても解除されなければ、
    /// tick 側の保険として override を強制クリアする。`is_eof_reached()` は demux が
    /// ファイル全体を読み切ったときだけ true (request_seek で一旦クリア) なので、
    /// 進行中の通常 seek を誤検出しない。
    /// (詳細は [docs/video-architecture.md] の seek HUD 節を参照。)
    seek_eof_stuck_since: Option<std::time::Instant>,
    /// キーリピート / seekbar drag のような連続ユーザー seek を coalesce する。
    /// frame-step は専用の latest-wins gate を持つのでここには混ぜない。
    user_seek_coalesce: Mutex<UserSeekCoalesceState>,
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
    native_hover_thumbnail_request: Mutex<Option<SeekThumbnailRequest>>,
    #[cfg(windows)]
    native_hover_thumbnail_sent_key: Mutex<Option<NativeHoverThumbnailKey>>,
    /// decoder thread / native presenter thread / UI と共有する動的状態。
    /// `open` で 1 度生成し、`build_switch_source_payload` で fast-swap 経路に
    /// 渡すために保持する (= 新ソース open 時に作った Arc を旧 presenter に
    /// 引き継ぐ)。
    #[cfg(windows)]
    dynamic: Arc<crate::video::decoder::VideoDynamicState>,
    /// A/V sync drift デバッグ用の atomic bundle。audio.rs (cpal callback / pump) と
    /// native presenter (present 経路 + overlay 描画) で同じ Arc を共有する。
    /// fast-swap でも `build_switch_source_payload` で同じ Arc が引き継がれる。
    /// 詳細は `src/video/audio_diagnostics.rs` の doc コメント参照。
    audio_diagnostics: Arc<crate::video::audio_diagnostics::AudioDiagnostics>,
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeHoverThumbnailKey {
    target_bits: u64,
    width: u32,
    height: u32,
    rgba_ptr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SeekThumbnailRequest {
    target_secs: f64,
    tolerance_secs: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeVideoPlacement {
    MainWindowChild,
    FullscreenBorderless,
    DetachedViewerChild,
    DetachedWindow,
}

impl NativeVideoPlacement {
    pub fn is_main_window_child(&self) -> bool {
        matches!(self, Self::MainWindowChild)
    }

    pub fn is_child_window(&self) -> bool {
        matches!(self, Self::MainWindowChild | Self::DetachedViewerChild)
    }

    pub fn is_fullscreen_borderless(&self) -> bool {
        matches!(self, Self::FullscreenBorderless)
    }

    /// detached viewer (F12 別ウィンドウ) の子として重ねる placement か。
    /// pump はこの分類を parent/client rect と host registry の選択に使う。window 単位の
    /// destroy は pump lifetime を終わらせず、終了は typed Shutdown が所有する。
    pub fn is_detached_viewer_child(&self) -> bool {
        matches!(self, Self::DetachedViewerChild)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::MainWindowChild => "main-window-child",
            Self::FullscreenBorderless => "fullscreen-borderless",
            Self::DetachedViewerChild => "detached-viewer-child",
            Self::DetachedWindow => "detached-window",
        }
    }
}

#[cfg(windows)]
fn native_child_should_set_focus(placement: NativeVideoPlacement, activate_on_show: bool) -> bool {
    placement.is_child_window() && activate_on_show
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct NativeVideoOutputConfig {
    pub rect: windows::Win32::Foundation::RECT,
    pub owner_hwnd: u64,
    pub fallback_file_name: String,
    pub sync_interval: u32,
    pub perf_overlay_visible: bool,
    pub initial_tile_overlay: bool,
    pub vst3_available: bool,
    pub checked: bool,
    pub cursor_hide_delay_secs: f32,
    /// main egui Context の zoom_factor を native overlay にミラーする倍率。
    pub ui_scale: f32,
    /// main egui Context と native HUD で共有する文字コントラスト。
    pub text_contrast: crate::settings::TextContrast,
    /// main egui Context と native HUD で共有する UI フォント。
    pub ui_font: crate::settings::UiFontSettings,
    /// Viewer-wide tone controls and registered Creative LUT snapshot.
    pub video_grade: crate::creative_lut::VideoGradeSnapshot,
    /// Startup-selected native video scaling owner.
    pub scale_filter: crate::settings::VideoScaleFilter,
    /// CP7: HUD raise の allowlist 判定 (`foreground_allows_hud_raise`) で参照する
    /// VST editor container HWND の snapshot。App が `dsp_bridge.editor_hwnds_snapshot()` を
    /// 渡す。`None` のとき HUD HWND を作っても raise 判定で常に false (= raise 起動しない)
    /// になるが、`SetWindowRgn` 経由の click-through は機能する。
    pub editor_hwnds_snapshot:
        Option<std::sync::Arc<std::sync::RwLock<std::collections::HashSet<u64>>>>,
    /// CP7: `foreground_allows_hud_raise` 判定用の main HWND (mIV メインウィンドウ)。
    /// 0 だと「mIV 既知 HWND」判定で main HWND が許可されなくなる (= presenter / HUD のみ
    /// 許可)。App が `self.main_hwnd` を渡す。
    pub main_hwnd_for_raise: u64,
    /// CP7: HUD overlay HWND を有効化するか。`false` のとき従来の presenter HWND DComp tree
    /// に egui overlay を載せるフォールバック経路 (= CP4-6 の動作と等価)。
    /// `true` で HUD HWND を作成し、VST より前面に bars を出す。万が一の regression に備えて
    /// 環境変数 `MIV_HUD_OVERLAY=0` で強制 off できるよう App 側で配線する。
    pub hud_overlay_enabled: bool,
    pub placement: NativeVideoPlacement,
    pub activate_on_show: bool,
    /// The presenter starts in the same visibility state as its owning app surface.
    /// Tray residency can create or reattach an output while the root window is hidden;
    /// creating that HWND visible would briefly escape the tray lifecycle before the App
    /// can enqueue a follow-up command.
    pub initial_visibility: NativeVideoInitialVisibility,
    /// Phase 0 spike: `true` のとき presenter HWND を `owner_hwnd` の子
    /// (`WS_CHILD`) として生成し、owner のクライアント領域に重ねて in-window
    /// 再生する。`false` のとき従来どおりモニタ全面の borderless popup。
    pub in_main_window: bool,
    /// 音声のみ (映像トラック無し) のプレイヤーに native presenter を attach する
    /// モード (music VST シェル、Inc 6 ②-1)。`true` のとき present ループは
    /// **フレームが永久に来ない前提**で回る:
    /// - `first_presented` を startup 直後に立てて `native_presenter_pending()` が
    ///   「準備中」で永久固着しないようにする (映像 first frame を待たない)。
    /// - `waiting_for_first_frame` を常に false 扱いにする。
    /// - フレーム待ちアイドルの sleep を frame pacing 用 1ms でなく HUD periodic
    ///   tick に十分な間隔にして無駄なスピンを避ける。
    /// `false` のとき従来の動画経路と**バイト等価**。
    pub audio_only: bool,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeVideoInitialVisibility {
    Visible,
    Hidden,
}

#[cfg(any(windows, test))]
impl NativeVideoInitialVisibility {
    fn is_visible(self) -> bool {
        matches!(self, Self::Visible)
    }
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub enum NativeVideoOutputEvent {
    Window(native_window::NativeVideoWindowEvent),
    /// Latest presenter-side input ownership snapshot. This is observation
    /// state, not an App command, so `NativeVideoOutput::drain_events` consumes
    /// it into the output-local snapshot before returning semantic events.
    OverlayInputRouting(native_presenter::NativeOverlayInputRouting),
    Seek {
        target_secs: f64,
    },
    SeekRelative {
        delta_secs: f64,
    },
    TouchChromeLearned,
    TileSeek {
        target_secs: f64,
    },
    NavigateItem {
        delta: i32,
        via_wheel: bool,
    },
    TileColumnsDelta {
        delta: i32,
    },
    RequestSeekThumbnail {
        target_secs: f64,
        bar_width_points: f64,
        pixels_per_point: f64,
    },
    /// hover が外れて hover thumbnail 要求がもう不要 (T35)。
    ClearSeekThumbnail,
    ToggleTileMode,
    TogglePerfOverlay,
    ToggleSidePanelMode,
    ToggleClickInfoOpen,
    OpenTouchInfoPanel,
    DismissTouchSidePanels,
    ToggleVst3Gui,
    /// 動画 HUD の「音声モード」ボタン (Inc 7、動画→音声モード)。App が `enter_video_audio_mode`
    /// を呼ぶ (映像を切って音楽ビューへ、音声無中断)。
    ToggleAudioMode,
    /// フルスクリーン終了要求。`generation` は要求を出した presenter placement 世代。
    /// placement switch 直後に旧世代由来で遅れて届く close を App 側が棄却するために使う。
    CloseFullscreen {
        generation: u64,
    },
    /// 動画 HUD のトグルボタン: ウィンドウ内再生 ⇔ 全画面 を切り替える。
    ToggleWindowMode,
    PlacementSwitched {
        request_id: u64,
        placement: NativeVideoPlacement,
        /// 切替後 (= 現在 live な) presenter window の placement 世代。App は
        /// `committed_generation` をこの値まで進め、旧世代 close を棄却する。
        generation: u64,
    },
    PlacementSwitchFailed {
        request_id: u64,
    },
    SetVst3PanelVisible {
        visible: bool,
    },
    SetVst3VideoCompact {
        compact: bool,
    },
    SetVst3PanelPos {
        pos: [f32; 2],
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
    VideoAdjustLoadSlot {
        slot_idx: usize,
    },
    VideoAdjustSaveSlot {
        slot_idx: usize,
    },
    SeekToStartAndPlay,
    TogglePlay,
    ToggleMute,
    ToggleLoop,
    ToggleContinuous,
    SetVolume {
        volume: f64,
        persist: bool,
    },
    SetVideoAdjustments {
        adjustments: crate::creative_lut::VideoAdjustments,
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
    SetPinAt {
        target_secs: f64,
    },
    /// 動画 HUD 2 段化リデザイン (Phase 4): 前/次マーカーへジャンプ。
    /// `next=true` で次マーカー (= K キー)、`false` で前マーカー (= J キー)。
    JumpMarker {
        next: bool,
    },
    /// 動画 HUD 2 段化リデザイン (Phase 5): 現在フレームをキャプチャ保存フォルダへ保存
    /// (= Ctrl+S と等価)。
    SaveFrameToFile,
    SetBookmarkTitle {
        id: i64,
        title: String,
    },
    DeleteBookmark {
        id: i64,
    },
    DeletePin,
    /// 一括ブックマーク登録 (YouTube コメント形式のチャプター列)。
    /// `(pts_secs, title)` の Vec。タイトルが空文字なら NULL 保存。
    BulkAddBookmarks {
        entries: Vec<(f64, String)>,
    },
    /// 現在再生中の動画のブックマーク一覧をクリップボードへコピー。
    /// `seconds_only` が true なら整数秒へ floor、false なら小数 3 桁 (ms 精度) で出力。
    ExportBookmarksToClipboard {
        seconds_only: bool,
    },
    /// 現在再生中の動画のブックマークを全削除。
    ClearAllBookmarksForCurrent,
    OpenExternalUrl {
        url: String,
    },
    /// 右パネル先頭の★行クリック。解決済みの新レーティング (0..=5)。
    SetRating {
        stars: u8,
    },
    ToggleTag {
        name: String,
    },
    AddTag {
        name: String,
    },
    RemoveTag {
        name: String,
    },
    OpenTagViewForTag {
        name: String,
    },
    /// 音量ノーマライズボタン左クリック (3 状態モデル: Off → ON 化 / OnApplied → OFF 化 /
    /// OnUnmeasured → スキャン起動)。詳細は `App::handle_toggle_normalize`。
    ToggleNormalize,
    /// 音量ノーマライズボタン右クリック (どの状態からでもグローバル OFF 化、救済経路)。
    DisableNormalize,
    /// スキャン中の進捗パネル × ボタン or ESC でキャンセル。
    CancelNormalizeScan,
}

#[cfg(windows)]
const OUTPUT_EVENT_LATEST_MOUSE_MOVE: usize = 0;
#[cfg(windows)]
const OUTPUT_EVENT_LATEST_SEEK_THUMBNAIL: usize = 1;
#[cfg(windows)]
const OUTPUT_EVENT_LATEST_VST_PANEL_POS: usize = 2;
#[cfg(windows)]
const OUTPUT_EVENT_LATEST_OVERLAY_INPUT_ROUTING: usize = 3;
#[cfg(windows)]
const OUTPUT_EVENT_LATEST_SLOTS: usize = 4;

#[cfg(windows)]
fn native_output_event_latest_slot(event: &NativeVideoOutputEvent) -> Option<usize> {
    match event {
        NativeVideoOutputEvent::Window(native_window::NativeVideoWindowEvent::MouseMove(_)) => {
            Some(OUTPUT_EVENT_LATEST_MOUSE_MOVE)
        }
        NativeVideoOutputEvent::RequestSeekThumbnail { .. } => {
            Some(OUTPUT_EVENT_LATEST_SEEK_THUMBNAIL)
        }
        NativeVideoOutputEvent::SetVst3PanelPos { .. } => Some(OUTPUT_EVENT_LATEST_VST_PANEL_POS),
        NativeVideoOutputEvent::OverlayInputRouting(_) => {
            Some(OUTPUT_EVENT_LATEST_OVERLAY_INPUT_ROUTING)
        }
        _ => None,
    }
}

#[cfg(windows)]
struct SequencedNativeOutputEvent {
    sequence: u64,
    source_epoch: u64,
    event: NativeVideoOutputEvent,
}

#[cfg(windows)]
struct NativeOutputEventBusShared {
    next_sequence: AtomicU64,
    latest: Mutex<Vec<Option<SequencedNativeOutputEvent>>>,
    overflow_fault: Arc<AtomicBool>,
}

#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct NativeOutputEventSender {
    lossless_tx: crossbeam_channel::Sender<SequencedNativeOutputEvent>,
    shared: Arc<NativeOutputEventBusShared>,
}

#[cfg(windows)]
struct NativeOutputEventReceiver {
    lossless_rx: crossbeam_channel::Receiver<SequencedNativeOutputEvent>,
    shared: Arc<NativeOutputEventBusShared>,
}

#[cfg(windows)]
fn native_output_event_bus(
    capacity: usize,
    overflow_fault: Arc<AtomicBool>,
) -> (NativeOutputEventSender, NativeOutputEventReceiver) {
    let (lossless_tx, lossless_rx) = crossbeam_channel::bounded(capacity);
    let shared = Arc::new(NativeOutputEventBusShared {
        next_sequence: AtomicU64::new(1),
        latest: Mutex::new((0..OUTPUT_EVENT_LATEST_SLOTS).map(|_| None).collect()),
        overflow_fault,
    });
    (
        NativeOutputEventSender {
            lossless_tx,
            shared: Arc::clone(&shared),
        },
        NativeOutputEventReceiver {
            lossless_rx,
            shared,
        },
    )
}

#[cfg(windows)]
impl NativeOutputEventSender {
    pub(crate) fn send(&self, source_epoch: u64, event: NativeVideoOutputEvent) {
        let sequence = self.shared.next_sequence.fetch_add(1, Ordering::Relaxed);
        let queued = SequencedNativeOutputEvent {
            sequence,
            source_epoch,
            event,
        };
        if let Some(slot) = native_output_event_latest_slot(&queued.event) {
            match self.shared.latest.lock() {
                Ok(mut latest) => latest[slot] = Some(queued),
                Err(_) => self.shared.overflow_fault.store(true, Ordering::Release),
            }
            return;
        }
        match self.lossless_tx.try_send(queued) {
            Ok(()) => {}
            Err(crossbeam_channel::TrySendError::Full(_))
            | Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                self.shared.overflow_fault.store(true, Ordering::Release)
            }
        }
    }
}

#[cfg(windows)]
impl NativeOutputEventReceiver {
    fn drain(&self) -> Vec<(u64, NativeVideoOutputEvent)> {
        let mut events: Vec<_> = self.lossless_rx.try_iter().collect();
        match self.shared.latest.lock() {
            Ok(mut latest) => events.extend(latest.iter_mut().filter_map(Option::take)),
            Err(_) => self.shared.overflow_fault.store(true, Ordering::Release),
        }
        events.sort_unstable_by_key(|event| event.sequence);
        events
            .into_iter()
            .map(|event| (event.source_epoch, event.event))
            .collect()
    }
}

/// [`VideoPlayer::seek_relative`] の結果。
/// 境界 (先頭 / 末尾) に達して実シークを発行しなかったケースを呼び出し側が
/// 検出し、トースト表示に振り替えるために使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativeSeekOutcome {
    /// 実シークを発行した (通常)。
    Seeked,
    /// 既に動画先頭に居て、これ以上戻れなかった。
    AtStart,
    /// 既に動画末尾に居て、これ以上進めなかった。
    AtEnd,
}

#[cfg(windows)]
pub(crate) struct SwitchSourcePayload {
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
    frame_step_active: Arc<AtomicBool>,
    duration_secs_bits: Arc<AtomicU64>,
    /// 新ソースの decoder/UI と共有する動的状態 (per-frame プレゼン経路 /
    /// デインターレース)。fast-swap 経路でも presenter の present_stats が
    /// 同じ Arc を握り直す。
    dynamic: Arc<crate::video::decoder::VideoDynamicState>,
    /// A/V sync drift デバッグ用 atomic bundle。`VideoPlayer.audio_diagnostics` と
    /// 同じ Arc を渡す (fast-swap 経路でも引き継がれる)。
    audio_diagnostics: Arc<crate::video::audio_diagnostics::AudioDiagnostics>,
    source_epoch: u64,
    fallback_file_name: String,
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
    SetVideoGrade {
        grade: crate::creative_lut::VideoGradeSnapshot,
    },
    SetMetadata {
        metadata: Option<native_presenter::NativeOverlayMetadata>,
    },
    SetSidePanelState {
        mode: crate::settings::FsSidePanelMode,
        info_panel_open: crate::ui_helpers::MetadataPanelOpenState,
    },
    ResetSidePanelSession,
    SetLoopEnabled {
        enabled: bool,
    },
    /// HUD ボタン表示用のループモード (= ユーザー設定の video_loop_mode、display mode)。
    /// 再生挙動には `SetLoopEnabled` の bool を使う (= effective mode から導出)。
    /// 「BM モード設定 + BM 無し動画」のとき、ボタン表示は BM のまま、挙動は Full と等価。
    SetLoopMode {
        mode: crate::settings::VideoLoopMode,
    },
    SetContinuousMode {
        mode: VideoContinuousMode,
    },
    SetVst3Available {
        available: bool,
    },
    SetHudDimmed {
        dimmed: bool,
    },
    SetTextContrast {
        contrast: crate::settings::TextContrast,
    },
    SetChecked {
        checked: bool,
    },
    SetVideoCompact {
        compact: bool,
    },
    SetVideoGeometry {
        num: u32,
        den: u32,
        orientation: display_metadata::VideoOrientation,
    },
    SetVst3Panel {
        panel: Option<native_presenter::NativeOverlayVst3Panel>,
    },
    SetPlaybackStatus {
        first_frame_presented: bool,
        error: Option<String>,
        /// 動画オープン中の進捗 (`first_frame_presented = false` の間だけ HUD に出る)。
        prep_status: avio_progress::PreparingStatus,
    },
    ShowToast {
        text: String,
        centered: bool,
        /// 表示維持時間。`None` のとき presenter 側が `centered` から既定値
        /// (centered: 2.5s / それ以外: 1.8s) を導く。←→ ホットキーの境界トーストの
        /// ように「キーを離したら早めに消したい」用途では `Some(短い値)` を渡す。
        linger: Option<std::time::Duration>,
    },
    SetTileOverlay {
        tile_overlay: Option<native_presenter::NativeOverlayTileOverlay>,
    },
    SetRingPickerOverlay {
        overlay: Option<native_presenter::NativeOverlayRingPicker>,
    },
    SetRingGuideOverlay {
        overlay: Option<native_presenter::NativeOverlayRingGuide>,
    },
    // SetNavigationPreview/SwitchSource は `app/` 経由でのみ構築される (= bin 専属の
    // `app` 経路。lib.rs では `app` が stub のため lib 視点では dead variant)。
    #[allow(dead_code)]
    SetNavigationPreview {
        preview: Option<native_presenter::NativeOverlayNavigationPreview>,
    },
    #[allow(dead_code)]
    SwitchSource {
        payload: Box<SwitchSourcePayload>,
    },
    #[allow(dead_code)]
    SwitchPlacement {
        request_id: u64,
        placement: NativeVideoPlacement,
        owner_hwnd: u64,
        rect: windows::Win32::Foundation::RECT,
        activate_on_show: bool,
        visible: bool,
    },
    /// native presenter HWND の `push_native_event` を経由しない pointer 活動を
    /// 伝搬する。NativeEguiOverlay の cursor auto-hide タイマをリセットして
    /// 即時にカーソルを再表示するため。
    MarkCursorActivity,
    /// キー操作後の HUD 更新など、カーソル auto-hide タイマには触れずに
    /// native overlay の再描画だけを要求する。
    RequestOverlayRender,
    /// 音量ノーマライズの UI 状態 + 進捗 snapshot を native overlay に配信する。
    /// App 側 `update` で normalize_ui_states と normalize_state.progress を読んで
    /// `NormalizeOverlayState` を作り、毎フレーム送る。overlay 側はボタン色 +
    /// (Scanning 中) 進捗パネルの描画に使う。
    SetNormalizeOverlayState {
        state: crate::video::normalize_types::NormalizeOverlayState,
    },
    /// HUD overlay HWND を VST GUI より前面に上げ直す。VST z-order 操作後の hook、
    /// render の cursor observation 評価 (activation zone 検知)、HUD wndproc の
    /// `WM_WINDOWPOSCHANGING` などから発火される。pump が pop して
    /// 即時 → 16ms → 64ms の short retry burst で `SetWindowPos(hud, HWND_TOPMOST, ...)`
    /// を呼ぶ (VST IPC が非同期で z-order を動かしても拾えるように)。
    RaiseHudToTop,
    /// native presenter HWND を pump thread 側で前面・foreground に戻す。
    ///
    /// PrintScreen / Snipping Tool などで一度 foreground が外部プロセスへ移ったあと、
    /// egui 側の黒 backdrop が presenter HWND より前に残ることがある。UI thread から
    /// `SetWindowPos` / `SetForegroundWindow` すると DWM / HWND owner 待ちで
    /// 固まるリスクがあるため、command 経由で HWND 所有スレッドに再アサートさせる。
    RaisePresenterToFront,
    /// Hidden presenter lifecycle (動画→音声モード / tray residency): presenter
    /// ウィンドウと HUD overlay を
    /// 表示 / 非表示にする。`visible=false` = presenter ループが「consume-and-hold」モードに
    /// 入り、present() を呼ばず drain + frame selection + present 成功時 bookkeeping だけを
    /// 続けて最新フレームを hold する (音声は無改変 = 無中断)。`visible=true` = hold して
    /// いたフレームを 1 回 present してから show_and_raise で復帰する。処理後に presenter
    /// pump が typed `WindowHostState` 適用後に output-lifetime の
    /// `NativePresenterVisibility` を更新し、render と App が同じ状態を参照できるようにする。
    // App (bin 専属) からのみ構築される。lib build では app が stub のため dead に見える。
    #[allow(dead_code)]
    SetWindowVisible {
        visible: bool,
    },
}

/// The native output context's single owner of the most recent frame.
///
/// A successfully presented GPU frame is released back to reader key 1 by
/// `NativeRenderCore`, so both `Hidden` and `Visible` frames can be presented
/// again without a render-thread re-arm acquire. `Visible::fence` belongs to
/// the core that last copied the frame and is used only after that frame is
/// displaced into the disposal-only retire queue.
#[cfg(any(windows, test))]
enum FramePresentationState<F> {
    Empty,
    Hidden { frame: F },
    Visible { frame: F, fence: u64 },
}

/// Pump-published native presenter visibility.
///
/// `WindowHostState` on the window-pump thread is the authority. Both the App and
/// render loop keep a clone of this projection; neither maintains an independent
/// hidden/visible flag. In particular, replacing `PresenterSourceState` must not
/// replace or reset this output-lifetime state.
#[cfg(any(windows, test))]
#[derive(Clone)]
struct NativePresenterVisibility {
    hidden: Arc<AtomicBool>,
}

#[cfg(any(windows, test))]
impl NativePresenterVisibility {
    fn new(initial_visibility: NativeVideoInitialVisibility) -> Self {
        Self {
            hidden: Arc::new(AtomicBool::new(!initial_visibility.is_visible())),
        }
    }

    fn is_hidden(&self) -> bool {
        self.hidden.load(Ordering::Acquire)
    }

    /// Publish the visibility after the pump has applied the matching typed
    /// `WindowHostState` transition.
    fn publish_hidden(&self, hidden: bool) {
        self.hidden.store(hidden, Ordering::Release);
    }
}

#[cfg(any(windows, test))]
enum DisplacedPresentation<F> {
    Hidden { frame: F },
    Visible { frame: F, fence: u64 },
}

#[cfg(any(windows, test))]
impl<F> FramePresentationState<F> {
    fn replace_hidden(&mut self, frame: F) -> Option<DisplacedPresentation<F>> {
        let previous = std::mem::replace(self, Self::Hidden { frame });
        Self::into_displaced(previous)
    }

    fn replace_visible(&mut self, frame: F, fence: u64) -> Option<DisplacedPresentation<F>> {
        let previous = std::mem::replace(self, Self::Visible { frame, fence });
        Self::into_displaced(previous)
    }

    fn hide(&mut self) {
        let previous = std::mem::replace(self, Self::Empty);
        *self = match previous {
            Self::Visible { frame, .. } | Self::Hidden { frame } => Self::Hidden { frame },
            Self::Empty => Self::Empty,
        };
    }

    fn frame(&self) -> Option<&F> {
        match self {
            Self::Empty => None,
            Self::Hidden { frame } | Self::Visible { frame, .. } => Some(frame),
        }
    }

    fn visible_frame(&self) -> Option<&F> {
        match self {
            Self::Visible { frame, .. } => Some(frame),
            Self::Empty | Self::Hidden { .. } => None,
        }
    }

    fn mark_current_visible(&mut self, fence: u64) -> bool {
        let previous = std::mem::replace(self, Self::Empty);
        match previous {
            Self::Empty => false,
            Self::Hidden { frame } | Self::Visible { frame, .. } => {
                *self = Self::Visible { frame, fence };
                true
            }
        }
    }

    fn take_displaced(&mut self) -> Option<DisplacedPresentation<F>> {
        let previous = std::mem::replace(self, Self::Empty);
        Self::into_displaced(previous)
    }

    fn should_represent_for_grade_change(&self, render_grade_changed: bool) -> bool {
        render_grade_changed && matches!(self, Self::Visible { .. })
    }

    fn is_hidden(&self) -> bool {
        matches!(self, Self::Hidden { .. })
    }

    #[cfg(test)]
    fn is_visible(&self) -> bool {
        matches!(self, Self::Visible { .. })
    }

    fn into_displaced(state: Self) -> Option<DisplacedPresentation<F>> {
        match state {
            Self::Empty => None,
            Self::Hidden { frame } => Some(DisplacedPresentation::Hidden { frame }),
            Self::Visible { frame, fence } => Some(DisplacedPresentation::Visible { frame, fence }),
        }
    }
}

#[cfg(windows)]
const NATIVE_COMMAND_LATEST_SLOTS: usize = 27;

#[cfg(windows)]
fn native_command_latest_slot(command: &NativeVideoOutputCommand) -> Option<usize> {
    match command {
        NativeVideoOutputCommand::SetHoverThumbnail { .. } => Some(0),
        NativeVideoOutputCommand::SetHoverPreviewPinned { .. } => Some(1),
        NativeVideoOutputCommand::SetTimelineMarkers { .. } => Some(2),
        NativeVideoOutputCommand::SetJumpEntries { .. } => Some(3),
        NativeVideoOutputCommand::SetVideoGrade { .. } => Some(4),
        NativeVideoOutputCommand::SetMetadata { .. } => Some(5),
        NativeVideoOutputCommand::SetSidePanelState { .. } => Some(6),
        NativeVideoOutputCommand::SetLoopEnabled { .. } => Some(7),
        NativeVideoOutputCommand::SetLoopMode { .. } => Some(8),
        NativeVideoOutputCommand::SetContinuousMode { .. } => Some(9),
        NativeVideoOutputCommand::SetVst3Available { .. } => Some(10),
        NativeVideoOutputCommand::SetHudDimmed { .. } => Some(11),
        NativeVideoOutputCommand::SetTextContrast { .. } => Some(12),
        NativeVideoOutputCommand::SetChecked { .. } => Some(13),
        NativeVideoOutputCommand::SetVideoCompact { .. } => Some(14),
        NativeVideoOutputCommand::SetVideoGeometry { .. } => Some(15),
        NativeVideoOutputCommand::SetVst3Panel { .. } => Some(16),
        NativeVideoOutputCommand::SetPlaybackStatus { .. } => Some(17),
        NativeVideoOutputCommand::SetTileOverlay { .. } => Some(18),
        NativeVideoOutputCommand::SetRingPickerOverlay { .. } => Some(19),
        NativeVideoOutputCommand::SetRingGuideOverlay { .. } => Some(20),
        NativeVideoOutputCommand::SetNavigationPreview { .. } => Some(21),
        NativeVideoOutputCommand::MarkCursorActivity => Some(22),
        NativeVideoOutputCommand::RequestOverlayRender => Some(23),
        NativeVideoOutputCommand::SetNormalizeOverlayState { .. } => Some(24),
        NativeVideoOutputCommand::RaiseHudToTop => Some(25),
        NativeVideoOutputCommand::RaisePresenterToFront => Some(26),
        NativeVideoOutputCommand::ResetSidePanelSession
        | NativeVideoOutputCommand::ShowToast { .. }
        | NativeVideoOutputCommand::SwitchSource { .. }
        | NativeVideoOutputCommand::SwitchPlacement { .. }
        | NativeVideoOutputCommand::SetWindowVisible { .. } => None,
    }
}

#[cfg(windows)]
struct SequencedNativeCommand {
    sequence: u64,
    command: NativeVideoOutputCommand,
}

#[cfg(windows)]
struct NativeCommandBusShared {
    next_sequence: AtomicU64,
    latest: Mutex<Vec<Option<SequencedNativeCommand>>>,
    overflow_fault: Arc<AtomicBool>,
}

#[cfg(windows)]
#[derive(Clone)]
struct NativeCommandSender {
    lossless_tx: crossbeam_channel::Sender<SequencedNativeCommand>,
    shared: Arc<NativeCommandBusShared>,
}

#[cfg(windows)]
struct NativeCommandReceiver {
    lossless_rx: crossbeam_channel::Receiver<SequencedNativeCommand>,
    shared: Arc<NativeCommandBusShared>,
}

#[cfg(windows)]
fn native_command_bus(
    capacity: usize,
    overflow_fault: Arc<AtomicBool>,
) -> (NativeCommandSender, NativeCommandReceiver) {
    let (lossless_tx, lossless_rx) = crossbeam_channel::bounded(capacity);
    let shared = Arc::new(NativeCommandBusShared {
        next_sequence: AtomicU64::new(1),
        latest: Mutex::new((0..NATIVE_COMMAND_LATEST_SLOTS).map(|_| None).collect()),
        overflow_fault,
    });
    (
        NativeCommandSender {
            lossless_tx,
            shared: Arc::clone(&shared),
        },
        NativeCommandReceiver {
            lossless_rx,
            shared,
        },
    )
}

#[cfg(windows)]
impl NativeCommandSender {
    fn send(&self, command: NativeVideoOutputCommand) -> Result<(), ()> {
        let sequence = self.shared.next_sequence.fetch_add(1, Ordering::Relaxed);
        let queued = SequencedNativeCommand { sequence, command };
        if let Some(slot) = native_command_latest_slot(&queued.command) {
            match self.shared.latest.lock() {
                Ok(mut latest) => latest[slot] = Some(queued),
                Err(_) => self.shared.overflow_fault.store(true, Ordering::Release),
            }
            return Ok(());
        }
        match self.lossless_tx.try_send(queued) {
            Ok(()) => Ok(()),
            Err(crossbeam_channel::TrySendError::Full(_))
            | Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                self.shared.overflow_fault.store(true, Ordering::Release);
                Err(())
            }
        }
    }
}

#[cfg(windows)]
impl NativeCommandReceiver {
    fn drain(&self) -> Vec<NativeVideoOutputCommand> {
        let mut commands: Vec<_> = self.lossless_rx.try_iter().collect();
        match self.shared.latest.lock() {
            Ok(mut latest) => commands.extend(latest.iter_mut().filter_map(Option::take)),
            Err(_) => self.shared.overflow_fault.store(true, Ordering::Release),
        }
        commands.sort_unstable_by_key(|command| command.sequence);
        commands.into_iter().map(|queued| queued.command).collect()
    }
}

#[cfg(windows)]
struct PresenterSourceState {
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
    frame_step_active: Arc<AtomicBool>,
    duration_secs_bits: Arc<AtomicU64>,
    /// A/V sync drift デバッグ用。fast-swap で旧 source から新 source に payload を
    /// 切り替えるときも同じ Arc を引き継ぎ、grafana 的な時系列が分断されないようにする。
    audio_diagnostics: Arc<crate::video::audio_diagnostics::AudioDiagnostics>,
    source_epoch: u64,
    queue: std::collections::VecDeque<VideoFrame>,
    last_seen_serial: u64,
    first_frame_event_last_epoch: Option<u64>,
    pending_first_frame_event: Option<(u64, f64)>,
    present_stats: NativeFullscreenPresentStats,
    last_present_wall: Option<std::time::Instant>,
    last_present_source_pts: Option<f64>,
    /// drift サンプリングの 1Hz 刻み。
    last_drift_log_at: std::time::Instant,
    /// 大ジャンプ閾値跨ぎ edge の連打を抑える 100ms rate limit。
    last_big_drift_emit_at: std::time::Instant,
    /// 直前 sample が big_drift だったか (edge 検出用)。
    last_av_drift_was_big: bool,
}

#[cfg(windows)]
impl PresenterSourceState {
    fn new(payload: SwitchSourcePayload) -> Self {
        let last_seen_serial = payload.clock.current_seek_serial();
        // 新ソースに切り替わったので present_path は Pending に戻す (= 旧ソースの
        // 直近フレーム経路が UI に残らないように)。deinterlace_status / interlace_detected
        // は decoder thread (新ソース) が独自に書き込むため、ここでは触らない。
        payload.dynamic.present_path.store(
            crate::video::decoder::PRESENT_PATH_PENDING,
            std::sync::atomic::Ordering::Release,
        );
        let now = std::time::Instant::now();
        Self {
            video_rx: payload.video_rx,
            clock: payload.clock,
            engine_event_tx: payload.engine_event_tx,
            displayed_frame_seq: payload.displayed_frame_seq,
            last_displayed_pts_bits: payload.last_displayed_pts_bits,
            frame_step_active: payload.frame_step_active,
            duration_secs_bits: payload.duration_secs_bits,
            audio_diagnostics: payload.audio_diagnostics,
            source_epoch: payload.source_epoch,
            queue: std::collections::VecDeque::new(),
            last_seen_serial,
            first_frame_event_last_epoch: None,
            pending_first_frame_event: None,
            present_stats: NativeFullscreenPresentStats::new(payload.dynamic),
            last_present_wall: None,
            last_present_source_pts: None,
            last_drift_log_at: now,
            last_big_drift_emit_at: now,
            last_av_drift_was_big: false,
        }
    }
}

#[cfg(windows)]
pub(crate) struct NativeVideoOutput {
    cancel: Arc<AtomicBool>,
    hwnd: Arc<AtomicU64>,
    /// HUD overlay HWND (= bars / interactive UI 用の独立 top-level)。
    /// pump thread が `HudOverlayWindow::create` 成功後に store、
    /// fullscreen 終了 / 失敗時は 0 に戻す。App が `dsp_bridge.set_hud_hwnd(...)`
    /// で bridge にも教える経路で参照する (= raise allowlist の「mIV 既知 HWND」判定)。
    hud_hwnd: Arc<AtomicU64>,
    first_presented: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    /// Inc 7 hidden presenter: pump の typed `WindowHostState` が所有する実際の可視状態を
    /// output lifetime で公開する projection。render の consume policy と App
    /// (`exit_video_audio_mode` の async 待ち) は同じ値を読み、source switch では交換しない。
    /// 初期値 visible。
    /// App (bin 専属) からのみ読まれる。lib build では app が stub のため dead に見える。
    #[allow(dead_code)]
    presenter_visibility: NativePresenterVisibility,
    perf_overlay_visible: Arc<AtomicBool>,
    /// app/native_video.rs (bin 専属) からのみ参照されるため lib build では dead に見える。
    #[allow(dead_code)]
    source_epoch: Arc<AtomicU64>,
    /// App 側が「信頼する」現在の presenter placement 世代。`PlacementSwitched` を
    /// 受けるたびに単調非減少で進め、これより古い世代の close を stale として棄却する。
    /// pump/render threads とは共有しない (App = UI thread 専用) が、fast-swap の
    /// `take/attach_native_output` で presenter と一緒に運ばれるため lifetime 正しい。
    #[allow(dead_code)]
    committed_generation: AtomicU64,
    last_vst3_available: AtomicBool,
    last_checked: AtomicBool,
    last_text_contrast_strong: AtomicBool,
    visibility_gate: Arc<NativeVideoOutputVisibilityGate>,
    command_tx: NativeCommandSender,
    event_rx: std::sync::Mutex<NativeOutputEventReceiver>,
    /// Latest presenter-published input ownership snapshot. The UI thread
    /// refreshes it while draining the existing output event bus; gamepad
    /// polling reads it without reaching into the presenter thread.
    overlay_input_routing: std::sync::Mutex<native_presenter::NativeOverlayInputRouting>,
    /// Presenter thread 内で起きた fatal init error (`CoInitializeEx` /
    /// `NativeWindowHost::create` / `NativeRenderCore::new` 失敗) を
    /// VideoPlayer に伝えるための one-shot ストレージ。
    /// `take_init_error` で 1 度だけ取り出し、`VideoPlayer.error` に転写する。
    init_error: Arc<Mutex<Option<String>>>,
    threads: Option<NativeVideoOutputThreads>,
}

#[cfg(windows)]
struct NativeVideoOutputVisibilityGate {
    base_visible: AtomicBool,
    command_tx: NativeCommandSender,
    hwnd: Arc<AtomicU64>,
}

#[cfg(windows)]
impl NativeVideoOutputVisibilityGate {
    fn new(initial_visible: bool, command_tx: NativeCommandSender, hwnd: Arc<AtomicU64>) -> Self {
        Self {
            base_visible: AtomicBool::new(initial_visible),
            command_tx,
            hwnd,
        }
    }

    fn set_base_visible(&self, visible: bool) {
        self.base_visible.store(visible, Ordering::Release);
        self.publish();
    }

    fn publish(&self) {
        let visible = self.effective_visible();
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetWindowVisible { visible });
        crate::video::native_window::post_wake(self.hwnd.load(Ordering::Acquire));
    }

    fn effective_visible(&self) -> bool {
        self.base_visible.load(Ordering::Acquire)
    }
}

#[cfg(windows)]
struct NativeVideoOutputThreads {
    render: std::thread::JoinHandle<()>,
    pump: std::thread::JoinHandle<()>,
}

#[cfg(windows)]
impl NativeVideoOutput {
    #[cfg(test)]
    pub(crate) fn disconnected_for_test() -> Self {
        Self::disconnected_for_test_with_event_sender().0
    }

    #[cfg(test)]
    fn disconnected_for_test_with_event_sender() -> (Self, NativeOutputEventSender) {
        let channel_fault = Arc::new(AtomicBool::new(false));
        let (command_tx, _command_rx) = native_command_bus(8, Arc::clone(&channel_fault));
        let (event_tx, event_rx) = native_output_event_bus(8, channel_fault);
        let hwnd = Arc::new(AtomicU64::new(0));
        let visibility_gate = Arc::new(NativeVideoOutputVisibilityGate::new(
            true,
            command_tx.clone(),
            Arc::clone(&hwnd),
        ));
        let output = Self {
            cancel: Arc::new(AtomicBool::new(false)),
            hwnd,
            hud_hwnd: Arc::new(AtomicU64::new(0)),
            first_presented: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
            presenter_visibility: NativePresenterVisibility::new(
                NativeVideoInitialVisibility::Visible,
            ),
            perf_overlay_visible: Arc::new(AtomicBool::new(false)),
            source_epoch: Arc::new(AtomicU64::new(0)),
            committed_generation: AtomicU64::new(0),
            last_vst3_available: AtomicBool::new(false),
            last_checked: AtomicBool::new(false),
            last_text_contrast_strong: AtomicBool::new(false),
            visibility_gate,
            command_tx,
            event_rx: std::sync::Mutex::new(event_rx),
            overlay_input_routing: std::sync::Mutex::new(
                native_presenter::NativeOverlayInputRouting::default(),
            ),
            init_error: Arc::new(Mutex::new(None)),
            threads: None,
        };
        (output, event_tx)
    }

    #[cfg(test)]
    pub(crate) fn mark_closed_for_test(&self) {
        self.closed.store(true, Ordering::Release);
    }

    fn spawn(
        video_rx: crossbeam_channel::Receiver<VideoFrame>,
        clock: Arc<AvClock>,
        engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
        displayed_frame_seq: Arc<AtomicU64>,
        last_displayed_pts_bits: Arc<AtomicU64>,
        frame_step_active: Arc<AtomicBool>,
        duration_secs_bits: Arc<AtomicU64>,
        config: NativeVideoOutputConfig,
        dynamic: Arc<crate::video::decoder::VideoDynamicState>,
        audio_diagnostics: Arc<crate::video::audio_diagnostics::AudioDiagnostics>,
    ) -> Option<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let hwnd = Arc::new(AtomicU64::new(0));
        let hud_hwnd = Arc::new(AtomicU64::new(0));
        let first_presented = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let presenter_visibility = NativePresenterVisibility::new(config.initial_visibility);
        let perf_overlay_visible = Arc::new(AtomicBool::new(config.perf_overlay_visible));
        let source_epoch = Arc::new(AtomicU64::new(0));
        let initial_vst3_available = config.vst3_available;
        let initial_checked = config.checked;
        let initial_text_contrast_strong =
            matches!(config.text_contrast, crate::settings::TextContrast::Strong);
        let channel_fault = Arc::new(AtomicBool::new(false));
        let health = native_window_health::NativeWindowHealth::new_registered();
        let (event_tx, event_rx) = native_output_event_bus(512, Arc::clone(&channel_fault));
        let (command_tx, command_rx) = native_command_bus(512, Arc::clone(&channel_fault));
        let visibility_gate = Arc::new(NativeVideoOutputVisibilityGate::new(
            config.initial_visibility.is_visible(),
            command_tx.clone(),
            Arc::clone(&hwnd),
        ));
        let init_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let thread_cancel = Arc::clone(&cancel);
        let thread_first_presented = Arc::clone(&first_presented);
        let thread_perf_overlay_visible = Arc::clone(&perf_overlay_visible);
        let thread_init_error = Arc::clone(&init_error);
        let thread_presenter_visibility = presenter_visibility.clone();
        let thread_health = Arc::clone(&health);
        let pump = match native_window_pump::spawn_native_window_pump(
            native_window_pump::NativeWindowPumpSpawn {
                config: config.clone(),
                cancel: Arc::clone(&cancel),
                hwnd_out: Arc::clone(&hwnd),
                hud_hwnd_out: Arc::clone(&hud_hwnd),
                closed: Arc::clone(&closed),
                presenter_visibility: presenter_visibility.clone(),
                source_epoch: Arc::clone(&source_epoch),
                ui_event_tx: event_tx.clone(),
                init_error: Arc::clone(&init_error),
                channel_fault: Arc::clone(&channel_fault),
                health: Arc::clone(&health),
            },
        ) {
            Ok(pump) => pump,
            Err(err) => {
                crate::logger::log(format!("[native-video] failed to spawn window pump: {err}"));
                return None;
            }
        };
        let native_window_pump::NativeWindowPumpThread {
            render: pump_render,
            join: pump_join,
        } = pump;
        let render = match std::thread::Builder::new()
            .name("native-video-render".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_native_video_output(
                        video_rx,
                        clock,
                        engine_event_tx,
                        displayed_frame_seq,
                        last_displayed_pts_bits,
                        frame_step_active,
                        duration_secs_bits,
                        config,
                        command_rx,
                        event_tx,
                        thread_cancel,
                        thread_first_presented,
                        thread_perf_overlay_visible,
                        thread_presenter_visibility,
                        dynamic,
                        audio_diagnostics,
                        &pump_render,
                        thread_health,
                    )
                }));
                let error = match result {
                    Ok(Ok(())) => return,
                    Ok(Err(err)) => err,
                    Err(_) => "native video render thread panicked".to_string(),
                };
                {
                    let err = error;
                    crate::logger::log(format!("[native-video] presenter stopped: {err}"));
                    if let Ok(mut slot) = thread_init_error.lock() {
                        if slot.is_none() {
                            *slot = Some(format!("native video render fault: {err}"));
                        }
                    }
                    pump_render.render_fault(u64::MAX - 1, err);
                }
            }) {
            Ok(render) => render,
            Err(err) => {
                cancel.store(true, Ordering::Release);
                crate::logger::log(format!(
                    "[native-video] failed to spawn render thread: {err}"
                ));
                return None;
            }
        };
        Some(Self {
            cancel,
            hwnd,
            hud_hwnd,
            first_presented,
            closed,
            presenter_visibility,
            perf_overlay_visible,
            source_epoch,
            committed_generation: AtomicU64::new(0),
            last_vst3_available: AtomicBool::new(initial_vst3_available),
            last_checked: AtomicBool::new(initial_checked),
            last_text_contrast_strong: AtomicBool::new(initial_text_contrast_strong),
            visibility_gate,
            command_tx,
            event_rx: std::sync::Mutex::new(event_rx),
            overlay_input_routing: std::sync::Mutex::new(
                native_presenter::NativeOverlayInputRouting::default(),
            ),
            init_error,
            threads: Some(NativeVideoOutputThreads {
                render,
                pump: pump_join,
            }),
        })
    }

    pub(crate) fn hwnd(&self) -> u64 {
        self.hwnd.load(Ordering::Acquire)
    }

    /// HUD overlay HWND (= bars / interactive UI 用の独立 top-level)。
    /// pump thread が `HudOverlayWindow::create` 成功時に store する。
    /// HUD HWND が生成されていない (フォールバック経路) なら 0。
    pub(crate) fn hud_hwnd(&self) -> u64 {
        self.hud_hwnd.load(Ordering::Acquire)
    }

    fn first_presented(&self) -> bool {
        self.first_presented.load(Ordering::Acquire)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Hidden presenter lifecycle: presenter ウィンドウ (+ HUD overlay) の表示 / 非表示を
    /// 要求する。App は `is_presenter_hidden` で pump の typed visibility ack 反映完了
    /// (= show 済み) をポーリングする。
    /// App (bin 専属) からのみ呼ばれる。lib build では app が stub のため dead に見える。
    #[allow(dead_code)]
    pub(crate) fn set_window_visible(&self, visible: bool) {
        self.visibility_gate.set_base_visible(visible);
    }

    /// presenter ウィンドウが現在 hide (consume-and-hold) 中か。exit の async 待ちで
    /// 「show コマンドが反映されて再表示済みか」を検知するのに使う。
    /// App (bin 専属) からのみ呼ばれる。lib build では app が stub のため dead に見える。
    #[allow(dead_code)]
    pub(crate) fn is_presenter_hidden(&self) -> bool {
        self.presenter_visibility.is_hidden()
    }

    /// pump/render thread 内で起きた fatal init error を 1 度だけ取り出す。
    /// VideoPlayer::tick が毎フレーム pull し、Some なら `self.error` に転写して
    /// UI に赤字エラーを表示させる。
    fn take_init_error(&self) -> Option<String> {
        self.init_error.lock().ok().and_then(|mut g| g.take())
    }

    #[allow(dead_code)]
    fn source_epoch(&self) -> u64 {
        self.source_epoch.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    pub(crate) fn committed_generation(&self) -> u64 {
        self.committed_generation.load(Ordering::Acquire)
    }

    /// `PlacementSwitched` を受けたときに App が呼ぶ。世代は単調非減少 (max) で進める。
    /// stale (request mismatch / out-of-order) な PlacementSwitched でも、presenter の
    /// 現世代を追い越すことは無いので max で吸収する。
    #[allow(dead_code)]
    pub(crate) fn bump_committed_generation(&self, generation: u64) {
        let cur = self.committed_generation.load(Ordering::Acquire);
        if generation > cur {
            self.committed_generation
                .store(generation, Ordering::Release);
        }
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

    fn set_video_grade(&self, grade: crate::creative_lut::VideoGradeSnapshot) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetVideoGrade { grade });
        crate::video::native_window::post_wake(self.hwnd.load(Ordering::Acquire));
    }

    fn set_metadata(&self, metadata: Option<native_presenter::NativeOverlayMetadata>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetMetadata { metadata });
    }

    fn set_side_panel_state(
        &self,
        mode: crate::settings::FsSidePanelMode,
        info_panel_open: crate::ui_helpers::MetadataPanelOpenState,
    ) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetSidePanelState {
                mode,
                info_panel_open,
            });
    }

    fn reset_side_panel_session(&self) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::ResetSidePanelSession);
    }

    fn set_loop_enabled(&self, enabled: bool) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetLoopEnabled { enabled });
    }

    fn set_loop_mode(&self, mode: crate::settings::VideoLoopMode) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetLoopMode { mode });
    }

    fn set_continuous_mode(&self, mode: VideoContinuousMode) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetContinuousMode { mode });
    }

    fn set_vst3_available(&self, available: bool) {
        if self.last_vst3_available.swap(available, Ordering::AcqRel) == available {
            return;
        }
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetVst3Available { available });
    }

    fn set_hud_dimmed(&self, dimmed: bool) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetHudDimmed { dimmed });
    }

    fn set_text_contrast(&self, contrast: crate::settings::TextContrast) {
        let strong = matches!(contrast, crate::settings::TextContrast::Strong);
        if self
            .last_text_contrast_strong
            .swap(strong, Ordering::AcqRel)
            == strong
        {
            return;
        }
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetTextContrast { contrast });
        crate::video::native_window::post_wake(self.hwnd.load(Ordering::Acquire));
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

    fn set_video_geometry(
        &self,
        num: u32,
        den: u32,
        orientation: display_metadata::VideoOrientation,
    ) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetVideoGeometry {
                num,
                den,
                orientation,
            });
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

    fn set_ring_picker_overlay(&self, overlay: Option<native_presenter::NativeOverlayRingPicker>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetRingPickerOverlay { overlay });
    }

    fn set_ring_guide_overlay(&self, overlay: Option<native_presenter::NativeOverlayRingGuide>) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetRingGuideOverlay { overlay });
    }

    #[allow(dead_code)]
    pub(crate) fn set_navigation_preview(
        &self,
        preview: Option<native_presenter::NativeOverlayNavigationPreview>,
    ) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetNavigationPreview { preview });
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

    #[allow(dead_code)]
    fn switch_placement(
        &self,
        request_id: u64,
        placement: NativeVideoPlacement,
        owner_hwnd: u64,
        rect: windows::Win32::Foundation::RECT,
        activate_on_show: bool,
    ) {
        let visible = self.visibility_gate.effective_visible();
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SwitchPlacement {
                request_id,
                placement,
                owner_hwnd,
                rect,
                activate_on_show,
                visible,
            });
    }

    fn set_playback_status(
        &self,
        first_frame_presented: bool,
        error: Option<String>,
        prep_status: avio_progress::PreparingStatus,
    ) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetPlaybackStatus {
                first_frame_presented,
                error,
                prep_status,
            });
    }

    fn show_toast(&self, text: String, centered: bool, linger: Option<std::time::Duration>) {
        let _ = self.command_tx.send(NativeVideoOutputCommand::ShowToast {
            text,
            centered,
            linger,
        });
    }

    fn mark_cursor_activity(&self) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::MarkCursorActivity);
    }

    fn request_overlay_render(&self) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::RequestOverlayRender);
    }

    /// CP7: HUD overlay HWND を最前面に上げ直す要求を render orchestration に送る。
    /// `DspBridge::hud_raise_hook` 経由で発火された全 z-order 変更操作に対する応答。
    /// pump 側で retry burst (即時/16ms/50ms) として処理される。
    fn request_hud_raise(&self) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::RaiseHudToTop);
    }

    pub(crate) fn request_presenter_raise(&self) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::RaisePresenterToFront);
    }

    fn set_normalize_overlay_state(
        &self,
        state: crate::video::normalize_types::NormalizeOverlayState,
    ) {
        let _ = self
            .command_tx
            .send(NativeVideoOutputCommand::SetNormalizeOverlayState { state });
    }

    pub(crate) fn drain_events(&self) -> Vec<(u64, NativeVideoOutputEvent)> {
        let Ok(rx) = self.event_rx.lock() else {
            return Vec::new();
        };
        let mut events = rx.drain();
        let mut latest_routing = None;
        events.retain(|(_, event)| match event {
            NativeVideoOutputEvent::OverlayInputRouting(routing) => {
                latest_routing = Some(*routing);
                false
            }
            _ => true,
        });
        if let Some(routing) = latest_routing
            && let Ok(mut published) = self.overlay_input_routing.lock()
        {
            *published = routing;
        }
        events
    }

    pub(crate) fn overlay_input_routing_snapshot(
        &self,
    ) -> native_presenter::NativeOverlayInputRouting {
        if self.is_closed() || self.is_presenter_hidden() {
            return native_presenter::NativeOverlayInputRouting::default();
        }
        self.overlay_input_routing
            .lock()
            .map(|routing| *routing)
            .unwrap_or_default()
    }
}

#[cfg(windows)]
impl Drop for NativeVideoOutput {
    fn drop(&mut self) {
        crate::gpu_info::emit_vram_trace_with_options(
            "native_output_drop",
            "before_cancel",
            &[],
            false,
        );
        self.cancel.store(true, Ordering::Release);
        if let Some(threads) = self.threads.take() {
            match std::thread::Builder::new()
                .name("native-video-output-drop-join".to_string())
                .spawn(move || {
                    let pump_join_ok = threads.pump.join().is_ok();
                    let render_join_ok = threads.render.join().is_ok();
                    crate::gpu_info::emit_vram_trace(
                        "native_output_drop_join",
                        "after_native_video_threads_join",
                        &[
                            ("pump_join_ok", serde_json::Value::from(pump_join_ok)),
                            ("render_join_ok", serde_json::Value::from(render_join_ok)),
                        ],
                    );
                }) {
                Ok(_) => {}
                Err(e) => {
                    crate::logger::log(format!(
                        "native video output drop join spawn failed: {e:?}"
                    ));
                }
            }
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
struct NativeFullscreenPresentStats {
    presented: u64,
    gpu: u64,
    cpu: u64,
    late_drop: u64,
    wait_timeout: u64,
    max_late_ms: f64,
    max_total_ms: f64,
    max_interval_ms: f64,
    /// Per-frame プレゼン経路を UI に動的公開するための共有 atomic。
    /// `record_present` で `present_path` を都度更新する。
    dynamic: Arc<crate::video::decoder::VideoDynamicState>,
}

#[cfg(windows)]
impl NativeFullscreenPresentStats {
    fn new(dynamic: Arc<crate::video::decoder::VideoDynamicState>) -> Self {
        Self {
            presented: 0,
            gpu: 0,
            cpu: 0,
            late_drop: 0,
            wait_timeout: 0,
            max_late_ms: 0.0,
            max_total_ms: 0.0,
            max_interval_ms: 0.0,
            dynamic,
        }
    }

    fn record_present(
        &mut self,
        outcome: &crate::video::native_presenter::NativePresentOutcome,
        late_ms: f64,
        total_ms: f64,
        interval_ms: f64,
    ) {
        self.presented += 1;
        match outcome.path {
            "d3d11_shared" => {
                self.gpu += 1;
                self.dynamic
                    .present_path
                    .store(crate::video::decoder::PRESENT_PATH_GPU, Ordering::Release);
            }
            "cpu_upload" => {
                self.cpu += 1;
                self.dynamic
                    .present_path
                    .store(crate::video::decoder::PRESENT_PATH_CPU, Ordering::Release);
            }
            _ => {}
        }
        if outcome.wait_timed_out {
            self.wait_timeout += 1;
        }
        self.max_late_ms = self.max_late_ms.max(late_ms);
        self.max_total_ms = self.max_total_ms.max(total_ms);
        self.max_interval_ms = self.max_interval_ms.max(interval_ms);
    }

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

    /// Per-frame snapshot for the P-key overlay header text. `diag_view` carries the
    /// drift / underrun state from `AudioDiagnostics` (callers atomic-load the values
    /// before invoking; this struct stays decoupled from `Source` / `VideoPlayer`).
    fn overlay_snapshot(
        &self,
        duration: std::time::Duration,
        diag_view: crate::video::audio_diagnostics::OverlayDiagnostics,
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
            av_drift_ms: diag_view.av_drift_ms,
            av_offset_ms: diag_view.av_offset_ms.unwrap_or(f32::NAN),
            audio_active: diag_view.audio_active,
            audio_lead_ms: diag_view.audio_lead_ms,
            audio_underrun_active: diag_view.audio_underrun_active,
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

struct HeadlessVideoOutput {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HeadlessVideoOutput {
    fn spawn(
        video_rx: crossbeam_channel::Receiver<VideoFrame>,
        player_cancel: Arc<AtomicBool>,
        clock: Arc<AvClock>,
        engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
        displayed_frame_seq: Arc<AtomicU64>,
        last_displayed_pts_bits: Arc<AtomicU64>,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = std::thread::Builder::new()
            .name("remote-headless-video-output".to_owned())
            .spawn(move || {
                run_headless_video_output(
                    video_rx,
                    worker_stop,
                    player_cancel,
                    clock,
                    engine_event_tx,
                    displayed_frame_seq,
                    last_displayed_pts_bits,
                );
            })
            .map_err(|err| err.to_string())?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for HeadlessVideoOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

enum VideoOutputState {
    Presentation,
    RemoteHeadless(HeadlessVideoOutput),
    Inactive,
}

#[derive(Default)]
struct HeadlessReadyState {
    ready_epoch: Option<u64>,
    pending: Option<(u64, f64)>,
}

impl HeadlessReadyState {
    fn follow_epoch(&mut self, live_epoch: u64) {
        if self
            .pending
            .is_some_and(|(pending_epoch, _)| pending_epoch != live_epoch)
        {
            self.pending = None;
        }
    }

    fn retry(&mut self, clock: &AvClock, engine_event_tx: &crossbeam_channel::Sender<EngineEvent>) {
        let Some((epoch, pts)) = self.pending else {
            return;
        };
        if clock.current_seek_serial() != epoch {
            self.pending = None;
        } else {
            match try_send_headless_first_frame_ready(engine_event_tx, epoch, pts) {
                HeadlessReadySend::Sent => {
                    log_headless_first_frame_ready(epoch, pts);
                    self.ready_epoch = Some(epoch);
                    self.pending = None;
                }
                HeadlessReadySend::Full => {}
                HeadlessReadySend::Disconnected => {
                    self.ready_epoch = Some(epoch);
                    self.pending = None;
                }
            }
        }
    }

    fn observe(&mut self, epoch: u64, pts: f64) -> bool {
        if self.ready_epoch != Some(epoch)
            && self.pending.map(|(pending_epoch, _)| pending_epoch) != Some(epoch)
        {
            self.pending = Some((epoch, pts));
            true
        } else {
            false
        }
    }
}

fn run_headless_video_output(
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    stop: Arc<AtomicBool>,
    player_cancel: Arc<AtomicBool>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
) {
    let mut live_epoch = clock.current_seek_serial();
    crate::logger::log(format!(
        "[remote-headless-video] consumer started: live_epoch={}",
        live_epoch
    ));
    let mut ready = HeadlessReadyState::default();
    let mut drained_frames = 0_u64;
    while !stop.load(Ordering::Acquire) && !player_cancel.load(Ordering::Acquire) {
        let current_epoch = clock.current_seek_serial();
        if current_epoch != live_epoch {
            crate::logger::log(format!(
                "[remote-headless-video] consumer epoch changed: previous_epoch={live_epoch} live_epoch={current_epoch}"
            ));
            live_epoch = current_epoch;
            ready.follow_epoch(live_epoch);
        }
        ready.retry(&clock, &engine_event_tx);
        match video_rx.recv_timeout(std::time::Duration::from_millis(5)) {
            Ok(frame) => {
                let epoch = frame.seek_serial;
                let pts = frame.pts_secs;
                let current_epoch = clock.current_seek_serial();
                if current_epoch != live_epoch {
                    crate::logger::log(format!(
                        "[remote-headless-video] consumer epoch changed: previous_epoch={live_epoch} live_epoch={current_epoch}"
                    ));
                    live_epoch = current_epoch;
                    ready.follow_epoch(live_epoch);
                }
                if epoch == live_epoch {
                    if ready.observe(epoch, pts) {
                        crate::logger::log(format!(
                            "[remote-headless-video] consumer active: epoch={epoch} first_pts={pts:.3} remaining_video_rx_len={}",
                            video_rx.len()
                        ));
                    }
                    last_displayed_pts_bits.store(pts.to_bits(), Ordering::Release);
                    displayed_frame_seq.fetch_add(1, Ordering::AcqRel);
                }
                drained_frames = drained_frames.saturating_add(1);
                native_reset_unpresented_frame(frame);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    while let Ok(frame) = video_rx.try_recv() {
        drained_frames = drained_frames.saturating_add(1);
        native_reset_unpresented_frame(frame);
    }
    crate::logger::log(format!(
        "[remote-headless-video] consumer stopped: drained_frames={drained_frames}"
    ));
}

struct VideoOutputRoute {
    player_rx: crossbeam_channel::Receiver<VideoFrame>,
    state: VideoOutputState,
    init_error: Option<String>,
}

fn route_video_output(
    consumer: VideoOutputConsumer,
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    player_cancel: Arc<AtomicBool>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
) -> VideoOutputRoute {
    match consumer {
        VideoOutputConsumer::Presentation => {
            return VideoOutputRoute {
                player_rx: video_rx,
                state: VideoOutputState::Presentation,
                init_error: None,
            };
        }
        VideoOutputConsumer::RemoteHeadless => {}
    }

    let player_rx = dummy_video_rx();
    match HeadlessVideoOutput::spawn(
        video_rx,
        player_cancel,
        clock,
        engine_event_tx,
        displayed_frame_seq,
        last_displayed_pts_bits,
    ) {
        Ok(headless) => VideoOutputRoute {
            player_rx,
            state: VideoOutputState::RemoteHeadless(headless),
            init_error: None,
        },
        Err(error) => VideoOutputRoute {
            player_rx,
            state: VideoOutputState::Inactive,
            init_error: Some(error),
        },
    }
}

enum HeadlessReadySend {
    Sent,
    Full,
    Disconnected,
}

fn log_headless_first_frame_ready(epoch: u64, pts: f64) {
    crate::logger::log(format!(
        "[remote-headless-video] FirstFrameReady emitted: epoch={epoch} pts={pts:.3}"
    ));
}

fn try_send_headless_first_frame_ready(
    engine_event_tx: &crossbeam_channel::Sender<EngineEvent>,
    epoch: u64,
    pts: f64,
) -> HeadlessReadySend {
    let event = EngineEvent::Decoder(engine::state::DecoderEvent::FirstFrameReady { epoch, pts });
    match engine_event_tx.try_send(event) {
        Ok(()) => HeadlessReadySend::Sent,
        Err(crossbeam_channel::TrySendError::Full(_)) => HeadlessReadySend::Full,
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => HeadlessReadySend::Disconnected,
    }
}

/// Hidden native presenters do not pace frames against the display clock. Drain both the
/// already-paced queue and the decoder channel so the decoder can continue through demux EOF.
///
/// `drained` is caller-owned scratch storage; production reuses it for the lifetime of the
/// presenter thread. The caller still validates seek serials and reduces the batch to the one
/// latest `FramePresentationState::Hidden` owner.
#[cfg(any(windows, test))]
fn drain_hidden_available_frames<F>(
    queue: &mut std::collections::VecDeque<F>,
    receiver: &crossbeam_channel::Receiver<F>,
    drained: &mut Vec<F>,
) -> usize {
    drained.clear();
    drained.extend(queue.drain(..));
    while let Ok(frame) = receiver.try_recv() {
        drained.push(frame);
    }
    drained.len()
}

// Fence-stall fallback cap. The shared pool also retains at most one typed
// Visible/Hidden frame, so retire stays bounded independently of the current
// presentation owner.
#[cfg(windows)]
const NATIVE_PRESENT_RETIRE_CAP: usize = 4;

/// Native-output-local ownership for the current frame and frames whose
/// asynchronous presenter copy is awaiting disposal.
///
/// `presentation` is the only recent-frame source. `present_retire` never
/// supplies a frame for presentation; it only delays drop until the copy fence
/// completes (or the bounded fallback cap is reached).
#[cfg(windows)]
struct NativeFrameOutputContext {
    presentation: FramePresentationState<VideoFrame>,
    present_retire: std::collections::VecDeque<(VideoFrame, u64)>,
}

#[cfg(windows)]
impl NativeFrameOutputContext {
    fn new() -> Self {
        Self {
            presentation: FramePresentationState::Empty,
            present_retire: std::collections::VecDeque::new(),
        }
    }

    fn retire_len(&self) -> usize {
        self.present_retire.len()
    }

    fn should_represent_for_grade_change(&self, render_grade_changed: bool) -> bool {
        self.presentation
            .should_represent_for_grade_change(render_grade_changed)
    }

    fn hide(&mut self) {
        self.presentation.hide();
    }

    fn is_hidden(&self) -> bool {
        self.presentation.is_hidden()
    }

    fn frame(&self) -> Option<&VideoFrame> {
        self.presentation.frame()
    }

    fn visible_frame(&self) -> Option<&VideoFrame> {
        self.presentation.visible_frame()
    }

    fn mark_current_visible(&mut self, fence: u64) -> bool {
        self.presentation.mark_current_visible(fence)
    }

    fn hold_hidden(&mut self, frame: VideoFrame) {
        let displaced = self.presentation.replace_hidden(frame);
        self.dispose_displaced(displaced);
    }

    fn commit_presented(&mut self, frame: VideoFrame, fence: u64) {
        let displaced = self.presentation.replace_visible(frame, fence);
        self.dispose_displaced(displaced);
    }

    fn clear_current(&mut self) {
        let displaced = self.presentation.take_displaced();
        self.dispose_displaced(displaced);
    }

    fn retire_completed(&mut self, completed: Option<u64>) {
        if let Some(completed) = completed {
            while self
                .present_retire
                .front()
                .is_some_and(|(_, value)| *value != 0 && *value <= completed)
            {
                self.present_retire.pop_front();
            }
        }
        self.enforce_retire_cap();
    }

    fn invalidate_retire_fence_prefix(&mut self, count: usize) {
        for entry in self.present_retire.iter_mut().take(count) {
            entry.1 = 0;
        }
    }

    fn dispose_displaced(&mut self, displaced: Option<DisplacedPresentation<VideoFrame>>) {
        match displaced {
            Some(DisplacedPresentation::Hidden { frame }) => {
                native_reset_unpresented_frame(frame);
            }
            Some(DisplacedPresentation::Visible { frame, fence }) => {
                self.present_retire.push_back((frame, fence));
                self.enforce_retire_cap();
            }
            None => {}
        }
    }

    fn enforce_retire_cap(&mut self) {
        while self.present_retire.len() > NATIVE_PRESENT_RETIRE_CAP {
            self.present_retire.pop_front();
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VisibleVideoLoopAction {
    ProcessFrames,
    RefreshSettledResizeThenIdle,
}

#[cfg(any(windows, test))]
fn visible_video_loop_action(
    presenter_hidden: bool,
    is_playing: bool,
    is_seeking: bool,
    waiting_for_first_frame: bool,
) -> VisibleVideoLoopAction {
    if !presenter_hidden && !is_playing && !is_seeking && !waiting_for_first_frame {
        VisibleVideoLoopAction::RefreshSettledResizeThenIdle
    } else {
        VisibleVideoLoopAction::ProcessFrames
    }
}

#[cfg(windows)]
fn refresh_settled_resize_if_due(
    presenter: &mut native_presenter::NativeRenderCore,
    frame_output: &mut NativeFrameOutputContext,
    sync_interval: u32,
) {
    if !presenter.take_settled_resize_refresh_due(std::time::Instant::now()) {
        return;
    }

    let refresh = frame_output
        .visible_frame()
        .map(|frame| presenter.present(frame, sync_interval));
    match refresh {
        Some(Ok(outcome)) => {
            debug_assert!(frame_output.mark_current_visible(outcome.copy_fence_value));
            frame_output.retire_completed(presenter.copy_fence_completed_value());
        }
        Some(Err(error)) => crate::logger::log(format!(
            "[native-video] settled resize refresh present failed: {error}"
        )),
        None => {}
    }
}

#[cfg(any(windows, test))]
fn video_grade_render_changed(
    previous: &crate::creative_lut::VideoGradeSnapshot,
    next: &crate::creative_lut::VideoGradeSnapshot,
) -> bool {
    if previous.adjustments != next.adjustments {
        return true;
    }
    if next.adjustments.creative_lut.is_identity() {
        return false;
    }
    match (&previous.lut, &next.lut) {
        (Some(previous), Some(next)) => !std::sync::Arc::ptr_eq(previous, next),
        (None, None) => false,
        _ => true,
    }
}

#[cfg(windows)]
fn emit_native_vram_trace(
    kind: &str,
    reason: &str,
    source: &PresenterSourceState,
    presenter: &native_presenter::NativeRenderCore,
    present_retire_len: usize,
) {
    if !crate::perf::is_enabled() {
        return;
    }
    let (surface_width, surface_height) = presenter.surface_size();
    let (grade_intermediate_bytes, resample_intermediate_bytes) =
        presenter.intermediate_vram_bytes();
    crate::gpu_info::emit_vram_trace(
        kind,
        reason,
        &[
            (
                "source_epoch",
                serde_json::Value::from(source.source_epoch as i64),
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
                "present_retire_len",
                serde_json::Value::from(present_retire_len as i64),
            ),
            (
                "shared_texture_cache_len",
                serde_json::Value::from(presenter.shared_texture_cache_len() as i64),
            ),
            (
                "retired_video_surfaces_len",
                serde_json::Value::from(presenter.retired_video_surface_len() as i64),
            ),
            (
                "surface_width",
                serde_json::Value::from(surface_width as i64),
            ),
            (
                "surface_height",
                serde_json::Value::from(surface_height as i64),
            ),
            (
                "grade_intermediate_bytes",
                serde_json::Value::from(grade_intermediate_bytes),
            ),
            (
                "resample_intermediate_bytes",
                serde_json::Value::from(resample_intermediate_bytes),
            ),
        ],
    );
}

/// 環境変数を「真偽値」として読む。`""`/`"0"`/`"false"`/`"off"`/`"no"`
/// (大小無視) を false、未設定時は `default` を返す。
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

/// CP7: 環境変数で「明示的に off にされているか」を判定。
/// 値が `0` / `false` / `off` / `no` のとき true、それ以外は false (= 未設定 / その他)。
/// 「default off で 1/true/on/yes で有効化」する `native_video_env_flag_enabled` と異なり、
/// 「default on で 0/false/off/no で無効化」する用途に使う。
fn native_video_env_flag_disabled(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            let v = v.trim();
            v == "0"
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("no")
        })
        .unwrap_or(false)
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
    tx: &NativeOutputEventSender,
    source_epoch: u64,
    event: NativeVideoOutputEvent,
) {
    tx.send(source_epoch, event);
}

#[cfg(windows)]
fn publish_native_overlay_input_routing(
    tx: &NativeOutputEventSender,
    source_epoch: u64,
    published: &mut native_presenter::NativeOverlayInputRouting,
    routing: native_presenter::NativeOverlayInputRouting,
) {
    if *published == routing {
        return;
    }
    *published = routing;
    send_native_output_event(
        tx,
        source_epoch,
        NativeVideoOutputEvent::OverlayInputRouting(routing),
    );
}

#[cfg(windows)]
fn send_native_overlay_command(
    tx: &NativeOutputEventSender,
    source_epoch: u64,
    generation: u64,
    command: crate::video::native_presenter::NativeOverlayCommand,
) {
    use crate::video::native_presenter::NativeOverlayCommand as Command;
    let event = match command {
        Command::Seek { target_secs } => NativeVideoOutputEvent::Seek { target_secs },
        Command::SeekRelative { delta_secs } => NativeVideoOutputEvent::SeekRelative { delta_secs },
        Command::TouchChromeLearned => NativeVideoOutputEvent::TouchChromeLearned,
        Command::TileSeek { target_secs } => NativeVideoOutputEvent::TileSeek { target_secs },
        Command::NavigateItem { delta, via_wheel } => {
            NativeVideoOutputEvent::NavigateItem { delta, via_wheel }
        }
        Command::TileColumnsDelta { delta } => NativeVideoOutputEvent::TileColumnsDelta { delta },
        Command::RequestSeekThumbnail {
            target_secs,
            bar_width_points,
            pixels_per_point,
        } => NativeVideoOutputEvent::RequestSeekThumbnail {
            target_secs,
            bar_width_points,
            pixels_per_point,
        },
        Command::ClearSeekThumbnail => NativeVideoOutputEvent::ClearSeekThumbnail,
        Command::ToggleTileMode => NativeVideoOutputEvent::ToggleTileMode,
        Command::TogglePerfOverlay => NativeVideoOutputEvent::TogglePerfOverlay,
        Command::ToggleSidePanelMode => NativeVideoOutputEvent::ToggleSidePanelMode,
        Command::ToggleClickInfoOpen => NativeVideoOutputEvent::ToggleClickInfoOpen,
        Command::OpenTouchInfoPanel => NativeVideoOutputEvent::OpenTouchInfoPanel,
        Command::DismissTouchSidePanels => NativeVideoOutputEvent::DismissTouchSidePanels,
        Command::ToggleVst3Gui => NativeVideoOutputEvent::ToggleVst3Gui,
        Command::ToggleAudioMode => NativeVideoOutputEvent::ToggleAudioMode,
        Command::CloseFullscreen => NativeVideoOutputEvent::CloseFullscreen { generation },
        Command::ToggleWindowMode => NativeVideoOutputEvent::ToggleWindowMode,
        Command::SetVst3PanelVisible { visible } => {
            NativeVideoOutputEvent::SetVst3PanelVisible { visible }
        }
        Command::SetVst3VideoCompact { compact } => {
            NativeVideoOutputEvent::SetVst3VideoCompact { compact }
        }
        Command::SetVst3PanelPos { pos } => NativeVideoOutputEvent::SetVst3PanelPos { pos },
        Command::Vst3ShowSlotGui { idx, path } => {
            NativeVideoOutputEvent::Vst3ShowSlotGui { idx, path }
        }
        Command::Vst3HideSlotGui { idx, path } => {
            NativeVideoOutputEvent::Vst3HideSlotGui { idx, path }
        }
        Command::Vst3SetBypass { idx, path, bypass } => {
            NativeVideoOutputEvent::Vst3SetBypass { idx, path, bypass }
        }
        Command::VideoAdjustLoadSlot { slot_idx } => {
            NativeVideoOutputEvent::VideoAdjustLoadSlot { slot_idx }
        }
        Command::VideoAdjustSaveSlot { slot_idx } => {
            NativeVideoOutputEvent::VideoAdjustSaveSlot { slot_idx }
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
        Command::SetVideoAdjustments {
            adjustments,
            persist,
        } => NativeVideoOutputEvent::SetVideoAdjustments {
            adjustments,
            persist,
        },
        Command::SetPlaybackSpeed { speed } => NativeVideoOutputEvent::SetPlaybackSpeed { speed },
        Command::CopyFrameToClipboard => NativeVideoOutputEvent::CopyFrameToClipboard,
        Command::FrameStep { direction } => NativeVideoOutputEvent::FrameStep { direction },
        Command::ToggleLoop => NativeVideoOutputEvent::ToggleLoop,
        Command::ToggleContinuous => NativeVideoOutputEvent::ToggleContinuous,
        Command::AddBookmarkAt { target_secs } => {
            NativeVideoOutputEvent::AddBookmarkAt { target_secs }
        }
        Command::SetPinAt { target_secs } => NativeVideoOutputEvent::SetPinAt { target_secs },
        Command::JumpMarker { next } => NativeVideoOutputEvent::JumpMarker { next },
        Command::SaveFrameToFile => NativeVideoOutputEvent::SaveFrameToFile,
        Command::SetBookmarkTitle { id, title } => {
            NativeVideoOutputEvent::SetBookmarkTitle { id, title }
        }
        Command::DeleteBookmark { id } => NativeVideoOutputEvent::DeleteBookmark { id },
        Command::DeletePin => NativeVideoOutputEvent::DeletePin,
        Command::BulkAddBookmarks { entries } => {
            NativeVideoOutputEvent::BulkAddBookmarks { entries }
        }
        Command::ExportBookmarksToClipboard { seconds_only } => {
            NativeVideoOutputEvent::ExportBookmarksToClipboard { seconds_only }
        }
        Command::ClearAllBookmarksForCurrent => NativeVideoOutputEvent::ClearAllBookmarksForCurrent,
        Command::OpenExternalUrl { url } => NativeVideoOutputEvent::OpenExternalUrl { url },
        Command::SetRating { stars } => NativeVideoOutputEvent::SetRating { stars },
        Command::ToggleTag { name } => NativeVideoOutputEvent::ToggleTag { name },
        Command::AddTag { name } => NativeVideoOutputEvent::AddTag { name },
        Command::RemoveTag { name } => NativeVideoOutputEvent::RemoveTag { name },
        Command::OpenTagViewForTag { name } => NativeVideoOutputEvent::OpenTagViewForTag { name },
        Command::ToggleNormalize => NativeVideoOutputEvent::ToggleNormalize,
        Command::DisableNormalize => NativeVideoOutputEvent::DisableNormalize,
        Command::CancelNormalizeScan => NativeVideoOutputEvent::CancelNormalizeScan,
    };
    send_native_output_event(tx, source_epoch, event);
}

#[cfg(windows)]
fn native_window_mode_for_placement(
    placement: NativeVideoPlacement,
    rect: windows::Win32::Foundation::RECT,
) -> crate::video::native_window::NativeVideoWindowMode {
    match placement {
        NativeVideoPlacement::MainWindowChild => {
            crate::video::native_window::NativeVideoWindowMode::Child { rect }
        }
        NativeVideoPlacement::DetachedViewerChild => {
            crate::video::native_window::NativeVideoWindowMode::Child { rect }
        }
        NativeVideoPlacement::FullscreenBorderless => {
            crate::video::native_window::NativeVideoWindowMode::Borderless { rect }
        }
        NativeVideoPlacement::DetachedWindow => {
            crate::video::native_window::NativeVideoWindowMode::WindowedAt { rect }
        }
    }
}

#[cfg(windows)]
fn native_window_owner_for_placement(owner_hwnd: u64, placement: NativeVideoPlacement) -> u64 {
    match placement {
        NativeVideoPlacement::DetachedWindow => 0,
        NativeVideoPlacement::MainWindowChild
        | NativeVideoPlacement::DetachedViewerChild
        | NativeVideoPlacement::FullscreenBorderless => owner_hwnd,
    }
}

#[cfg(windows)]
fn native_hud_overlay_enabled_for_placement(
    config: &NativeVideoOutputConfig,
    placement: NativeVideoPlacement,
) -> bool {
    native_hud_overlay_enabled_for_values(
        config.hud_overlay_enabled,
        placement,
        native_video_env_flag_disabled("MIV_HUD_OVERLAY"),
    )
}

#[cfg(windows)]
fn native_hud_overlay_enabled_for_values(
    hud_overlay_enabled: bool,
    placement: NativeVideoPlacement,
    env_disabled: bool,
) -> bool {
    let placement_supports_hud = match placement {
        NativeVideoPlacement::MainWindowChild
        | NativeVideoPlacement::DetachedViewerChild
        | NativeVideoPlacement::DetachedWindow => false,
        NativeVideoPlacement::FullscreenBorderless => true,
    };
    hud_overlay_enabled && placement_supports_hud && !env_disabled
}

#[cfg(windows)]
struct NativeWindowAttach {
    targets: crate::video::native_window_host::NativeRenderTargets,
    width: u32,
    height: u32,
    pixels_per_point: f32,
    observation: crate::video::native_window_host::NativeWindowObservation,
}

#[cfg(windows)]
fn wait_for_native_window_attach(
    pump: &native_window_pump::NativeWindowPumpRenderClient,
    request: u64,
    epoch: u64,
    cancel: &AtomicBool,
) -> Result<NativeWindowAttach, String> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("native window attach cancelled".to_string());
        }
        match pump.recv_lifecycle_timeout(std::time::Duration::from_millis(10)) {
            Ok(native_window_pump::PumpLifecycleEvent::Attach {
                request: actual_request,
                epoch: actual_epoch,
                targets,
                width,
                height,
                pixels_per_point,
                observation,
            }) if actual_request == request && actual_epoch == epoch => {
                return Ok(NativeWindowAttach {
                    targets: targets.into_targets(),
                    width,
                    height,
                    pixels_per_point,
                    observation,
                });
            }
            Ok(native_window_pump::PumpLifecycleEvent::Fault { message, .. }) => {
                return Err(message);
            }
            Ok(native_window_pump::PumpLifecycleEvent::Shutdown) => {
                return Err("native window pump shut down during attach".to_string());
            }
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err("native window pump disconnected during attach".to_string());
            }
        }
    }
}

#[cfg(windows)]
fn wait_for_native_window_published(
    pump: &native_window_pump::NativeWindowPumpRenderClient,
    request: u64,
    epoch: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("native window publish cancelled".to_string());
        }
        match pump.recv_lifecycle_timeout(std::time::Duration::from_millis(10)) {
            Ok(native_window_pump::PumpLifecycleEvent::Published {
                request: actual_request,
                epoch: actual_epoch,
            }) if actual_request == request && actual_epoch == epoch => return Ok(()),
            Ok(native_window_pump::PumpLifecycleEvent::Fault { message, .. }) => {
                return Err(message);
            }
            Ok(native_window_pump::PumpLifecycleEvent::Shutdown) => {
                return Err("native window pump shut down during publish".to_string());
            }
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err("native window pump disconnected during publish".to_string());
            }
        }
    }
}

#[cfg(windows)]
fn wait_for_native_window_resize(
    pump: &native_window_pump::NativeWindowPumpRenderClient,
    epoch: u64,
    cancel: &AtomicBool,
) -> Result<(u32, u32), String> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("native window resize cancelled".to_string());
        }
        match pump.recv_lifecycle_timeout(std::time::Duration::from_millis(10)) {
            Ok(native_window_pump::PumpLifecycleEvent::Resized {
                epoch: actual_epoch,
                width,
                height,
            }) if actual_epoch == epoch => return Ok((width, height)),
            Ok(native_window_pump::PumpLifecycleEvent::Fault { message, .. }) => {
                return Err(message);
            }
            Ok(native_window_pump::PumpLifecycleEvent::Shutdown) => {
                return Err("native window pump shut down during resize".to_string());
            }
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err("native window pump disconnected during resize".to_string());
            }
        }
    }
}

#[cfg(windows)]
fn wait_for_native_window_visibility(
    pump: &native_window_pump::NativeWindowPumpRenderClient,
    epoch: u64,
    visible: bool,
    cancel: &AtomicBool,
) -> Result<(), String> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("native window visibility cancelled".to_string());
        }
        match pump.recv_lifecycle_timeout(std::time::Duration::from_millis(10)) {
            Ok(native_window_pump::PumpLifecycleEvent::VisibilityApplied {
                epoch: actual_epoch,
                visible: actual_visible,
            }) if actual_epoch == epoch && actual_visible == visible => return Ok(()),
            Ok(native_window_pump::PumpLifecycleEvent::Fault { request, message }) => {
                return Err(format!("window request {request} failed: {message}"));
            }
            Ok(native_window_pump::PumpLifecycleEvent::Shutdown) => {
                return Err("native window pump shut down during visibility".to_string());
            }
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err("native window pump disconnected during visibility".to_string());
            }
        }
    }
}

#[cfg(windows)]
fn run_native_video_output(
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    last_displayed_pts_bits: Arc<AtomicU64>,
    frame_step_active: Arc<AtomicBool>,
    duration_secs_bits: Arc<AtomicU64>,
    config: NativeVideoOutputConfig,
    command_rx: NativeCommandReceiver,
    ui_event_tx: NativeOutputEventSender,
    cancel: Arc<AtomicBool>,
    first_presented_out: Arc<AtomicBool>,
    perf_overlay_visible: Arc<AtomicBool>,
    presenter_visibility: NativePresenterVisibility,
    dynamic: Arc<crate::video::decoder::VideoDynamicState>,
    audio_diagnostics: Arc<crate::video::audio_diagnostics::AudioDiagnostics>,
    window_pump: &native_window_pump::NativeWindowPumpRenderClient,
    health: Arc<native_window_health::NativeWindowHealth>,
) -> Result<(), String> {
    use std::time::{Duration, Instant};

    let _com = NativeComApartment::init()?;
    let width = (config.rect.right - config.rect.left).max(1) as u32;
    let height = (config.rect.bottom - config.rect.top).max(1) as u32;
    let mut cur_generation: u64 = 1;
    let mut next_window_epoch = cur_generation;
    let mut cur_window_request: u64 = 1;
    window_pump.open(native_window_pump::PumpPlacementRequest {
        request: cur_window_request,
        epoch: cur_generation,
        placement: config.placement,
        owner_hwnd: config.owner_hwnd,
        rect: config.rect,
        activate_on_show: config.activate_on_show,
        initially_visible: config.initial_visibility.is_visible(),
    })?;
    let initial_attach =
        wait_for_native_window_attach(&window_pump, cur_window_request, cur_generation, &cancel)?;
    let (new_presenter, topology, startup_window_intents) =
        crate::video::native_presenter::NativeRenderCore::new(
            crate::video::native_presenter::NativeRenderConfig {
                targets: initial_attach.targets,
                width: initial_attach.width,
                height: initial_attach.height,
                os_pixels_per_point: initial_attach.pixels_per_point,
                initial_observation: initial_attach.observation,
                test_overlay: std::env::var_os("MIV_NATIVE_VIDEO_TEST_OVERLAY").is_some(),
                egui_overlay: native_video_env_flag_enabled("MIV_NATIVE_VIDEO_EGUI_OVERLAY", true),
                cursor_hide_delay_secs: config.cursor_hide_delay_secs,
                ui_scale: config.ui_scale,
                text_contrast: config.text_contrast,
                ui_font: config.ui_font.clone(),
                scale_filter: config.scale_filter,
                health: Arc::clone(&health),
                window_epoch: cur_generation,
            },
        )?;
    let mut presenter = new_presenter;
    if cancel.load(Ordering::Acquire) {
        first_presented_out.store(false, Ordering::Release);
        window_pump.shutdown(cur_window_request.saturating_add(1));
        presenter.detach();
        return Ok(());
    }
    crate::logger::log(format!(
        "[native-video] render initialized placement={} rect=({},{} {}x{}) sync_interval={}",
        config.placement.label(),
        config.rect.left,
        config.rect.top,
        width,
        height,
        config.sync_interval
    ));
    // Plan B: viewer presentation 切替 (`SwitchPlacement`) で presenter を作り直す
    // とき、新 presenter に再適用するための現行状態。`config.*` は初期値しか持たない
    // ため、command で更新されうる値はここで追跡する。
    //   - `cur_checked` / `cur_vst3_available` / `cur_text_contrast`: HUD 状態
    //     (`SetChecked` / `SetVst3Available` / `SetTextContrast` は `NativeVideoOutput` 側で
    //     dedup されるため、再構築後の新 presenter には command が来ない可能性がある
    //     → ここから直接再適用する)。
    //   - `cur_video_geometry`: SAR + display matrix。info 到着時に 1 度だけ送られるので、
    //     再構築後は新 presenter へ手動で再適用する必要がある。
    let mut cur_placement = config.placement;
    let mut cur_owner_hwnd = config.owner_hwnd;
    let mut cur_checked = config.checked;
    let mut cur_vst3_available = config.vst3_available;
    let mut cur_text_contrast = config.text_contrast;
    let cur_ui_scale = crate::settings::normalize_ui_scale_factor(config.ui_scale);
    let mut cur_hud_dimmed = false;
    let mut cur_video_geometry: Option<(u32, u32, display_metadata::VideoOrientation)> = None;
    // **review #12 対応**: SwitchPlacement で presenter を作り直したとき再適用が
    // 漏れていた現行値。App 側は loop / continuous / compact を「ユーザー操作時のみ
    // push」するため、これらを presenter 側で覚えておかないと SwitchPlacement 後の
    // 新 presenter が default に戻る。
    let mut cur_loop_enabled: Option<bool> = None;
    let mut cur_loop_mode: Option<crate::settings::VideoLoopMode> = None;
    let mut cur_continuous_mode: Option<VideoContinuousMode> = None;
    let mut cur_video_compact: Option<bool> = None;
    let mut cur_fallback_file_name = config.fallback_file_name.clone();
    let mut cur_video_grade = config.video_grade.clone();
    fn sync_hud_regions(
        window_pump: &native_window_pump::NativeWindowPumpRenderClient,
        epoch: u64,
        presenter: &crate::video::native_presenter::NativeRenderCore,
        outcome: &crate::video::native_presenter::NativeOverlayInputOutcome,
    ) {
        window_pump.publish_visual(native_window_pump::NativeWindowVisualUpdate {
            epoch,
            window_intents: outcome.window_intents.clone(),
            hud_regions: Some(outcome.hud_regions.clone()),
            toast_active: presenter.overlay_toast_active(),
            fullscreen_overlay_active: presenter.fullscreen_overlay_active(),
            debug_description: presenter.hud_debug_description(&outcome.hud_regions),
        });
    }

    let run_started = Instant::now();
    presenter.set_overlay_vst3_available(config.vst3_available);
    presenter.set_video_grade(cur_video_grade.clone())?;
    presenter.set_overlay_audio_only(config.audio_only);
    presenter.set_overlay_checked(config.checked);
    presenter.set_overlay_fallback_file_name(cur_fallback_file_name.clone());
    if config.initial_tile_overlay {
        presenter.set_overlay_tile_overlay(Some(
            crate::video::native_presenter::NativeOverlayTileOverlay::preparing_with_filename(
                cur_fallback_file_name.clone(),
            ),
        ));
        match presenter.tick_overlay_video_state(
            clock.now_secs(),
            f64::from_bits(duration_secs_bits.load(Ordering::Acquire)),
            clock.is_playing(),
            clock.volume(),
            clock.is_muted(),
            clock.limiter_ceiling_hit_seq(),
            clock.playback_speed(),
            frame_step_active.load(Ordering::Acquire),
            clock.is_seeking(),
            clock.current_seek_serial(),
        ) {
            Ok(outcome) => sync_hud_regions(&window_pump, cur_generation, &presenter, &outcome),
            Err(err) => crate::logger::log(format!(
                "[native-video] initial tile overlay render failed: {err}"
            )),
        }
    }
    // Publish the native window as soon as the overlay can draw its center status.
    // Broken/unsupported videos may never produce a first frame, so waiting until
    // first-present leaves the user on the egui fallback surface with the native
    // video input/HUD path inactive. Showing the presenter early gives the
    // preparing/error overlay a real HWND and keeps wheel/hover/Esc routing
    // consistent with normal video playback.
    if !config.initial_tile_overlay {
        match presenter.tick_overlay_video_state(
            clock.now_secs(),
            f64::from_bits(duration_secs_bits.load(Ordering::Acquire)),
            clock.is_playing(),
            clock.volume(),
            clock.is_muted(),
            clock.limiter_ceiling_hit_seq(),
            clock.playback_speed(),
            frame_step_active.load(Ordering::Acquire),
            clock.is_seeking(),
            clock.current_seek_serial(),
        ) {
            Ok(outcome) => sync_hud_regions(&window_pump, cur_generation, &presenter, &outcome),
            Err(err) => crate::logger::log(format!(
                "[native-video] initial preparing overlay render failed: {err}"
            )),
        }
    }
    window_pump.target_ready(
        cur_window_request,
        cur_generation,
        topology,
        startup_window_intents,
    )?;
    wait_for_native_window_published(&window_pump, cur_window_request, cur_generation, &cancel)?;
    // 音声のみ native シェル (Inc 6 ②-1): 映像 first frame が永久に来ないので、window を
    // publish できた時点で presenter を「表示準備完了」とみなす。これをしないと
    // `native_presenter_pending()` (= `!first_presented`) が永久に true のままになり、
    // UI 側が「準備中」表示で固着する。動画経路 (audio_only=false) には触れない。
    if config.audio_only {
        first_presented_out.store(true, Ordering::Release);
    }

    let mut source = PresenterSourceState::new(SwitchSourcePayload {
        video_rx,
        clock,
        engine_event_tx,
        displayed_frame_seq,
        last_displayed_pts_bits,
        frame_step_active,
        duration_secs_bits,
        dynamic,
        audio_diagnostics,
        source_epoch: 0,
        fallback_file_name: cur_fallback_file_name.clone(),
        show_preparing_overlay: config.initial_tile_overlay,
    });
    health.record_source_generation(source.source_epoch);
    let mut last_summary_log = Instant::now();
    let mut last_present_log = Instant::now();
    let mut last_overlay_tick = Instant::now();
    let mut published_overlay_input_routing =
        crate::video::native_presenter::NativeOverlayInputRouting::default();
    let mut last_source_state_probe = Instant::now();
    let mut last_vram_trace = run_started
        .checked_sub(Duration::from_secs(1))
        .unwrap_or(run_started);
    let mut vram_trace_deadlines: Vec<(Instant, &'static str)> = Vec::new();
    // CP6: cursor polling 用 state。HUD topology があるときだけ
    // 機能する。フォールバック経路では `presenter.cursor_polling_tick` が早期 return。
    let mut last_cursor_poll: Option<Instant> = None;
    let mut last_native_mouse_at: Option<Instant> = None;
    let mut pointer_present_synthetic: bool = false;
    let mut window_observation = initial_attach.observation;
    let startup_probe_until = run_started + Duration::from_secs(5);
    let mut last_startup_probe = run_started
        .checked_sub(Duration::from_millis(250))
        .unwrap_or(run_started);
    let mut startup_probe_count = 0_u32;
    let mut first_present_probe_logged = false;
    let mut native_events = Vec::new();
    let trace_every_present = std::env::var_os("MIV_NATIVE_VIDEO_PRESENT_TRACE").is_some();
    let mut pending_navigation_preview_clear_at: Option<Instant> = None;
    const NAVIGATION_PREVIEW_CLEAR_DELAY: Duration = Duration::from_millis(40);
    // The current frame and disposal-only retire queue have one output-local
    // owner. `source.queue` remains an unpresented pacing queue and is never a
    // placement/grade fallback.
    let mut frame_output = NativeFrameOutputContext::new();
    /// presenter 側 `source.queue` の最大長 (Codex 助言、2026-05-15)。旧コードは
    /// `video_rx.try_recv()` を空になるまで drain して queue に積み込んでいたため、
    /// 高負荷 / pool exhausted 時に queue が 23 まで肥大化 → present_retire / shared
    /// pool / cache 合算で adapter memory が枯渇し wgpu OOM していた。queue を 8 で
    /// cap し、超えたら decoder 側に back-pressure を返す (= video_tx (cap=8) が満杯に
    /// なって decoder が `try_send` 失敗 → 古い frame を drop して新 frame に置換)。
    /// 30fps で約 270ms 分 = pacing 上は十分。これは visible の display pacing 専用で、
    /// hidden は EOF まで decoder を進めるため全件 drain する。
    const MAX_NATIVE_SOURCE_QUEUE: usize = 8;
    // Inc 7 hidden presenter (動画→音声モード): pump が公開する可視状態を consume policy の
    // 唯一の正本にする。source swap はこの output-lifetime state を交換しない。
    let mut hidden_frame_scratch = Vec::with_capacity(MAX_NATIVE_SOURCE_QUEUE.saturating_mul(2));
    emit_native_vram_trace(
        "native_presenter_started",
        "after_presenter_init",
        &source,
        &presenter,
        frame_output.retire_len(),
    );
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

        if let Some(observation) = window_pump.take_observation()
            && observation.epoch == cur_generation
        {
            window_observation = observation.value;
            presenter.set_window_observation(window_observation);
        }
        loop {
            match window_pump.try_recv_lifecycle() {
                Ok(native_window_pump::PumpLifecycleEvent::Fault { message, .. }) => {
                    return Err(message);
                }
                Ok(native_window_pump::PumpLifecycleEvent::Detached { epoch })
                    if epoch == cur_generation =>
                {
                    presenter.detach();
                    return Ok(());
                }
                Ok(native_window_pump::PumpLifecycleEvent::Shutdown) => {
                    presenter.detach();
                    return Ok(());
                }
                Ok(_) => {}
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    return Err("native window pump lifecycle disconnected".to_string());
                }
            }
        }
        let now = Instant::now();
        if crate::perf::is_enabled() {
            if now.duration_since(last_vram_trace) >= Duration::from_secs(1) {
                emit_native_vram_trace(
                    "snapshot",
                    "native_loop_1hz",
                    &source,
                    &presenter,
                    frame_output.retire_len(),
                );
                last_vram_trace = now;
            }
            let mut i = 0usize;
            while i < vram_trace_deadlines.len() {
                if now >= vram_trace_deadlines[i].0 {
                    let (_, reason) = vram_trace_deadlines.swap_remove(i);
                    emit_native_vram_trace(
                        "deferred",
                        reason,
                        &source,
                        &presenter,
                        frame_output.retire_len(),
                    );
                } else {
                    i += 1;
                }
            }
        }
        if pending_navigation_preview_clear_at.is_some_and(|deadline| now >= deadline) {
            pending_navigation_preview_clear_at = None;
            presenter.set_overlay_navigation_preview(None);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "navigation_preview_clear",
                    None,
                    0,
                    &[
                        (
                            "source_epoch",
                            serde_json::Value::from(source.source_epoch as i64),
                        ),
                        (
                            "delay_ms",
                            serde_json::Value::from(
                                NAVIGATION_PREVIEW_CLEAR_DELAY.as_secs_f64() * 1000.0,
                            ),
                        ),
                    ],
                );
            }
        }
        if now < startup_probe_until
            && now.duration_since(last_startup_probe) >= Duration::from_millis(250)
        {
            last_startup_probe = now;
            startup_probe_count = startup_probe_count.saturating_add(1);
            let first_presented = first_presented_out.load(Ordering::Acquire);
            let displayed_seq = source.displayed_frame_seq.load(Ordering::Acquire);
            let foreground_current_process = window_observation.focus.foreground_is_current_process;
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
        for command in command_rx.drain() {
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
                NativeVideoOutputCommand::SetVideoGrade { grade } => {
                    let render_grade_changed = video_grade_render_changed(&cur_video_grade, &grade);
                    cur_video_grade = grade;
                    match presenter.set_video_grade(cur_video_grade.clone()) {
                        Ok(()) => {
                            // A grade command is the only paused-state refresh trigger.
                            // Empty, Hidden, and render-equivalent snapshots stay idle.
                            if frame_output.should_represent_for_grade_change(render_grade_changed)
                            {
                                let refresh = frame_output
                                    .visible_frame()
                                    .map(|frame| presenter.present(frame, config.sync_interval));
                                match refresh {
                                    Some(Ok(outcome)) => {
                                        debug_assert!(
                                            frame_output
                                                .mark_current_visible(outcome.copy_fence_value)
                                        );
                                        frame_output.retire_completed(
                                            presenter.copy_fence_completed_value(),
                                        );
                                    }
                                    Some(Err(error)) => crate::logger::log(format!(
                                        "[native-video] grade refresh present failed: {error}"
                                    )),
                                    None => {}
                                }
                            }
                        }
                        Err(error) => crate::logger::log(format!(
                            "[native-video] Creative LUT shader update failed: {error}"
                        )),
                    }
                }
                NativeVideoOutputCommand::SetMetadata { metadata } => {
                    presenter.set_overlay_metadata(metadata);
                }
                NativeVideoOutputCommand::SetSidePanelState {
                    mode,
                    info_panel_open,
                } => {
                    presenter.set_overlay_side_panel_state(mode, info_panel_open);
                }
                NativeVideoOutputCommand::ResetSidePanelSession => {
                    presenter.reset_overlay_side_panel_session();
                }
                NativeVideoOutputCommand::SetLoopEnabled { enabled } => {
                    cur_loop_enabled = Some(enabled);
                    presenter.set_overlay_loop_enabled(enabled);
                }
                NativeVideoOutputCommand::SetLoopMode { mode } => {
                    cur_loop_mode = Some(mode);
                    presenter.set_overlay_loop_mode(mode);
                }
                NativeVideoOutputCommand::SetContinuousMode { mode } => {
                    cur_continuous_mode = Some(mode);
                    presenter.set_overlay_continuous_mode(mode);
                }
                NativeVideoOutputCommand::SetVst3Available { available } => {
                    cur_vst3_available = available;
                    presenter.set_overlay_vst3_available(available);
                }
                NativeVideoOutputCommand::SetHudDimmed { dimmed } => {
                    cur_hud_dimmed = dimmed;
                    presenter.set_overlay_hud_dimmed(dimmed);
                }
                NativeVideoOutputCommand::SetTextContrast { contrast } => {
                    cur_text_contrast = contrast;
                    presenter.set_overlay_text_contrast(contrast);
                }
                NativeVideoOutputCommand::SetChecked { checked } => {
                    cur_checked = checked;
                    presenter.set_overlay_checked(checked);
                }
                NativeVideoOutputCommand::SetVideoCompact { compact } => {
                    cur_video_compact = Some(compact);
                    if let Err(err) = presenter.set_video_compact(compact) {
                        crate::logger::log(format!(
                            "[native-video] set compact transform failed: {err}"
                        ));
                    }
                }
                NativeVideoOutputCommand::SetVideoGeometry {
                    num,
                    den,
                    orientation,
                } => {
                    cur_video_geometry = Some((num, den, orientation));
                    if let Err(err) = presenter.set_video_geometry(num, den, orientation) {
                        crate::logger::log(format!(
                            "[native-video] set display geometry failed: {err}"
                        ));
                    }
                }
                NativeVideoOutputCommand::SetVst3Panel { panel } => {
                    presenter.set_overlay_vst3_panel(panel);
                }
                NativeVideoOutputCommand::SetPlaybackStatus {
                    first_frame_presented,
                    error,
                    prep_status,
                } => {
                    // Playback status is driven from the UI thread and can lag behind
                    // a queued SwitchSource command. During deferred wheel navigation
                    // the shared NativeVideoOutput may still report the previous
                    // source as presented until the presenter thread consumes
                    // SwitchSource, so clearing the navigation preview here can expose
                    // the previous video frame for one compositor pass. Keep preview
                    // lifetime tied to the actual present path; errors still clear it
                    // so the failure HUD can be seen.
                    if error.is_some() {
                        pending_navigation_preview_clear_at = None;
                        presenter.set_overlay_navigation_preview(None);
                    }
                    presenter.set_overlay_playback_status(
                        first_frame_presented,
                        error,
                        prep_status,
                    );
                }
                NativeVideoOutputCommand::ShowToast {
                    text,
                    centered,
                    linger,
                } => {
                    presenter.show_overlay_toast(text, centered, linger);
                }
                NativeVideoOutputCommand::SetTileOverlay { tile_overlay } => {
                    presenter.set_overlay_tile_overlay(tile_overlay);
                }
                NativeVideoOutputCommand::SetRingPickerOverlay { overlay } => {
                    presenter.set_overlay_ring_picker(overlay);
                }
                NativeVideoOutputCommand::SetRingGuideOverlay { overlay } => {
                    presenter.set_overlay_ring_guide(overlay);
                }
                NativeVideoOutputCommand::SetNavigationPreview { preview } => {
                    pending_navigation_preview_clear_at = None;
                    presenter.set_overlay_navigation_preview(preview);
                }
                NativeVideoOutputCommand::MarkCursorActivity => {
                    window_pump.mark_cursor_activity(cur_generation)?;
                    presenter.mark_overlay_cursor_activity();
                }
                NativeVideoOutputCommand::RequestOverlayRender => {
                    presenter.request_overlay_render();
                }
                NativeVideoOutputCommand::SetNormalizeOverlayState { state } => {
                    presenter.set_overlay_normalize_state(state);
                }
                NativeVideoOutputCommand::RaiseHudToTop => {
                    window_pump.raise_hud(cur_generation)?;
                }
                NativeVideoOutputCommand::RaisePresenterToFront => {
                    window_pump.raise_presenter(cur_generation)?;
                }
                NativeVideoOutputCommand::SetWindowVisible { visible } => {
                    if visible {
                        // Hidden presenter show (音声モード→動画 / tray restore): hold していた最新
                        // フレームを 1 回 present してから再表示する。これで音声モード中に
                        // seek していても正しい位置の映像で復帰し、hide 前の古いフレームが
                        // 一瞬見える flash を防ぐ。present は通常経路と同じ retire 管理を通す。
                        let hidden_present = if frame_output.is_hidden() {
                            frame_output.frame().map(|frame| {
                                (
                                    frame.pts_secs,
                                    presenter.present(frame, config.sync_interval),
                                )
                            })
                        } else {
                            None
                        };
                        if let Some((pts, result)) = hidden_present {
                            match result {
                                Ok(outcome) => {
                                    source
                                        .last_displayed_pts_bits
                                        .store(pts.to_bits(), Ordering::Release);
                                    debug_assert!(
                                        frame_output.mark_current_visible(outcome.copy_fence_value)
                                    );
                                    frame_output
                                        .retire_completed(presenter.copy_fence_completed_value());
                                }
                                Err(err) => {
                                    crate::logger::log(format!(
                                        "[native-video] hidden-show present failed: {err}"
                                    ));
                                    frame_output.clear_current();
                                }
                            }
                        }
                        window_pump.set_visibility(cur_generation, true)?;
                        wait_for_native_window_visibility(
                            window_pump,
                            cur_generation,
                            true,
                            &cancel,
                        )?;
                        crate::logger::log(
                            "[native-video] presenter show requested (visibility transition)"
                                .to_string(),
                        );
                    } else {
                        // Hidden presenter hide (動画→音声モード / tray residency):
                        // presenter ウィンドウと
                        // HUD overlay を隠す。以降の present ループは consume-and-hold に入り、
                        // present() を呼ばず最新フレームだけ hold する (音声は無中断)。
                        window_pump.set_visibility(cur_generation, false)?;
                        wait_for_native_window_visibility(
                            window_pump,
                            cur_generation,
                            false,
                            &cancel,
                        )?;
                        frame_output.hide();
                        crate::logger::log(
                            "[native-video] presenter hidden (visibility transition)".to_string(),
                        );
                    }
                }
                NativeVideoOutputCommand::SwitchSource { payload } => {
                    emit_native_vram_trace(
                        "switch_source_begin",
                        "before_drain_old_source",
                        &source,
                        &presenter,
                        frame_output.retire_len(),
                    );
                    native_drain_unpresented_queue(&mut source.queue);
                    // The current typed frame belongs to the old source. Visible frames
                    // move to the disposal queue; Hidden frames return to producer-side
                    // recovery without becoming a fallback for the new source.
                    frame_output.clear_current();
                    // present_retire の OLD source 由来エントリのうち fence が完了したものを
                    // 解放する (rapid swap で旧 slot が retire に滞留して共有出力プールを
                    // 圧迫するのを防ぐ)。fence ゲート付きなので未完コピーは解放しない (安全)。
                    if let Some(completed) = presenter.copy_fence_completed_value() {
                        frame_output.retire_completed(Some(completed));
                    }
                    // 旧 source の `shared_texture_cache` (presenter 側 D3D11 共有 texture
                    // キャッシュ、4K で 32 MB/枚) を即時破棄し adapter memory を解放する
                    // (Codex 助言 2026-05-15、wgpu OOM 対策)。新 source は新 shared_handle で
                    // 再キャッシュされる。
                    presenter.clear_shared_texture_cache();
                    emit_native_vram_trace(
                        "switch_source_after_clear",
                        "after_drain_retire_and_shared_cache_clear",
                        &source,
                        &presenter,
                        frame_output.retire_len(),
                    );
                    let show_preparing_overlay = payload.show_preparing_overlay;
                    cur_fallback_file_name = payload.fallback_file_name.clone();
                    source = PresenterSourceState::new(*payload);
                    health.record_source_generation(source.source_epoch);
                    emit_native_vram_trace(
                        "switch_source_attached",
                        "after_new_source_attached",
                        &source,
                        &presenter,
                        frame_output.retire_len(),
                    );
                    let switch_trace_now = Instant::now();
                    vram_trace_deadlines.push((
                        switch_trace_now + Duration::from_millis(250),
                        "switch_source_250ms",
                    ));
                    vram_trace_deadlines.push((
                        switch_trace_now + Duration::from_secs(1),
                        "switch_source_1s",
                    ));
                    vram_trace_deadlines.push((
                        switch_trace_now + Duration::from_secs(3),
                        "switch_source_3s",
                    ));
                    first_presented_out.store(false, Ordering::Release);
                    pending_navigation_preview_clear_at = None;
                    // ソース切替時は新動画のオープン前なので prep_status は初期値で送る。
                    // 直後に decoder thread が format::input を始めると、次の tick で
                    // 新しい snapshot が押し込まれ、HUD 文言が更新される。
                    presenter.set_overlay_playback_status(
                        false,
                        None,
                        crate::video::avio_progress::PreparingStatus {
                            phase: crate::video::avio_progress::prep_phase::OPENING,
                            bytes_read: 0,
                            file_size: 0,
                        },
                    );
                    presenter.set_overlay_fallback_file_name(cur_fallback_file_name.clone());
                    presenter.set_overlay_metadata(None);
                    presenter.set_overlay_timeline_markers(Vec::new());
                    presenter.set_overlay_jump_entries(Vec::new());
                    presenter.reset_overlay_source_session();
                    // 前ソースの perf 履歴 (interval_ms / source_delta_ms / av_offset_ms)
                    // が残ったまま新ソースの最初のサンプルが入ると、median ベースの Y 軸が
                    // 古い fps を引きずって新サンプル蓄積後にガクッと切り替わる。新動画は
                    // 解像度 / fps / 同期特性が違う前提なので、ここで明示クリアする。
                    presenter.reset_overlay_perf();
                    // hover preview の transient state も新ソース用にクリアする。
                    // overlay は「直近のサムネ画像を常に描く」ようになったため、
                    // ここでクリアしないと、ソース切替直後の最初の hover preview で
                    // 前の動画のサムネが (場合によっては「シーク中」box も出ずに)
                    // 表示されてしまう。新 player の thumb worker は別 path で
                    // 改めてサムネを生成するので、None 始まりで問題ない。
                    presenter.set_overlay_hover_thumbnail(None);
                    presenter.set_overlay_hover_preview_pinned(false);
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
                    }
                    if show_preparing_overlay {
                        // The new player will resend fresh overlay content; keep the
                        // tile surface visible while its VideoInfo and thumbnails load.
                        presenter.set_overlay_tile_overlay(Some(
                            crate::video::native_presenter::NativeOverlayTileOverlay::preparing_with_filename(
                                cur_fallback_file_name.clone(),
                            ),
                        ));
                    }
                    if !source.clock.is_playing() {
                        source.last_present_wall = None;
                        source.last_present_source_pts = None;
                    }
                }
                NativeVideoOutputCommand::SwitchPlacement {
                    request_id,
                    placement,
                    owner_hwnd,
                    rect: new_rect,
                    activate_on_show,
                    visible,
                } => {
                    if placement == cur_placement && owner_hwnd == cur_owner_hwnd {
                        window_pump.resize(cur_generation, placement, new_rect)?;
                        let (new_width, new_height) =
                            wait_for_native_window_resize(&window_pump, cur_generation, &cancel)?;
                        match presenter.resize(new_width, new_height) {
                            Ok(intents) => window_pump.publish_visual(
                                native_window_pump::NativeWindowVisualUpdate {
                                    epoch: cur_generation,
                                    window_intents: intents,
                                    hud_regions: None,
                                    toast_active: presenter.overlay_toast_active(),
                                    fullscreen_overlay_active: false,
                                    debug_description: None,
                                },
                            ),
                            Err(err) => crate::logger::log(format!(
                                "[native-video] same placement resize failed: {err}"
                            )),
                        }
                        if visible && presenter_visibility.is_hidden() {
                            let hidden_present = if frame_output.is_hidden() {
                                frame_output
                                    .frame()
                                    .map(|frame| presenter.present(frame, config.sync_interval))
                            } else {
                                None
                            };
                            if let Some(result) = hidden_present {
                                match result {
                                    Ok(outcome) => {
                                        debug_assert!(
                                            frame_output
                                                .mark_current_visible(outcome.copy_fence_value)
                                        );
                                        frame_output.retire_completed(
                                            presenter.copy_fence_completed_value(),
                                        );
                                    }
                                    Err(error) => {
                                        crate::logger::log(format!(
                                            "[native-video] same-placement show present failed: {error}"
                                        ));
                                        frame_output.clear_current();
                                    }
                                }
                            }
                            window_pump.set_visibility(cur_generation, true)?;
                            wait_for_native_window_visibility(
                                window_pump,
                                cur_generation,
                                true,
                                &cancel,
                            )?;
                        } else if !visible && !presenter_visibility.is_hidden() {
                            window_pump.set_visibility(cur_generation, false)?;
                            wait_for_native_window_visibility(
                                window_pump,
                                cur_generation,
                                false,
                                &cancel,
                            )?;
                            frame_output.hide();
                        }
                        send_native_output_event(
                            &ui_event_tx,
                            source.source_epoch,
                            NativeVideoOutputEvent::PlacementSwitched {
                                request_id,
                                placement,
                                generation: cur_generation,
                            },
                        );
                        continue;
                    }

                    next_window_epoch = next_window_epoch.saturating_add(1);
                    let candidate_epoch = next_window_epoch;
                    cur_window_request = cur_window_request.saturating_add(1);
                    let host_request = cur_window_request;
                    window_pump.switch(native_window_pump::PumpPlacementRequest {
                        request: host_request,
                        epoch: candidate_epoch,
                        placement,
                        owner_hwnd,
                        rect: new_rect,
                        activate_on_show,
                        initially_visible: visible,
                    })?;
                    let attach = wait_for_native_window_attach(
                        &window_pump,
                        host_request,
                        candidate_epoch,
                        &cancel,
                    )?;
                    let new_presenter_result =
                        crate::video::native_presenter::NativeRenderCore::new(
                            crate::video::native_presenter::NativeRenderConfig {
                                targets: attach.targets,
                                width: attach.width,
                                height: attach.height,
                                os_pixels_per_point: attach.pixels_per_point,
                                initial_observation: attach.observation,
                                test_overlay: std::env::var_os("MIV_NATIVE_VIDEO_TEST_OVERLAY")
                                    .is_some(),
                                egui_overlay: native_video_env_flag_enabled(
                                    "MIV_NATIVE_VIDEO_EGUI_OVERLAY",
                                    true,
                                ),
                                cursor_hide_delay_secs: config.cursor_hide_delay_secs,
                                ui_scale: cur_ui_scale,
                                text_contrast: cur_text_contrast,
                                ui_font: config.ui_font.clone(),
                                scale_filter: config.scale_filter,
                                health: Arc::clone(&health),
                                window_epoch: candidate_epoch,
                            },
                        );
                    let (mut new_presenter, topology, startup_window_intents) =
                        match new_presenter_result {
                            Ok(value) => value,
                            Err(err) => {
                                window_pump.target_failed(host_request, candidate_epoch)?;
                                crate::logger::log(format!(
                                    "[native-video] placement switch target failed: {err}"
                                ));
                                send_native_output_event(
                                    &ui_event_tx,
                                    source.source_epoch,
                                    NativeVideoOutputEvent::PlacementSwitchFailed { request_id },
                                );
                                continue;
                            }
                        };
                    let prepare_result = (|| -> Result<_, String> {
                        new_presenter.set_overlay_vst3_available(
                            cur_vst3_available && placement.is_fullscreen_borderless(),
                        );
                        new_presenter.set_video_grade(cur_video_grade.clone())?;
                        new_presenter.set_overlay_audio_only(config.audio_only);
                        new_presenter.set_overlay_hud_dimmed(cur_hud_dimmed);
                        new_presenter.set_overlay_checked(cur_checked);
                        new_presenter
                            .set_overlay_fallback_file_name(cur_fallback_file_name.clone());
                        if let Some((num, den, orientation)) = cur_video_geometry {
                            new_presenter.set_video_geometry(num, den, orientation)?;
                        }
                        if let Some(enabled) = cur_loop_enabled {
                            new_presenter.set_overlay_loop_enabled(enabled);
                        }
                        if let Some(mode) = cur_loop_mode {
                            new_presenter.set_overlay_loop_mode(mode);
                        }
                        if let Some(mode) = cur_continuous_mode {
                            new_presenter.set_overlay_continuous_mode(mode);
                        }
                        if let Some(compact) = cur_video_compact {
                            new_presenter.set_video_compact(compact)?;
                        }
                        new_presenter.set_overlay_playback_status(
                            first_presented_out.load(Ordering::Acquire),
                            None,
                            crate::video::avio_progress::PreparingStatus {
                                phase: crate::video::avio_progress::prep_phase::DONE,
                                bytes_read: 0,
                                file_size: 0,
                            },
                        );
                        let overlay_outcome = new_presenter.tick_overlay_video_state(
                            source.clock.now_secs(),
                            f64::from_bits(source.duration_secs_bits.load(Ordering::Acquire)),
                            source.clock.is_playing(),
                            source.clock.volume(),
                            source.clock.is_muted(),
                            source.clock.limiter_ceiling_hit_seq(),
                            source.clock.playback_speed(),
                            source.frame_step_active.load(Ordering::Acquire),
                            source.clock.is_seeking(),
                            source.clock.current_seek_serial(),
                        )?;
                        // The typed presentation state is the only prime source.
                        // On failure it stays owned by the old core/host so a rejected
                        // switch cannot destroy the paused-frame fallback.
                        let primed = if let Some(frame) = frame_output.frame() {
                            let outcome = new_presenter.present(frame, config.sync_interval)?;
                            debug_assert!(
                                frame_output.mark_current_visible(outcome.copy_fence_value)
                            );
                            true
                        } else {
                            false
                        };
                        Ok((primed, overlay_outcome))
                    })();
                    let (primed, overlay_outcome) = match prepare_result {
                        Ok(value) => value,
                        Err(err) => {
                            window_pump.target_failed(host_request, candidate_epoch)?;
                            crate::logger::log(format!(
                                "[native-video] placement switch prime failed; old host retained: {err}"
                            ));
                            send_native_output_event(
                                &ui_event_tx,
                                source.source_epoch,
                                NativeVideoOutputEvent::PlacementSwitchFailed { request_id },
                            );
                            continue;
                        }
                    };
                    sync_hud_regions(
                        &window_pump,
                        candidate_epoch,
                        &new_presenter,
                        &overlay_outcome,
                    );
                    frame_output.retire_completed(presenter.copy_fence_completed_value());
                    let old_retire_len = frame_output.retire_len();
                    window_pump.target_ready(
                        host_request,
                        candidate_epoch,
                        topology,
                        startup_window_intents,
                    )?;
                    wait_for_native_window_published(
                        &window_pump,
                        host_request,
                        candidate_epoch,
                        &cancel,
                    )?;
                    let old_presenter = std::mem::replace(&mut presenter, new_presenter);
                    old_presenter.detach();
                    frame_output.invalidate_retire_fence_prefix(old_retire_len);
                    cur_generation = candidate_epoch;
                    cur_placement = placement;
                    cur_owner_hwnd = owner_hwnd;
                    window_observation = attach.observation;
                    presenter.set_window_observation(window_observation);
                    if !visible {
                        frame_output.hide();
                    }
                    native_events.clear();
                    last_cursor_poll = None;
                    last_native_mouse_at = None;
                    pointer_present_synthetic = false;
                    crate::logger::log(format!(
                        "[native-video] placement switched placement={} {}x{} primed={} request={} generation={}",
                        placement.label(),
                        attach.width,
                        attach.height,
                        primed,
                        request_id,
                        cur_generation,
                    ));
                    send_native_output_event(
                        &ui_event_tx,
                        source.source_epoch,
                        NativeVideoOutputEvent::PlacementSwitched {
                            request_id,
                            placement,
                            generation: cur_generation,
                        },
                    );
                }
            }
        }
        native_events.clear();
        native_events.extend(
            window_pump
                .drain_window_events()
                .into_iter()
                .filter(|envelope| {
                    envelope.epoch == cur_generation && envelope.generation == cur_generation
                })
                .map(|envelope| envelope.event),
        );

        let now = Instant::now();
        for event in &native_events {
            use crate::video::native_window::NativeVideoWindowEvent as NEvt;
            match event {
                NEvt::MouseMove(_) => {
                    last_native_mouse_at = Some(now);
                    pointer_present_synthetic = false;
                }
                NEvt::MouseLeave => {
                    pointer_present_synthetic = false;
                }
                NEvt::GeometryChanged { w, h, .. } => match presenter.resize(*w, *h) {
                    Ok(intents) => {
                        window_pump.publish_visual(native_window_pump::NativeWindowVisualUpdate {
                            epoch: cur_generation,
                            window_intents: intents,
                            hud_regions: None,
                            toast_active: presenter.overlay_toast_active(),
                            fullscreen_overlay_active: false,
                            debug_description: None,
                        })
                    }
                    Err(err) => crate::logger::log(format!(
                        "[native-video] GeometryChanged resize_to({w}x{h}) failed: {err}"
                    )),
                },
                NEvt::DpiChanged {
                    dpi,
                    suggested_rect,
                } => {
                    let ppp = (*dpi as f32 / 96.0).max(0.5);
                    presenter.set_overlay_pixels_per_point(ppp);
                    let rect_w = (suggested_rect.right - suggested_rect.left).max(1) as u32;
                    let rect_h = (suggested_rect.bottom - suggested_rect.top).max(1) as u32;
                    match presenter.resize_overlay_surface_only(rect_w, rect_h) {
                        Ok(intents) => window_pump.publish_visual(
                            native_window_pump::NativeWindowVisualUpdate {
                                epoch: cur_generation,
                                window_intents: intents,
                                hud_regions: None,
                                toast_active: presenter.overlay_toast_active(),
                                fullscreen_overlay_active: false,
                                debug_description: None,
                            },
                        ),
                        Err(err) => crate::logger::log(format!(
                            "[native-video] DpiChanged overlay resize failed: {err}"
                        )),
                    }
                }
                _ => {}
            }
        }

        let presenter_hidden = presenter_visibility.is_hidden();
        if presenter_hidden {
            publish_native_overlay_input_routing(
                &ui_event_tx,
                source.source_epoch,
                &mut published_overlay_input_routing,
                crate::video::native_presenter::NativeOverlayInputRouting::default(),
            );
        }
        let cursor_poll_due = !presenter_hidden
            && last_cursor_poll
                .map(|time| now.duration_since(time) >= Duration::from_millis(50))
                .unwrap_or(true);
        if cursor_poll_due {
            last_cursor_poll = Some(now);
            let raise_needed =
                presenter.cursor_polling_tick(last_native_mouse_at, &mut pointer_present_synthetic);
            if raise_needed {
                window_pump.raise_hud(cur_generation)?;
            }
        }
        if !native_events.is_empty() {
            presenter.update_overlay_video_state(
                source.clock.now_secs(),
                f64::from_bits(source.duration_secs_bits.load(Ordering::Acquire)),
                source.clock.is_playing(),
                source.clock.volume(),
                source.clock.is_muted(),
                source.clock.limiter_ceiling_hit_seq(),
                source.clock.playback_speed(),
                source.frame_step_active.load(Ordering::Acquire),
                source.clock.is_seeking(),
                source.clock.current_seek_serial(),
            );
            let overlay_routing = match presenter.handle_window_events(&native_events) {
                Ok(outcome) => {
                    sync_hud_regions(&window_pump, cur_generation, &presenter, &outcome);
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
                            crate::video::native_presenter::NativeOverlayCommand::SeekRelative {
                                delta_secs,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SeekRelative { delta_secs },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::TouchChromeLearned => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::TouchChromeLearned,
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
                            crate::video::native_presenter::NativeOverlayCommand::NavigateItem {
                                delta,
                                via_wheel,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::NavigateItem { delta, via_wheel },
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
                                bar_width_points,
                                pixels_per_point,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::RequestSeekThumbnail {
                                        target_secs,
                                        bar_width_points,
                                        pixels_per_point,
                                    },
                                );
                            }
                            // T35: hover が外れた合図を player に伝える
                            crate::video::native_presenter::NativeOverlayCommand::ClearSeekThumbnail => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ClearSeekThumbnail,
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
                            crate::video::native_presenter::NativeOverlayCommand::ToggleSidePanelMode => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleSidePanelMode,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleClickInfoOpen => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleClickInfoOpen,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::OpenTouchInfoPanel => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::OpenTouchInfoPanel,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::DismissTouchSidePanels => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::DismissTouchSidePanels,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleVst3Gui => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleVst3Gui,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleAudioMode => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleAudioMode,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::CloseFullscreen => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::CloseFullscreen {
                                        generation: cur_generation,
                                    },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleWindowMode => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleWindowMode,
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
                            crate::video::native_presenter::NativeOverlayCommand::SetVst3PanelPos {
                                pos,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetVst3PanelPos { pos },
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
                            crate::video::native_presenter::NativeOverlayCommand::VideoAdjustLoadSlot {
                                slot_idx,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::VideoAdjustLoadSlot { slot_idx },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::VideoAdjustSaveSlot {
                                slot_idx,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::VideoAdjustSaveSlot { slot_idx },
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
                            crate::video::native_presenter::NativeOverlayCommand::ToggleContinuous => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleContinuous,
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
                            crate::video::native_presenter::NativeOverlayCommand::SetVideoAdjustments {
                                adjustments,
                                persist,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetVideoAdjustments {
                                        adjustments,
                                        persist,
                                    },
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
                            crate::video::native_presenter::NativeOverlayCommand::SetPinAt {
                                target_secs,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetPinAt { target_secs },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::JumpMarker {
                                next,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::JumpMarker { next },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SaveFrameToFile => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SaveFrameToFile,
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
                            crate::video::native_presenter::NativeOverlayCommand::DeletePin => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::DeletePin,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::BulkAddBookmarks {
                                entries,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::BulkAddBookmarks { entries },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ExportBookmarksToClipboard {
                                seconds_only,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ExportBookmarksToClipboard { seconds_only },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ClearAllBookmarksForCurrent => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ClearAllBookmarksForCurrent,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::OpenExternalUrl {
                                url,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::OpenExternalUrl { url },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::SetRating {
                                stars,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::SetRating { stars },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleTag {
                                name,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleTag { name },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::AddTag {
                                name,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::AddTag { name },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::RemoveTag {
                                name,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::RemoveTag { name },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::OpenTagViewForTag {
                                name,
                            } => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::OpenTagViewForTag { name },
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::ToggleNormalize => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::ToggleNormalize,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::DisableNormalize => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::DisableNormalize,
                                );
                            }
                            crate::video::native_presenter::NativeOverlayCommand::CancelNormalizeScan => {
                                send_native_output_event(
                                    &ui_event_tx,
                                    event_epoch,
                                    NativeVideoOutputEvent::CancelNormalizeScan,
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
            publish_native_overlay_input_routing(
                &ui_event_tx,
                source.source_epoch,
                &mut published_overlay_input_routing,
                overlay_routing,
            );
            for event in &native_events {
                if overlay_routing.should_forward_to_ui(event) {
                    send_native_output_event(
                        &ui_event_tx,
                        source.source_epoch,
                        NativeVideoOutputEvent::Window(event.clone()),
                    );
                }
            }
            last_overlay_tick = Instant::now();
        } else if !presenter_hidden
            && (perf_visibility_changed
                || presenter.overlay_needs_render()
                || presenter.overlay_repaint_due(Instant::now())
                || (source.clock.is_seeking()
                    && last_overlay_tick.elapsed() >= Duration::from_millis(50))
                || (presenter.overlay_wants_periodic_tick()
                    && last_overlay_tick.elapsed() >= Duration::from_millis(250)))
        {
            match presenter.tick_overlay_video_state(
                source.clock.now_secs(),
                f64::from_bits(source.duration_secs_bits.load(Ordering::Acquire)),
                source.clock.is_playing(),
                source.clock.volume(),
                source.clock.is_muted(),
                source.clock.limiter_ceiling_hit_seq(),
                source.clock.playback_speed(),
                source.frame_step_active.load(Ordering::Acquire),
                source.clock.is_seeking(),
                source.clock.current_seek_serial(),
            ) {
                Ok(outcome) => {
                    publish_native_overlay_input_routing(
                        &ui_event_tx,
                        source.source_epoch,
                        &mut published_overlay_input_routing,
                        outcome.routing,
                    );
                    sync_hud_regions(&window_pump, cur_generation, &presenter, &outcome);
                    for command in outcome.commands {
                        send_native_overlay_command(
                            &ui_event_tx,
                            source.source_epoch,
                            cur_generation,
                            command,
                        );
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
        let mut latest_hidden_frame: Option<VideoFrame> = None;
        if presenter_hidden {
            // A hidden presenter has no display pacing consumer. Applying the visible queue cap
            // here fills both source.queue and decoder video_tx, preventing the video decoder
            // from reaching demux EOF while audio and the wall clock keep advancing. Drain every
            // frame already available and reduce the batch to the newest valid frame; the typed
            // Hidden state remains the sole long-lived owner.
            drain_hidden_available_frames(
                &mut source.queue,
                &source.video_rx,
                &mut hidden_frame_scratch,
            );
            for frame in hidden_frame_scratch.drain(..) {
                if frame.seek_serial < clock_serial {
                    native_reset_unpresented_frame(frame);
                    continue;
                }
                if frame.seek_serial > source.last_seen_serial {
                    if let Some(previous) = latest_hidden_frame.take() {
                        native_reset_unpresented_frame(previous);
                    }
                    source.last_seen_serial = frame.seek_serial;
                    source.first_frame_event_last_epoch = None;
                    source.pending_first_frame_event = None;
                    source.last_present_source_pts = None;
                }
                if let Some(previous) = latest_hidden_frame.replace(frame) {
                    native_reset_unpresented_frame(previous);
                }
            }
        } else {
            // Visible playback remains display-paced. Stop draining when source.queue reaches
            // `MAX_NATIVE_SOURCE_QUEUE`: this bounds shared GPU-frame ownership and lets the
            // decoder's existing full-channel policy provide back-pressure.
            while source.queue.len() < MAX_NATIVE_SOURCE_QUEUE {
                let Ok(frame) = source.video_rx.try_recv() else {
                    break;
                };
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
        }
        if let Some(frame) = latest_hidden_frame {
            debug_assert!(presenter_hidden);
            debug_assert!(source.queue.is_empty());
            source.queue.push_back(frame);
        }

        // 音声のみ native シェル (Inc 6 ②-1): 待つべき映像 first frame が存在しないので
        // 常に false。動画経路は従来どおり first-frame event の到達で判定する。
        let waiting_for_first_frame = !config.audio_only
            && source.first_frame_event_last_epoch != Some(source.last_seen_serial);
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
                        "presenter_hidden",
                        serde_json::Value::from(presenter_hidden),
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
        if visible_video_loop_action(
            presenter_hidden,
            source.clock.is_playing(),
            source.clock.is_seeking(),
            waiting_for_first_frame,
        ) == VisibleVideoLoopAction::RefreshSettledResizeThenIdle
        {
            source.last_present_wall = None;
            source.last_present_source_pts = None;
            refresh_settled_resize_if_due(&mut presenter, &mut frame_output, config.sync_interval);
            // message 対応待機: 一時停止中でもリサイズ等のメッセージで即起床する。
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }

        let now = source.clock.now_secs();
        let candidates: Vec<frame_selection::FrameCandidate> = source
            .queue
            .iter()
            .map(|f| frame_selection::FrameCandidate {
                pts_secs: f.pts_secs,
                seek_serial: f.seek_serial,
            })
            .collect();
        let selection = if presenter_hidden {
            frame_selection::FrameSelection {
                actions: if candidates.is_empty() {
                    Vec::new()
                } else {
                    debug_assert_eq!(candidates.len(), 1);
                    vec![frame_selection::PopAction::Display]
                },
            }
        } else {
            frame_selection::select_frame_for_present(
                &candidates,
                now,
                source.clock.current_seek_serial(),
                source.last_seen_serial,
                waiting_for_first_frame,
                source.clock.is_seeking(),
                clock::DISPLAY_LEAD_TOLERANCE_SECS,
            )
        };
        drop(candidates);
        let mut latest_renderable: Option<VideoFrame> = None;
        let mut late_drop_delta = 0u32;
        for action in &selection.actions {
            let frame = source
                .queue
                .pop_front()
                .expect("frame_selection::select_frame_for_present promised this many frames");
            match action {
                frame_selection::PopAction::DiscardStale => {
                    native_reset_unpresented_frame(frame);
                }
                frame_selection::PopAction::LateDrop => {
                    let late_ms = ((now - frame.pts_secs) * 1000.0).max(0.0);
                    source.present_stats.record_late_drop(
                        frame.pts_secs,
                        late_ms,
                        source.queue.len(),
                    );
                    late_drop_delta = late_drop_delta.saturating_add(1);
                    native_reset_unpresented_frame(frame);
                }
                frame_selection::PopAction::Display => {
                    debug_assert!(
                        latest_renderable.is_none(),
                        "FrameSelection invariant: at most one Display per tick"
                    );
                    latest_renderable = Some(frame);
                }
            }
        }

        // A decoded frame naturally applies a settled resize through present().
        // Otherwise, re-present the one typed Visible frame exactly once.
        if latest_renderable.is_none() && !presenter_hidden {
            refresh_settled_resize_if_due(&mut presenter, &mut frame_output, config.sync_interval);
        }

        if let Some(frame) = latest_renderable {
            let pts = frame.pts_secs;
            let serial = frame.seek_serial;
            if presenter_hidden {
                // Inc 7 hidden presenter (consume-and-hold): present() は呼ばず、選択した
                // フレームを hold するだけ。ただし **present 成功時と同じ再生状態 bookkeeping**
                // (last_displayed_pts_bits / displayed_frame_seq / first_presented /
                // FirstFrameReady / seek override clear) は続ける。でないと音声モード中に
                // seek すると engine が映像 FirstFrameReady 待ちで Buffering 固着する。
                // ここは下の通常 present の Ok アームの該当部分と一致させること。
                source
                    .last_displayed_pts_bits
                    .store(pts.to_bits(), Ordering::Release);
                source.displayed_frame_seq.fetch_add(1, Ordering::Release);
                let first_hidden_present_for_source =
                    !first_presented_out.swap(true, Ordering::AcqRel);
                // Inc 7 (音声モード連続再生 EOF): source-swap 直後は navigation preview
                // (「プレビュー未保存 - 再生準備中...」+ 最小 HUD) が被さっている。通常 present の
                // Ok アームは初フレーム present 時にこれの clear を予約するが、hidden 中は present を
                // 通らないので clear されず、exit で presenter を表示した瞬間に stale な preview が
                // 見える (実機 FB 2026-07-04: ×ボタンだけ + 「再生準備中」)。hidden では window が
                // 隠れていて旧 source を一瞬晒す compositor pass の心配が無いので、新 source の初
                // hold フレームで即クリアする (通常 present 経路はバイト等価: この分岐に入らない)。
                if first_hidden_present_for_source {
                    pending_navigation_preview_clear_at = None;
                    presenter.set_overlay_navigation_preview(None);
                }
                let should_emit_first_frame_ready =
                    source.first_frame_event_last_epoch != Some(serial);
                if should_emit_first_frame_ready {
                    if try_send_native_first_frame_ready(&source.engine_event_tx, serial, pts) {
                        source.first_frame_event_last_epoch = Some(serial);
                        source.pending_first_frame_event = None;
                    } else {
                        source.pending_first_frame_event = Some((serial, pts));
                    }
                }
                let now_for_clear = source.clock.now_secs();
                let frame_step_active = source.frame_step_active.load(Ordering::Acquire);
                if source.clock.is_seeking()
                    && clock::pts_clears_seek_override(pts, now_for_clear)
                    && (frame_step_active || !source.clock.is_playing())
                {
                    source.clock.set_paused_position(pts);
                    source.clock.clear_seek_target_override(serial);
                } else if clock::pts_clears_seek_override(pts, now_for_clear)
                    && !frame_step_active
                    && !source.clock.is_audio_active()
                {
                    source.clock.set_fallback_anchor(pts);
                    source.clock.clear_seek_target_override(serial);
                }
                // 最新フレームを typed Hidden state が 1 枚だけ所有する。
                frame_output.hold_hidden(frame);
                continue;
            }
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

                    // ── A/V drift instrumentation ──
                    // 2 つのメトリクスを書く:
                    //   (1) av_drift_ms = video_pts − master_clock (= video pacing health。
                    //       通常 ≈ 0、video frame が clock に追従できていれば値は小さい)
                    //   (2) av_offset_ms = video_pts − audio_audible_pts (= **ユーザー体感の
                    //       音映像差**。Norm clear で audio が clock から乖離すると、こちらだけ
                    //       数秒級に飛ぶ。今回の調査で発見した経路を直接値で見るため)
                    // late_ms は max(0,...) で正方向に clamp 済みなので drift とは別物。
                    let now_for_drift = source.clock.now_secs();
                    let av_drift_ms = ((pts - now_for_drift) * 1000.0) as f32;
                    source
                        .audio_diagnostics
                        .av_drift_ms_bits
                        .store((av_drift_ms as f64).to_bits(), Ordering::Release);

                    // av_offset_ms: audio inactive (動画 only / 音声起動失敗) または
                    // seek / buffer clear 直後の offset 未確定時は None。
                    let av_offset_ms_opt = source
                        .audio_diagnostics
                        .load_audio_audible_pts()
                        .map(|aud| ((pts - aud) * 1000.0) as f32);
                    source.audio_diagnostics.av_offset_ms_bits.store(
                        match av_offset_ms_opt {
                            Some(v) => (v as f64).to_bits(),
                            None => f64::NAN.to_bits(),
                        },
                        Ordering::Release,
                    );
                    let audio_lead_ms = source.audio_diagnostics.load_audio_lead_ms();
                    let audio_active = source.clock.is_audio_active();

                    if crate::perf::is_enabled() {
                        let log_now = Instant::now();
                        // 「big」判定は av_offset (= 体感ズレ) を主とする。audio inactive
                        // なら旧 av_drift_ms にフォールバックするが、seek / buffer clear
                        // 直後の offset 未確定中は edge 判定を出さない。
                        let cur_big_value = match av_offset_ms_opt {
                            Some(v) => v.abs(),
                            None if !audio_active => av_drift_ms.abs(),
                            None => 0.0,
                        };
                        let big = cur_big_value > 30.0;
                        let big_edge = big && !source.last_av_drift_was_big;
                        source.last_av_drift_was_big = big;
                        let regular = log_now.duration_since(source.last_drift_log_at)
                            >= Duration::from_secs(1);
                        let big_emit_ok = big_edge
                            && log_now.duration_since(source.last_big_drift_emit_at)
                                >= Duration::from_millis(100);
                        if regular || big_emit_ok {
                            let av_offset_for_log = av_offset_ms_opt
                                .map(|v| serde_json::Value::from(v as f64))
                                .unwrap_or(serde_json::Value::Null);
                            crate::perf::event(
                                "video",
                                "av_drift",
                                None,
                                0,
                                &[
                                    ("video_pts", serde_json::Value::from(pts)),
                                    ("now_secs", serde_json::Value::from(now_for_drift)),
                                    ("drift_ms", serde_json::Value::from(av_drift_ms as f64)),
                                    ("av_offset_ms", av_offset_for_log),
                                    (
                                        "audio_lead_ms",
                                        serde_json::Value::from(audio_lead_ms as f64),
                                    ),
                                    ("audio_active", serde_json::Value::from(audio_active)),
                                    ("big_edge", serde_json::Value::from(big_edge)),
                                ],
                            );
                            if regular {
                                source.last_drift_log_at = log_now;
                            }
                            if big_emit_ok {
                                source.last_big_drift_emit_at = log_now;
                            }
                        }
                    }
                    let underrun_active = source.audio_diagnostics.load_underrun_active();

                    source
                        .present_stats
                        .record_present(&outcome, late_ms, total_ms, interval_ms);
                    let diag_view = crate::video::audio_diagnostics::OverlayDiagnostics {
                        av_drift_ms,
                        av_offset_ms: av_offset_ms_opt,
                        audio_active,
                        audio_lead_ms,
                        audio_underrun_active: underrun_active,
                    };
                    presenter.push_overlay_perf_sample(
                        crate::video::native_presenter::NativeOverlayPerfSample {
                            arrival: present_t0,
                            interval_ms: interval_ms as f32,
                            total_ms: total_ms as f32,
                            copy_ms: outcome.copy_ms as f32,
                            present_waitable_ms: outcome.present_waitable_ms as f32,
                            present_call_ms: outcome.present_call_ms as f32,
                            late_ms: late_ms as f32,
                            late_drop_delta,
                            source_delta_ms: source_delta_ms as f32,
                            playback_speed: source.clock.playback_speed() as f32,
                            av_drift_ms,
                            av_offset_ms: av_offset_ms_opt.unwrap_or(f32::NAN),
                            audio_active,
                            audio_lead_ms,
                            audio_underrun_active: underrun_active,
                        },
                        source
                            .present_stats
                            .overlay_snapshot(run_started.elapsed(), diag_view),
                    );
                    if last_summary_log.elapsed() >= Duration::from_secs(1) {
                        source.present_stats.emit_summary(run_started.elapsed());
                        last_summary_log = Instant::now();
                    }
                    source.displayed_frame_seq.fetch_add(1, Ordering::Release);
                    let first_present_for_source =
                        !first_presented_out.swap(true, Ordering::AcqRel);
                    // Keep the deferred-navigation preview covering the old source until
                    // the first frame from the new source has actually reached the
                    // presenter and has had a short compositor window to latch.
                    // Clearing it at SwitchSource time or immediately after Present can
                    // expose the previous source for one compositor pass.
                    if first_present_for_source {
                        pending_navigation_preview_clear_at =
                            Some(Instant::now() + NAVIGATION_PREVIEW_CLEAR_DELAY);
                    }
                    if !first_present_probe_logged {
                        first_present_probe_logged = true;
                        crate::logger::log(format!(
                            "[native-video] first present probe: pts={:.3} serial={} elapsed_ms={:.1}",
                            pts,
                            serial,
                            present_t0.duration_since(run_started).as_secs_f64() * 1000.0
                        ));
                    }
                    let should_emit_first_frame_ready =
                        source.first_frame_event_last_epoch != Some(serial);
                    if should_emit_first_frame_ready {
                        if try_send_native_first_frame_ready(&source.engine_event_tx, serial, pts) {
                            source.first_frame_event_last_epoch = Some(serial);
                            source.pending_first_frame_event = None;
                        } else {
                            source.pending_first_frame_event = Some((serial, pts));
                        }
                    }
                    let now_for_clear = source.clock.now_secs();
                    let frame_step_active = source.frame_step_active.load(Ordering::Acquire);
                    if source.clock.is_seeking()
                        && clock::pts_clears_seek_override(pts, now_for_clear)
                        && (frame_step_active || !source.clock.is_playing())
                    {
                        source.clock.set_paused_position(pts);
                        source.clock.clear_seek_target_override(serial);
                    } else if clock::pts_clears_seek_override(pts, now_for_clear)
                        && !frame_step_active
                        && !source.clock.is_audio_active()
                    {
                        source.clock.set_fallback_anchor(pts);
                        source.clock.clear_seek_target_override(serial);
                    }
                    if crate::perf::is_enabled() {
                        // geometry 変更 (= swap chain 差し替え) の present は毎回出す。
                        // 左上ずれ / 別フレーム混入はこのタイミングで起きるため、推測でなく
                        // ログで切り分けられるようにする (Codex 助言)。
                        if trace_every_present
                            || outcome.geometry_changed
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
                                        "source_epoch",
                                        serde_json::Value::from(source.source_epoch as i64),
                                    ),
                                    ("seek_serial", serde_json::Value::from(serial as i64)),
                                    ("frame_width", serde_json::Value::from(frame.width as i64)),
                                    ("frame_height", serde_json::Value::from(frame.height as i64)),
                                    (
                                        "surface_width",
                                        serde_json::Value::from(outcome.surface_width as i64),
                                    ),
                                    (
                                        "surface_height",
                                        serde_json::Value::from(outcome.surface_height as i64),
                                    ),
                                    (
                                        "shared_texture_gen",
                                        serde_json::Value::from(outcome.shared_texture_gen),
                                    ),
                                    ("fence_value", serde_json::Value::from(outcome.fence_value)),
                                    (
                                        "geometry_changed",
                                        serde_json::Value::from(outcome.geometry_changed),
                                    ),
                                    (
                                        "surface_swapped",
                                        serde_json::Value::from(outcome.surface_swapped),
                                    ),
                                    (
                                        "commit_sync_ms",
                                        serde_json::Value::from(outcome.commit_sync_ms),
                                    ),
                                    (
                                        "retire_queue_len",
                                        serde_json::Value::from(frame_output.retire_len() as i64),
                                    ),
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
                    // The displayed frame becomes the sole reusable Visible frame.
                    // Its predecessor moves to the disposal-only retire queue.
                    frame_output.commit_presented(frame, outcome.copy_fence_value);
                    frame_output.retire_completed(presenter.copy_fence_completed_value());
                }
                Err(err) => {
                    crate::logger::log(format!("[native-video] present failed: {err}"));
                    native_reset_unpresented_frame(frame);
                    std::thread::sleep(Duration::from_millis(16));
                }
            }
        } else {
            let wait_ms = if config.audio_only {
                // 音声のみ native シェル (Inc 6 ②-1): 映像フレームが永久に来ないので、
                // frame pacing 用の 1ms スピンではなく HUD periodic tick (250ms) に十分な
                // 16ms で休む。HWND message pump は独立 owner thread が継続するため、
                // render のこの待機が mouse/resize dispatch を止めることはない。
                16u64
            } else {
                let speed = source.clock.playback_speed().max(clock::MIN_PLAYBACK_SPEED);
                source
                    .queue
                    .front()
                    .map(|front| (((front.pts_secs - now) / speed) * 500.0).clamp(1.0, 8.0) as u64)
                    .unwrap_or(1)
            };
            // message 対応待機: フレーム待ちのアイドル中でもリサイズ等のメッセージで
            // 即起床し、presenter ループが素早く WM を処理できるようにする。
            std::thread::sleep(Duration::from_millis(wait_ms));
        }
    }

    frame_output.clear_current();
    frame_output.retire_completed(presenter.copy_fence_completed_value());
    emit_native_vram_trace(
        "native_presenter_exit_begin",
        "before_exit_drain",
        &source,
        &presenter,
        frame_output.retire_len(),
    );
    native_drain_unpresented_queue(&mut source.queue);
    emit_native_vram_trace(
        "native_presenter_exit_after_drain",
        "after_exit_drain",
        &source,
        &presenter,
        frame_output.retire_len(),
    );
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
    emit_native_vram_trace(
        "native_presenter_before_destroy",
        "before_render_drop",
        &source,
        &presenter,
        frame_output.retire_len(),
    );
    window_pump.shutdown(cur_window_request.saturating_add(1));
    presenter.detach();
    crate::logger::log("[native-video] fullscreen presenter stopped".to_string());
    Ok(())
}

/// `future_frames` キューの最大長。decoder の `video_tx` (= 24) と揃える。
/// 1080p RGBA で 24 × ~8MB = 192MB 程度 (CPU 経路の上限)。GPU 経路では
/// 1 frame ≈ HANDLE+メタのみで実コストは無視できる。decoder の burst-stall
/// パターン (~400ms) + HDD random read (~100-300ms) を ~800ms buffer で
/// 吸収して UI tick の空振りを抑える (Phase 8.J)。
pub(crate) const MAX_RENDER_QUEUE: usize = 24;
const FRAME_STEP_NO_PENDING_SEQ: u64 = u64::MAX;
const USER_SEEK_REISSUE_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug, Default)]
struct UserSeekCoalesceState {
    pending_target_secs: Option<f64>,
    last_issued_at: Option<std::time::Instant>,
    last_issued_display_seq: u64,
}

fn user_seek_ready_to_issue(
    state: &UserSeekCoalesceState,
    is_seeking: bool,
    displayed_seq: u64,
    now: std::time::Instant,
) -> bool {
    !is_seeking
        || displayed_seq > state.last_issued_display_seq
        || state
            .last_issued_at
            .is_some_and(|issued_at| now.duration_since(issued_at) >= USER_SEEK_REISSUE_AFTER)
}

fn frame_step_base_secs(
    pending_step_base: Option<f64>,
    last_displayed_pts: Option<f64>,
    current_position: f64,
) -> f64 {
    pending_step_base
        .or(last_displayed_pts)
        .unwrap_or(current_position)
        .max(0.0)
}

fn frame_step_interval_secs(avg_fps: f64) -> f64 {
    if avg_fps.is_finite() && avg_fps > 1.0 {
        (1.0 / avg_fps).clamp(1.0 / 1000.0, 1.0)
    } else {
        1.0 / 30.0
    }
}

fn frame_step_seek_start_secs(base: f64, avg_fps: f64, direction: i32) -> f64 {
    let frame_interval = frame_step_interval_secs(avg_fps);
    let scan_back_secs = if direction < 0 {
        (frame_interval * 1.25).clamp(0.002, 0.25)
    } else {
        (frame_interval * 8.0).clamp(0.050, 1.0)
    };
    (base - scan_back_secs).max(0.0)
}

fn frame_step_waiting_for_display(
    pending_step_base: Option<f64>,
    issued_display_seq: u64,
    current_display_seq: u64,
) -> bool {
    pending_step_base.is_some()
        && issued_display_seq != FRAME_STEP_NO_PENDING_SEQ
        && issued_display_seq == current_display_seq
}

fn eof_freeze_position(known_duration_secs: Option<f64>, current_secs: f64) -> f64 {
    known_duration_secs
        .filter(|duration| *duration > 0.0)
        .unwrap_or_else(|| current_secs.max(0.0))
}

impl VideoPlayer {
    /// ParkedLive teardown の resume 保存テスト用。worker / native HWND を起動しない。
    #[cfg(test)]
    pub(crate) fn disconnected_for_test(path: PathBuf, position_secs: f64) -> Self {
        let seek_serial = Arc::new(AtomicU64::new(0));
        let clock = Arc::new(AvClock::new(1.0, seek_serial.clone()));
        clock.set_paused_position(position_secs);
        let engine = Arc::new(Mutex::new(EngineActor::new(
            OpenOptions {
                initial_volume: 1.0,
                autoplay: false,
                ..Default::default()
            },
            seek_serial,
            clock.clone(),
        )));
        let engine_state_atomic = engine.lock().unwrap().published_state_handle();
        let (engine_event_tx, engine_event_rx) = crossbeam_channel::bounded(8);
        Self {
            path,
            clock,
            engine,
            engine_state_atomic,
            engine_event_tx,
            engine_event_rx,
            info_event_emitted: false,
            first_frame_event_last_epoch: None,
            displayed_frame_seq: Arc::new(AtomicU64::new(0)),
            last_displayed_pts_bits: Arc::new(AtomicU64::new(position_secs.to_bits())),
            frame_step_base_bits: AtomicU64::new(f64::NAN.to_bits()),
            frame_step_active: Arc::new(AtomicBool::new(false)),
            frame_step_issued_display_seq: AtomicU64::new(FRAME_STEP_NO_PENDING_SEQ),
            #[cfg(windows)]
            duration_secs_bits: Arc::new(AtomicU64::new(0.0_f64.to_bits())),
            decoder_dropped_full_count: Arc::new(AtomicU64::new(0)),
            ui_dropped_past_count: AtomicU64::new(0),
            cancel: Arc::new(AtomicBool::new(true)),
            decode: dummy_decode_handles(),
            video_output: VideoOutputState::Inactive,
            audio: None,
            info: None,
            error: None,
            thumb_worker: None,
            remote_seek_thumbnail_request: Mutex::new(None),
            future_frames: std::collections::VecDeque::new(),
            pending_resume_secs: None,
            last_seen_seek_serial: 0,
            loop_enabled: AtomicBool::new(false),
            loop_target_bits: AtomicU64::new(0),
            eof_loop_quiet_ticks: AtomicU32::new(0),
            seek_inflight_since: None,
            seek_eof_stuck_since: None,
            user_seek_coalesce: Mutex::new(UserSeekCoalesceState::default()),
            #[cfg(windows)]
            gpu_latest: None,
            #[cfg(windows)]
            native_output: None,
            #[cfg(windows)]
            native_hover_thumbnail_request: Mutex::new(None),
            #[cfg(windows)]
            native_hover_thumbnail_sent_key: Mutex::new(None),
            #[cfg(windows)]
            dynamic: Arc::new(crate::video::decoder::VideoDynamicState::default()),
            audio_diagnostics: Arc::new(crate::video::audio_diagnostics::AudioDiagnostics::new(
                std::time::Instant::now(),
            )),
        }
    }

    #[cfg(test)]
    pub(crate) fn stream_ready_disconnected_for_test(path: PathBuf) -> Self {
        let mut player = Self::disconnected_for_test(path, 0.0);
        player.decode.video_tap =
            crate::video::stream::video_tap::VideoTapController::connected_without_frames_for_test(
            );
        player.audio =
            Some(crate::video::audio::AudioOutput::connected_without_output_for_test(48_000));
        #[cfg(windows)]
        {
            player.native_output = Some(NativeVideoOutput::disconnected_for_test());
        }
        player.info = Some(VideoInfo {
            width: 640,
            height: 360,
            duration_secs: 30.0,
            video_codec: "h264".to_owned(),
            video_decoder: "test".to_owned(),
            d3d11va_supported: false,
            d3d11va_config: String::new(),
            orientation: crate::video::display_metadata::VideoOrientation::IDENTITY,
            audio_codec: Some("aac".to_owned()),
            audio_bit_rate_bps: 128_000,
            has_audio: true,
            has_video: true,
            hw_decode_active: false,
            gpu_path_active: false,
            effective_deinterlace_mode: crate::settings::VideoDeinterlaceMode::Off,
            dynamic: Arc::new(crate::video::decoder::VideoDynamicState::default()),
            title: None,
            artist: None,
            original_url: None,
            description: None,
            avg_fps: 30.0,
            fps_num: 30,
            fps_den: 1,
            bit_rate_bps: 1_000_000,
            chapters: Vec::new(),
            sar_num: 1,
            sar_den: 1,
        });
        player
    }

    #[cfg(test)]
    pub(crate) fn set_last_displayed_pts_for_test(&self, position_secs: f64) {
        self.last_displayed_pts_bits
            .store(position_secs.to_bits(), Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn mark_eof_for_test(&self, duration_secs: f64) {
        let mut engine = self.engine.lock().unwrap();
        let epoch = engine.current_seek_epoch();
        engine.handle_decoder_event(engine::state::DecoderEvent::EofReached {
            epoch,
            duration_secs,
        });
    }

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
    /// `initial_volume` は線形ゲイン (0.0..+18dB 相当)。1.0 超は音声ポンプ側の
    /// 手動 boost として扱う。
    /// `initial_normalize_gain` は線形ゲイン (1.0 = 素通し)。open 前に DB hit が
    /// 分かっている場合、音声ワーカー起動前に設定して最初の chunk から反映する。
    /// `initial_audio_preroll_suspended` が true の間は、測定前 Norm などのために
    /// audio-pump の raw→processed 先読みを一時停止する。
    /// `resume_secs` を指定すると、最初の動画情報受領後に自動的にその位置へシークする。
    /// `hw_decode` が true なら D3D11VA HW デコードを試行する。D3D11VA 非対応 codec は
    /// SW で開き、D3D11VA 対応 codec の HW 初期化 / open 失敗はエラーにする。
    /// VST3 プラグイン処理用の DspBridge は `dsp_bridge` 引数で渡す。
    /// `None` または `is_enabled()=false` なら audio-pump はパススルー。
    /// `is_enabled()=true` のときは pump thread で `bridge.process_block` を呼ぶ。
    pub fn open(
        path: PathBuf,
        initial_volume: f64,
        initial_normalize_gain: f64,
        initial_audio_preroll_suspended: bool,
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
        Self::open_with_output_consumer(
            path,
            initial_volume,
            initial_normalize_gain,
            initial_audio_preroll_suspended,
            autoplay,
            resume_secs,
            hw_decode,
            deinterlace,
            #[cfg(windows)]
            gpu_video_device,
            #[cfg(windows)]
            dsp_bridge,
            VideoOutputConsumer::Presentation,
            #[cfg(windows)]
            native_output_config,
        )
    }

    pub(crate) fn open_with_output_consumer(
        path: PathBuf,
        initial_volume: f64,
        initial_normalize_gain: f64,
        initial_audio_preroll_suspended: bool,
        autoplay: bool,
        resume_secs: Option<f64>,
        hw_decode: bool,
        deinterlace: crate::settings::VideoDeinterlaceMode,
        #[cfg(windows)] gpu_video_device: Option<
            std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>,
        >,
        #[cfg(windows)] dsp_bridge: Option<std::sync::Arc<crate::video::dsp::DspBridge>>,
        output_consumer: VideoOutputConsumer,
        #[cfg(windows)] native_output_config: Option<NativeVideoOutputConfig>,
    ) -> Self {
        #[cfg(windows)]
        let native_output_config = match output_consumer {
            VideoOutputConsumer::Presentation => native_output_config,
            VideoOutputConsumer::RemoteHeadless => None,
        };
        // FFmpeg DLL ロード (1 回目のみ実時間の I/O。以降は OnceLock で即返り)
        if let Err(e) = ffmpeg_loader::init() {
            // open 失敗時の dummy engine (Idle のまま)。実 decoder は起きないので、
            // begin_loading は呼ばない (= Phase 3+ で resume 適用も走らない)。
            // 共有 seek_serial を 1 個作り、AvClock と EngineActor 双方に clone を渡す。
            let seek_serial = Arc::new(AtomicU64::new(0));
            let dummy_clock = Arc::new(AvClock::new(initial_volume, seek_serial.clone()));
            dummy_clock.set_normalize_gain(initial_normalize_gain);
            dummy_clock.set_audio_preroll_suspended(initial_audio_preroll_suspended);
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
                frame_step_base_bits: AtomicU64::new(f64::NAN.to_bits()),
                frame_step_active: Arc::new(AtomicBool::new(false)),
                frame_step_issued_display_seq: AtomicU64::new(FRAME_STEP_NO_PENDING_SEQ),
                decoder_dropped_full_count: Arc::new(AtomicU64::new(0)),
                ui_dropped_past_count: AtomicU64::new(0),
                cancel: Arc::new(AtomicBool::new(true)),
                decode: dummy_decode_handles(),
                video_output: VideoOutputState::Inactive,
                audio: None,
                info: None,
                error: Some(format!("FFmpeg DLL のロードに失敗しました: {e}")),
                thumb_worker: None,
                remote_seek_thumbnail_request: Mutex::new(None),
                future_frames: std::collections::VecDeque::new(),
                pending_resume_secs: None,
                last_seen_seek_serial: 0,
                loop_enabled: AtomicBool::new(false),
                loop_target_bits: AtomicU64::new(0u64), // = (0.0_f64).to_bits()
                eof_loop_quiet_ticks: AtomicU32::new(0),
                seek_inflight_since: None,
                seek_eof_stuck_since: None,
                user_seek_coalesce: Mutex::new(UserSeekCoalesceState::default()),
                #[cfg(windows)]
                gpu_latest: None,
                #[cfg(windows)]
                native_output: None,
                #[cfg(windows)]
                duration_secs_bits: Arc::new(AtomicU64::new(0.0_f64.to_bits())),
                #[cfg(windows)]
                native_hover_thumbnail_request: Mutex::new(None),
                #[cfg(windows)]
                native_hover_thumbnail_sent_key: Mutex::new(None),
                #[cfg(windows)]
                dynamic: Arc::new(crate::video::decoder::VideoDynamicState::default()),
                audio_diagnostics: Arc::new(
                    crate::video::audio_diagnostics::AudioDiagnostics::new(
                        std::time::Instant::now(),
                    ),
                ),
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
        // DB hit 済みの Norm gain は audio pump 起動前に入れる。open / source-swap 直後の
        // 最初の processed chunk から反映されるので、旧 gain の一瞬の鳴りを避けられる。
        clock.set_normalize_gain(initial_normalize_gain);
        clock.set_audio_preroll_suspended(initial_audio_preroll_suspended);
        let cancel = Arc::new(AtomicBool::new(false));

        // A/V sync drift デバッグ用 atomic bundle。audio.rs (cpal callback / pump) と
        // native presenter (present 経路 + overlay 描画) 両方が同じ Arc を共有する。
        // VideoPlayer 起動時刻を `wall_ns_now()` の基準として記録。
        let audio_diagnostics = Arc::new(crate::video::audio_diagnostics::AudioDiagnostics::new(
            std::time::Instant::now(),
        ));

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

        // decoder thread / native presenter thread / UI で per-frame の動的状態を
        // 共有するための atomic 群。VideoInfo にも同じ Arc を載せて UI が読む。
        let dynamic = Arc::new(crate::video::decoder::VideoDynamicState::default());

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
            Arc::clone(&dynamic),
        );

        // 音声出力起動。失敗してもプレイヤーは生きる (映像のみ再生)。
        // 音声を二重に消費するので、decoder の audio_rx を audio.start に渡す。
        // ここで decode.audio_rx を取り出す必要があるので構造体を分解する。
        let DecodeHandles {
            video_rx,
            audio_rx,
            info_rx,
            prep_progress,
            video_tap,
        } = decode;
        #[cfg(windows)]
        let native_video_rx = video_rx.clone();
        let audio = match audio::start(
            audio_rx,
            clock.clone(),
            engine_event_tx.clone(),
            engine_state_handle.clone(),
            Arc::clone(&audio_diagnostics),
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

        // 2026-05 root fix: AvClock の playing フラグはここでは触らない。
        // 旧コードは `clock.set_playing(autoplay)` で AvClock の wall extrapolation を
        // 即起動していたが、`EngineActor` 側が `Loading/Buffering = Frozen` の状態機械を
        // 持っている設計と二重管理になっており、presenter / decoder の参照する
        // `AvClock::now_secs()` だけが先行して進み、Playing 遷移までの ~300ms に
        // - presenter 側: 冒頭 frame に対する不当な late_drop
        // - decoder 側: queue full → 冒頭 14 frame の `dropped_full` 連発
        // を引き起こしていた (= 「動画再生開始直後の一瞬カクつき」)。
        //
        // 現在: AvClock は Frozen(0)/playing=false のまま据え置き、`EngineActor::begin_loading`
        // → 各 `transition_to_*` が `engine_freeze_at` / `engine_start_playing` で AvClock を
        // 同期する。autoplay 意図は `EngineActor::opts.autoplay` が単独で保持し、
        // `try_transition_from_buffering` の最後で Playing / Paused を選ぶ。
        let _ = autoplay; // intent は EngineActor 経由で配線済 (= OpenOptions.autoplay)

        // シーク先サムネ抽出ワーカー (失敗してもメイン再生は続行)
        let thumb_worker = Some(ThumbnailWorker::spawn(path.clone(), hw_decode));
        let displayed_frame_seq = Arc::new(AtomicU64::new(0));
        let last_displayed_pts_bits = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
        let frame_step_active = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        let duration_secs_bits = Arc::new(AtomicU64::new(0.0_f64.to_bits()));
        let VideoOutputRoute {
            player_rx: video_rx,
            state: video_output,
            init_error: headless_init_error,
        } = route_video_output(
            output_consumer,
            video_rx,
            Arc::clone(&cancel),
            Arc::clone(&clock),
            engine_event_tx.clone(),
            Arc::clone(&displayed_frame_seq),
            Arc::clone(&last_displayed_pts_bits),
        );
        // Presentation 出力は native presenter (独立 HWND + D3D11 swap chain) を必須とする。
        // RemoteHeadless は上の専用 consumer が video_rx を所有し、native output は作らない。
        // - `native_output_config = None`: 呼び出し元が後から `attach_native_output`
        //   で output を渡す fast-swap 経路のシグナル (= ここではエラー扱いしない)。
        //   実際にモニター情報取得失敗で None になった場合は、呼び出し元
        //   (`start_fs_load`) が `fail_native_init` で error を立てる責務を持つ。
        // - `spawn` 失敗: presenter スレッド生成エラー。UI に表示するフレームの
        //   実体経路がないので、その時点で player の `error` フィールドにメッセージを
        //   入れる。UI は赤字エラーで「読込失敗」を表示し、displayed_frame_seq=0 の
        //   ままなので "動画を準備中..." の無限ループに陥らない。
        #[cfg(windows)]
        let (native_output, native_init_error): (
            Option<NativeVideoOutput>,
            Option<String>,
        ) = {
            match native_output_config {
                // None は「呼び出し元が attach_native_output で後から output を渡す」
                // ことを示すシグナル (fast-swap 経路)。実際にモニター情報取得失敗で
                // None になったケースは、呼び出し元 (start_fs_load) が
                // `fail_native_init` で error をセットする責務を持つ。
                None => (None, None),
                Some(config) => match NativeVideoOutput::spawn(
                    native_video_rx,
                    Arc::clone(&clock),
                    engine_event_tx.clone(),
                    Arc::clone(&displayed_frame_seq),
                    Arc::clone(&last_displayed_pts_bits),
                    Arc::clone(&frame_step_active),
                    Arc::clone(&duration_secs_bits),
                    config,
                    Arc::clone(&dynamic),
                    Arc::clone(&audio_diagnostics),
                ) {
                    Some(output) => (Some(output), None),
                    None => (
                        None,
                        Some(
                            "ネイティブ動画プレゼンターの起動に失敗しました (スレッド生成エラー)"
                                .to_string(),
                        ),
                    ),
                },
            }
        };

        #[cfg(not(windows))]
        let native_init_error: Option<String> = None;

        let mut player = Self {
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
            frame_step_base_bits: AtomicU64::new(f64::NAN.to_bits()),
            frame_step_active,
            frame_step_issued_display_seq: AtomicU64::new(FRAME_STEP_NO_PENDING_SEQ),
            #[cfg(windows)]
            duration_secs_bits,
            decoder_dropped_full_count,
            ui_dropped_past_count: AtomicU64::new(0),
            cancel,
            decode: DecodeHandles {
                video_rx,
                audio_rx: dummy_audio_rx(),
                info_rx,
                prep_progress,
                video_tap,
            },
            video_output,
            audio,
            info: None,
            error: headless_init_error.or(native_init_error),
            thumb_worker,
            remote_seek_thumbnail_request: Mutex::new(None),
            future_frames: std::collections::VecDeque::new(),
            pending_resume_secs: resume_secs,
            last_seen_seek_serial: 0,
            loop_enabled: AtomicBool::new(false),
            loop_target_bits: AtomicU64::new(0u64),
            eof_loop_quiet_ticks: AtomicU32::new(0),
            seek_inflight_since: None,
            seek_eof_stuck_since: None,
            user_seek_coalesce: Mutex::new(UserSeekCoalesceState::default()),
            #[cfg(windows)]
            gpu_latest: None,
            #[cfg(windows)]
            native_output,
            #[cfg(windows)]
            native_hover_thumbnail_request: Mutex::new(None),
            #[cfg(windows)]
            native_hover_thumbnail_sent_key: Mutex::new(None),
            #[cfg(windows)]
            dynamic,
            audio_diagnostics,
        };
        // spawn 失敗で error が立った場合、すでに走っている decoder / audio /
        // thumbnail worker を停止する。`tick()` は `error.is_some()` で早期 return
        // するので、放置すると裏で再生パイプラインだけが回り続ける。
        // (config=None ケースは呼び出し元が `fail_native_init` 経由で同等の処理を行う)
        if player.error.is_some() {
            player.shutdown_workers_for_error();
        }
        crate::logger::log(format!(
            "[video-debug] VideoPlayer::open done path={} autoplay={} volume={:.2} normalize_gain={:.3} audio_preroll_suspended={} engine_state={} resume_secs={:?} video_rx_len={} audio_rx_len={}",
            player.path.display(),
            autoplay,
            initial_volume,
            player.normalize_gain(),
            player.audio_preroll_suspended(),
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
            let readiness_event = match &ev {
                EngineEvent::Decoder(engine::state::DecoderEvent::SeekCompleted {
                    epoch,
                    actual_pts,
                }) => Some(("SeekCompleted", *epoch, *actual_pts)),
                EngineEvent::Decoder(engine::state::DecoderEvent::FirstFrameReady {
                    epoch,
                    pts,
                }) => Some(("FirstFrameReady", *epoch, *pts)),
                EngineEvent::Audio(engine::state::AudioEvent::BufferReady {
                    epoch, pts, ..
                }) => Some(("BufferReady", *epoch, *pts)),
                _ => None,
            };
            let before = readiness_event.map(|_| engine.readiness_snapshot());
            match ev {
                EngineEvent::Decoder(d) => engine.handle_decoder_event(d),
                EngineEvent::Audio(a) => engine.handle_audio_event(a),
            }
            if let (Some((event, epoch, pts)), Some(before)) = (readiness_event, before) {
                let after = engine.readiness_snapshot();
                if event != "BufferReady" || !before.audio_ready || before.state != after.state {
                    crate::logger::log(format!(
                        "[video-engine] readiness event received: event={event} event_epoch={epoch} pts={pts:.3} state={}->{} video_required={} video_ready={} audio_required={} audio_ready={}",
                        before.state,
                        after.state,
                        after.video_required,
                        after.video_ready,
                        after.audio_required,
                        after.audio_ready
                    ));
                }
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

    /// 指定位置のサムネイルを要求する。許容秒数は利用者ごとに必ず渡す。
    pub fn request_seek_thumbnail(&self, target_secs: f64, tolerance_secs: f64) {
        if !target_secs.is_finite()
            || target_secs < 0.0
            || !tolerance_secs.is_finite()
            || tolerance_secs < 0.0
        {
            return;
        }
        if let Some(w) = &self.thumb_worker {
            let avg_fps = self.info.as_ref().map(|info| info.avg_fps).unwrap_or(0.0);
            let lookup_tolerance_secs =
                crate::video::thumbnail::cache_lookup_tolerance(tolerance_secs, avg_fps);
            w.request(target_secs, tolerance_secs, lookup_tolerance_secs);
        }
    }

    /// Native jump panel の marker サムネ warmup 枠を 1 件消費する。
    /// hover preview と worker を共有しているため、hover が **まだ満たされていない間** は
    /// marker 側から割り込まない (= 現在見ようとしているプレビューを最優先)。hover thumb
    /// がすでに worker キャッシュに入っていれば worker は idle なので、marker warmup を
    /// 進めてよい。worker 自身が同一要求を一度だけ処理する。
    /// 戻り値 true は「この marker を処理対象にしたので、このフレームでは後続 marker を
    /// 要求しない」という意味。hover 未満足の場合も true を返す。
    pub fn request_marker_thumbnail_warmup(&self, target_secs: f64) -> bool {
        if !target_secs.is_finite() || target_secs < 0.0 {
            return false;
        }
        if self.thumb_worker.is_none() {
            return true;
        }
        // hover が **未満足のとき** だけ marker を抑制する。
        // 過去には hover target が Some であるだけで suppress していたが、
        // `clear_native_hover_thumbnail` が届かない経路 (e.g., cursor が seek bar から
        // hover サムネ自体に乗ると `seek_resp.hovered()` が外れても target_secs は固定
        // — overlay_draw.rs の挙動) で sticky になり、新規 bookmark のサムネが
        // 動画再 open まで永久に warmup されない問題があった (2026-05-16 報告)。
        #[cfg(windows)]
        if let Some(hover_request) = self
            .native_hover_thumbnail_request
            .lock()
            .ok()
            .and_then(|request| *request)
            && self
                .nearest_seek_thumbnail(hover_request.target_secs, hover_request.tolerance_secs)
                .is_none()
        {
            return true;
        }
        if let Some(remote_request) = self
            .remote_seek_thumbnail_request
            .lock()
            .ok()
            .and_then(|request| *request)
            && self
                .nearest_seek_thumbnail(remote_request.target_secs, remote_request.tolerance_secs)
                .is_none()
        {
            return true;
        }
        self.request_seek_thumbnail(target_secs, 0.0);
        true
    }

    #[cfg(windows)]
    pub fn request_native_hover_thumbnail(&self, target_secs: f64, tolerance_secs: f64) {
        let target_secs = if target_secs.is_finite() {
            target_secs.max(0.0)
        } else {
            return;
        };
        if !tolerance_secs.is_finite() || tolerance_secs < 0.0 {
            return;
        }
        // T35 (Codex R-VTT-005): hover request は seek bar の右端で
        // `duration * frac` を渡すことが多く、container duration ぴったりだと
        // 最終 video frame の PTS を超えてサムネ抽出が EOF まで走る。
        // `clamp_seek_target` で同じ `duration - 0.1` クランプを適用してから渡す
        // (= 再生 seek と同じ可達領域に正規化)。
        let target_secs = self.clamp_seek_target(target_secs);
        self.request_seek_thumbnail(target_secs, tolerance_secs);
        if let Ok(mut request) = self.native_hover_thumbnail_request.lock() {
            let next = SeekThumbnailRequest {
                target_secs,
                tolerance_secs,
            };
            if *request != Some(next)
                && let Ok(mut sent) = self.native_hover_thumbnail_sent_key.lock()
            {
                *sent = None;
            }
            *request = Some(next);
        }
    }

    /// HUD hover が外れたときに hover thumbnail 要求を明示的にクリアする (T35)。
    /// クリアしないと `pump_native_hover_thumbnail` が「最後の target_secs」を保持し、
    /// 後続フレームでサムネ抽出を再要求し続けてしまう (= 永久リトライ)。
    #[cfg(windows)]
    pub fn clear_native_hover_thumbnail(&self) {
        if let Ok(mut request) = self.native_hover_thumbnail_request.lock() {
            *request = None;
        }
        if let Ok(mut sent) = self.native_hover_thumbnail_sent_key.lock() {
            *sent = None;
        }
        if let Some(output) = self.native_output.as_ref() {
            output.set_hover_thumbnail(None);
        }
    }

    /// 実時刻キャッシュを要求ごとの許容範囲で検索する。
    pub fn nearest_seek_thumbnail(
        &self,
        target_secs: f64,
        tolerance_secs: f64,
    ) -> Option<Thumbnail> {
        let avg_fps = self.info.as_ref().map(|info| info.avg_fps).unwrap_or(0.0);
        let lookup_tolerance_secs =
            crate::video::thumbnail::cache_lookup_tolerance(tolerance_secs, avg_fps);
        self.thumb_worker
            .as_ref()
            .and_then(|worker| worker.nearest(target_secs, tolerance_secs, lookup_tolerance_secs))
    }

    /// Remote seek/drag preview uses the same independent latest-wins worker as native hover.
    /// This only schedules auxiliary decode work; stream readiness never waits for its result.
    pub fn request_remote_seek_thumbnail(
        &self,
        target_secs: f64,
        tolerance_secs: f64,
    ) -> Option<f64> {
        if !target_secs.is_finite()
            || target_secs < 0.0
            || !tolerance_secs.is_finite()
            || tolerance_secs < 0.0
        {
            return None;
        }
        let target_secs = self.clamp_seek_target(target_secs);
        if let Ok(mut request) = self.remote_seek_thumbnail_request.lock() {
            *request = Some(SeekThumbnailRequest {
                target_secs,
                tolerance_secs,
            });
        }
        self.request_seek_thumbnail(target_secs, tolerance_secs);
        Some(target_secs)
    }

    pub fn clear_remote_seek_thumbnail(&self) {
        if let Ok(mut request) = self.remote_seek_thumbnail_request.lock() {
            *request = None;
        }
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

    /// 動画オープン中の進捗参照を返す。UI スレッドの `draw_video_hud` が
    /// `displayed_frame_seq() == 0` (= まだ最初のフレームが届いていない) のとき
    /// この atomic を読んで「メタデータ読込中... NN MB / YY MB」「ストリーム解析中...」
    /// 「デコード開始中...」を切り替え表示するために使う。
    pub fn prep_progress(&self) -> &crate::video::avio_progress::PreparingProgress {
        &self.decode.prep_progress
    }

    pub fn is_playing(&self) -> bool {
        self.clock.is_playing()
    }

    /// 「ユーザーが再生したいと思っているか」の intent。
    /// `is_playing()` は engine が `Playing` state にあるか (= AvClock が実際に進んで
    /// いるか) を返すが、これは Loading/Buffering/Seeking 中は autoplay=true でも
    /// false になる。UI 側の `toggle_play` / `set_playing` のような「ユーザー操作の
    /// 方向決定」には intent を読む必要がある (Codex P2 2026-05-17、詳細は
    /// `EngineActor::autoplay_intent` の doc コメント参照)。
    pub fn intent_playing(&self) -> bool {
        self.engine.lock().unwrap().autoplay_intent()
    }

    /// 再生時に提供する視覚モードを engine readiness へ反映する。
    /// Video は従来どおり初回映像提示を待ち、Music は音声バッファを再生開始条件にする。
    pub(crate) fn set_media_visual_mode(&self, mode: music_core::MediaVisualMode) {
        self.engine.lock().unwrap().set_media_visual_mode(mode);
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

    /// pause OR seek 処理中なら true。native presenter overlay の perf graph
    /// freeze 判定で使う (engine_state_atomic load + clock.is_seeking 1 回)。
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
        //
        // **EOF 判定** (2026-05-18 update): EofReached は engine に同期配線され、EOF 到達時に
        // engine.state=Eof + AvClock playing=false + is_eof_reached=true が atomic に確定する。
        // 「`!is_playing() && is_eof_reached()`」は engine_state==Eof と等価で、EOF 検出条件
        // として有効。`apply_command(Play)` の Eof arm 経由でも replay できる
        // (= `set_playing(true)` 経路) が、user の Space 入力は user seek 経路で
        // epoch 競合なく扱いたいので、ここでは明示的に `request_seek(0)` + `handle_seek_request(0)`
        // + `apply_command(Play)` を発行する。
        if !self.clock.is_playing() && self.clock.is_eof_reached() {
            self.clear_pending_user_seek();
            self.clear_frame_step_target();
            self.clock.request_seek(0.0);
            self.clear_audio_output_buffer();
            // 2026-05 root fix: `clock.set_playing(true)` の直書きは撤去。
            // 続く `handle_seek_request` → `apply_command(Play)` → engine 内部で
            // transition_to_seeking → transition_to_buffering → transition_to_playing
            // が走り、その中で `av_clock.engine_start_playing` が AvClock を更新する。
            //
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
        // 非 EOF: **intent 基準で方向決定** (Codex P2 2026-05-17)。
        // `is_playing()` は engine state=Playing のときだけ true なので、
        // Loading/Buffering/Seeking 中 (autoplay=true) に Space を押すと
        // `!is_playing()=true` から「再生したい」と誤判定して Pause→Play の往復になる。
        // intent_playing() は engine の autoplay 意図を直接読むので、準備中 + autoplay=true
        // でも `intent_playing()=true` となり、Space で正しく Pause へ遷移する。
        let new_playing = !self.intent_playing();
        if new_playing {
            self.clear_frame_step_target();
        } else {
            self.clear_pending_user_seek();
        }
        self.dispatch_play_pause(new_playing);
    }

    pub fn set_playing(&self, p: bool) {
        // **intent 基準で dispatch 判定** (Codex P2 2026-05-17): 旧版は
        // `prev = self.clock.is_playing()` を見ていたが、`is_playing()` は engine state
        // が Playing のときだけ true なので、Loading/Buffering/Seeking 中 (autoplay=true)
        // に `set_playing(false)` が呼ばれても `prev=false, p=false` で dispatch skip
        // → autoplay=true のまま準備完了し勝手に再生開始するバグになる。
        // `intent_playing()` を見れば「現状再生したいか」が分かるので、UI の意図と
        // engine の意図がずれているとき正しく dispatch する。
        let prev_intent = self.intent_playing();
        crate::logger::log(format!(
            "[video-debug] set_playing({p}) called: prev_intent={prev_intent} is_playing={} engine_state={} seek_serial={} video_rx_len={} audio_rx_len={}",
            self.clock.is_playing(),
            self.engine_state_name(),
            self.clock.current_seek_serial(),
            self.decode.video_rx.len(),
            self.decode.audio_rx.len()
        ));
        if p {
            self.clear_frame_step_target();
        } else {
            self.clear_pending_user_seek();
        }
        // **EOF replay 特例**: EofReached が engine に配線された後 (2026-05-18) は
        // engine.state=Eof / opts.autoplay は handle_pause で false に降ろされない限り
        // true を維持する。「autoplay=true で EOF に達した状態」では
        // `prev_intent=true, p=true` で通常なら dispatch skip 扱いになるが、ユーザーが
        // play ボタンを押したときは replay (= 0 から再生) を期待する。EOF + p=true は
        // 強制 dispatch することで `handle_play` の Eof arm (= `handle_seek_request(0)` +
        // autoplay 強制) を発火させ replay する。
        let force_dispatch = p && self.clock.is_eof_reached();
        if prev_intent != p || force_dispatch {
            self.dispatch_play_pause(p);
        }
        crate::logger::log(format!(
            "[video-debug] set_playing({p}) done: engine_state={} playing={} intent={} seek_serial={}",
            self.engine_state_name(),
            self.clock.is_playing(),
            self.intent_playing(),
            self.clock.current_seek_serial()
        ));
    }

    // 配信が時計なしトランスコードへ移り、再生器から映像 tap / 音声 tap / ローカル出力
    // ミュートを取り出す入口は無くなった。呼ばれない扉を残すと「配信は再生器から取れる」
    // という読み方が生き続けるので閉じてある。

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

    fn clear_pending_user_seek(&self) {
        if let Ok(mut state) = self.user_seek_coalesce.lock() {
            state.pending_target_secs = None;
        }
    }

    fn user_seek_base_secs(&self) -> f64 {
        self.user_seek_coalesce
            .lock()
            .ok()
            .and_then(|state| state.pending_target_secs)
            .unwrap_or_else(|| self.position())
    }

    fn issue_user_seek_locked(&self, state: &mut UserSeekCoalesceState, target_secs: f64) {
        self.clock.request_seek(target_secs);
        self.clear_audio_output_buffer();
        // 2026-05 root fix: `clock.set_playing(true)` の直書きは撤去。続く
        // `apply_command(Play)` が transition_to_playing 経由で AvClock を起動する。
        // user 操作 seek は autoplay 強制。
        // 呼び出し順注意: handle_seek_request → apply_command(Play)
        // (詳細は toggle_play を参照)。
        let mut g = self.engine.lock().unwrap();
        g.handle_seek_request(target_secs);
        g.apply_command(engine::actor::TransportCommand::Play);
        state.pending_target_secs = None;
        state.last_issued_at = Some(std::time::Instant::now());
        state.last_issued_display_seq = self.displayed_frame_seq.load(Ordering::Acquire);
    }

    fn request_user_seek(&self, target_secs: f64) {
        let target = self.clamp_seek_target(target_secs);
        let mut state = self.user_seek_coalesce.lock().unwrap();
        let now = std::time::Instant::now();
        let displayed_seq = self.displayed_frame_seq.load(Ordering::Acquire);
        if user_seek_ready_to_issue(&state, self.clock.is_seeking(), displayed_seq, now) {
            self.issue_user_seek_locked(&mut state, target);
        } else {
            state.pending_target_secs = Some(target);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "video",
                    "user_seek_coalesced",
                    None,
                    0,
                    &[
                        ("target", serde_json::Value::from(target)),
                        (
                            "displayed_seq",
                            serde_json::Value::from(displayed_seq as i64),
                        ),
                    ],
                );
            }
        }
    }

    fn maybe_issue_pending_user_seek(&self) {
        let mut state = self.user_seek_coalesce.lock().unwrap();
        let Some(target) = state.pending_target_secs else {
            return;
        };
        let now = std::time::Instant::now();
        let displayed_seq = self.displayed_frame_seq.load(Ordering::Acquire);
        if user_seek_ready_to_issue(&state, self.clock.is_seeking(), displayed_seq, now) {
            self.issue_user_seek_locked(&mut state, target);
        }
    }

    /// 絶対シーク (シークバークリック / ブックマーク等)。
    /// target 以前の keyframe から target まで preroll trim し、target ぴったりに着地する。
    /// target は `[0, duration - 0.1s)` にクランプされる。
    /// 一時停止中なら自動的に再生再開する (post-EOF / pause からの seek を
    /// ユーザー操作 1 回で完結させる)。
    pub fn seek(&self, target_secs: f64) {
        self.clear_frame_step_target();
        let clamped = self.clamp_seek_target(target_secs);
        crate::logger::log(format!(
            "[video-debug] seek({target_secs:.3}) called: clamped={clamped:.3} engine_state={} prev_seek_serial={} playing={} video_rx_len={} audio_rx_len={}",
            self.engine_state_name(),
            self.clock.current_seek_serial(),
            self.clock.is_playing(),
            self.decode.video_rx.len(),
            self.decode.audio_rx.len()
        ));
        self.request_user_seek(clamped);
        crate::logger::log(format!(
            "[video-debug] seek({target_secs:.3}) dispatched: engine_state={} seek_serial={} playing={} video_rx_len={} audio_rx_len={}",
            self.engine_state_name(),
            self.clock.current_seek_serial(),
            self.clock.is_playing(),
            self.decode.video_rx.len(),
            self.decode.audio_rx.len()
        ));
    }

    /// mIV Remote の seek request は応答した generation と実際の decoder seek を
    /// 1 対 1 に対応させる。通常 UI の連打 coalescing を通すと、進行中 seek の後で
    /// serial が遅れて進み、HTTP に返した generation が即 stale になり得るため使わない。
    pub(crate) fn seek_for_remote_streaming(&self, target_secs: f64) {
        self.clear_frame_step_target();
        let clamped = self.clamp_seek_target(target_secs);
        let mut state = self.user_seek_coalesce.lock().unwrap();
        self.issue_user_seek_locked(&mut state, clamped);
    }

    /// Freeze every player-owned input needed to start a clockless remote generation.
    ///
    /// `pending_resume_secs` is consumed only after metadata supplies the duration and
    /// `sanitize_resume_for_duration` has selected either the saved resume point or the start.
    /// When a resume is selected, `request_seek` synchronously publishes that target through
    /// `position_secs`, so the source origin is already final even though a paused player can keep
    /// its seek override indefinitely. We therefore must not wait for `clock.is_seeking()` here.
    ///
    /// The remote player is opened with autoplay disabled, which also disables deferred normalize
    /// scanning. Its gain is installed before decoder/audio workers start; copying it into this
    /// snapshot fixes the value used by the independent generation.
    pub(crate) fn remote_stream_start_inputs(&self) -> Option<RemoteStreamStartInputs> {
        let info = self.info.as_ref()?;
        if self.pending_resume_secs.is_some() {
            return None;
        }
        Some(RemoteStreamStartInputs {
            duration_secs: info.duration_secs,
            has_video: info.has_video,
            has_audio: info.has_audio,
            source_origin_secs: self.position_secs(),
            normalize_gain: self.normalize_gain(),
        })
    }

    pub(crate) fn log_remote_start_failure(&self, code: &str, detail: &str) {
        let readiness = self.engine.lock().unwrap().readiness_snapshot();
        let queued_events = self.engine_event_rx.len();
        let raw_pending_secs = self.clock.audio_raw_pending_secs();
        let processed_secs = self.clock.audio_processed_secs();
        let audio_tx_queued_secs = self.clock.audio_tx_queued_secs();
        let video_waiting = readiness.video_required && !readiness.video_ready;
        let audio_waiting = readiness.audio_required && !readiness.audio_ready;
        let clock_seeking = self.clock.is_seeking();
        crate::logger::log(format!(
            "[remote-video] start failed: code={code} detail={detail:?} state={} epoch={} video_waiting={video_waiting} audio_waiting={audio_waiting} video_required={} video_ready={} audio_required={} audio_ready={} queued_engine_events={} clock_seeking={clock_seeking} audio_raw_pending_secs={raw_pending_secs:.3} audio_processed_secs={processed_secs:.3} audio_tx_queued_secs={audio_tx_queued_secs:.3}",
            readiness.state,
            readiness.epoch,
            readiness.video_required,
            readiness.video_ready,
            readiness.audio_required,
            readiness.audio_ready,
            queued_events
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "remote_video",
                "start_failed",
                None,
                0,
                &[
                    ("code", serde_json::Value::from(code)),
                    ("engine_state", serde_json::Value::from(readiness.state)),
                    ("epoch", serde_json::Value::from(readiness.epoch as i64)),
                    (
                        "video_required",
                        serde_json::Value::from(readiness.video_required),
                    ),
                    (
                        "video_ready",
                        serde_json::Value::from(readiness.video_ready),
                    ),
                    ("video_waiting", serde_json::Value::from(video_waiting)),
                    (
                        "audio_required",
                        serde_json::Value::from(readiness.audio_required),
                    ),
                    (
                        "audio_ready",
                        serde_json::Value::from(readiness.audio_ready),
                    ),
                    ("audio_waiting", serde_json::Value::from(audio_waiting)),
                    ("clock_seeking", serde_json::Value::from(clock_seeking)),
                    (
                        "queued_engine_events",
                        serde_json::Value::from(queued_events as i64),
                    ),
                    (
                        "audio_raw_pending_secs",
                        serde_json::Value::from(raw_pending_secs),
                    ),
                    (
                        "audio_processed_secs",
                        serde_json::Value::from(processed_secs),
                    ),
                    (
                        "audio_tx_queued_secs",
                        serde_json::Value::from(audio_tx_queued_secs),
                    ),
                ],
            );
        }
    }

    #[cfg(test)]
    fn enqueue_remote_seek_readiness_for_test(&self) {
        let epoch = self.clock.current_seek_serial();
        self.engine_event_tx
            .send(EngineEvent::Decoder(
                engine::state::DecoderEvent::SeekCompleted {
                    epoch,
                    actual_pts: self.clock.now_secs(),
                },
            ))
            .unwrap();
        self.engine_event_tx
            .send(EngineEvent::Decoder(
                engine::state::DecoderEvent::FirstFrameReady {
                    epoch,
                    pts: self.clock.now_secs(),
                },
            ))
            .unwrap();
        self.engine_event_tx
            .send(EngineEvent::Audio(engine::state::AudioEvent::BufferReady {
                epoch,
                pts: self.clock.now_secs(),
                wall_now: std::time::Instant::now(),
            }))
            .unwrap();
    }

    #[cfg(test)]
    pub(crate) fn set_pending_remote_resume_for_test(&mut self, target_secs: f64) {
        self.pending_resume_secs = Some(target_secs);
    }

    #[cfg(test)]
    pub(crate) fn apply_pending_remote_resume_for_test(&mut self) {
        if let Some(target_secs) = self.pending_resume_secs.take() {
            let epoch = self.clock.current_seek_serial();
            self.engine.lock().unwrap().handle_decoder_event(
                engine::state::DecoderEvent::InfoReceived {
                    epoch,
                    duration_secs: 30.0,
                    has_audio: true,
                    has_video: true,
                },
            );
            self.clock.request_seek(target_secs);
            self.engine.lock().unwrap().handle_seek_request(target_secs);
            // Mirror the real paused metadata player: readiness reaches the actor, but without
            // playback consuming a frame/sample the clock seek override remains set.
            self.enqueue_remote_seek_readiness_for_test();
        }
    }

    #[cfg(test)]
    pub(crate) fn clock_is_seeking_for_test(&self) -> bool {
        self.clock.is_seeking()
    }

    /// 相対シーク (←→ ホットキー)。
    /// 絶対シークと同じ precise seek を使う。target 前の keyframe preview は表示せず、
    /// 現在フレームを保ったまま target 到達 frame を待つ。
    /// 一時停止中なら自動的に再生再開する。
    ///
    /// 既に先頭 / 末尾に居て要求方向へ実質シークできない場合は、シークを発行せず
    /// [`RelativeSeekOutcome::AtStart`] / [`AtEnd`](RelativeSeekOutcome::AtEnd) を
    /// 返す。`info.duration_secs` はコンテナ尺で最終フレームより後ろのことが多く、
    /// 末尾でシークを発行すると decoder が target 付近のフレームを返せず
    /// 「シーク中...」表示が固着する。呼び出し側はこの戻り値を見て境界トーストに
    /// 振り替える。
    ///
    /// さらに境界を検出した時点で、pending な user seek と seek override を
    /// 明示クリアする。直前の相対シークが末尾付近を target にして「シーク中...」
    /// 固着 (= override が通常経路で解除されないまま) になっているケースを、
    /// この境界判定のタイミングで回収して HUD 表示を正常化するため。
    pub fn seek_relative(&self, delta_secs: f64) -> RelativeSeekOutcome {
        self.clear_frame_step_target();
        let cur = self.user_seek_base_secs();
        let raw = (cur + delta_secs).max(0.0);
        let target = self.clamp_seek_target(raw);
        // 境界判定。先頭側と末尾側で **判定式そのものが違う** ことに注意。
        //
        // - **末尾側** (`delta > 0`): `target <= cur + 許容差`。動画が EOF で停止すると
        //   `cur` が clamp 上限 (`duration - 0.1`) 付近に張り付くので、許容差は狭くて
        //   よい。許容差はシーク粒度 (最小 1 秒) より小さく取る必要がある — そうしない
        //   と未 clamp の前進シーク (`target = cur + delta`) でも条件が成立してしまう。
        //
        // - **先頭側** (`delta < 0`): **`cur` の絶対位置**で判定する (`cur <= 許容差`)。
        //   再生中は `cur` が 0 から離れる方向にしか進まないため「再生開始直後に ← を
        //   押すと既に cur が 0 から離れていて AtStart にならない」(2026-05 報告)。
        //   ここで末尾側と同じ `target >= cur - 許容差` 形式を使うと、未 clamp の後退
        //   シークでは `target = cur - |delta|` なので、許容差 >= シーク粒度 (1 秒) の
        //   とき **常に成立**してしまい Shift+← (1 秒) が全く動かなくなる (2026-05 報告)。
        //   絶対位置判定なら粒度に依存せず、「先頭から 1 秒以内なら先頭扱い」で済む。
        const SEEK_START_BOUNDARY_TOLERANCE_SECS: f64 = 1.0;
        const SEEK_END_BOUNDARY_TOLERANCE_SECS: f64 = 0.01;
        let outcome = if delta_secs > 0.0 && target <= cur + SEEK_END_BOUNDARY_TOLERANCE_SECS {
            RelativeSeekOutcome::AtEnd
        } else if delta_secs < 0.0 && cur <= SEEK_START_BOUNDARY_TOLERANCE_SECS {
            RelativeSeekOutcome::AtStart
        } else {
            self.request_user_seek(target);
            return RelativeSeekOutcome::Seeked;
        };
        // 境界に達したときの pending / override クリアは **`is_eof_reached()` のときだけ**
        // 行う (Codex P1 反映)。
        //
        // 末尾付近を target にした seek は decoder が target 以降のフレームを返せず、
        // 通常の override 解除経路 (post-seek フレーム / 音声の消費) が発火しないまま
        // 固着する。これを掃除するのがこのクリアの目的。だが `cur` は
        // `user_seek_base_secs()` (= coalesce 中の pending target を優先) なので、
        // ←→ 押しっぱなしで pending target が clamp に到達すると、**実シークはまだ
        // 手前を向いている (= 正当な進行中 seek)** のに AtStart/AtEnd になり得る。
        // ここで無条件にクリアすると、その正当な in-flight seek の override と、
        // まだ発行されていない pending seek を巻き込んで潰してしまう。
        //
        // `is_eof_reached()` は demux がファイル全体を読み切ったときだけ true になり、
        // 「override がもう post-seek フレームを得られない = 固着」状態と一致する。
        // false のときは進行中 / pending の seek は正当なので touch しない —
        // 通常の override 解除経路、または tick 側の保険 (`seek_eof_stuck_since`,
        // 1200ms) に回収を任せる。
        // `current_seek_serial()` を completed_serial に渡すことで、直近のシーク
        // 世代の override だけを CAS で外す (新しいシークが割り込んでいたら何もしない)。
        if self.clock.is_eof_reached() {
            self.clear_pending_user_seek();
            self.clock
                .clear_seek_target_override(self.clock.current_seek_serial());
        }
        outcome
    }

    /// フレーム送り用の精密シーク。到着後は必ず一時停止状態に保つ。
    pub fn seek_paused(&self, target_secs: f64) {
        self.clear_pending_user_seek();
        self.clear_frame_step_target();
        self.seek_paused_internal(target_secs);
    }

    fn seek_paused_internal(&self, target_secs: f64) {
        self.clear_pending_user_seek();
        let clamped = self.clamp_seek_target(target_secs);
        self.clock.request_seek(clamped);
        self.clear_audio_output_buffer();
        // 2026-05 root fix: `clock.set_playing(false)` の直書きは撤去。続く
        // `apply_command(Pause)` が transition_to_seeking → transition_to_paused
        // (== handle_seek_request 直後の Seeking state、その後の SeekCompleted +
        // FirstFrameReady で Paused 入場) 経由で AvClock を freeze する。
        let mut g = self.engine.lock().unwrap();
        g.handle_seek_request(clamped);
        g.apply_command(engine::actor::TransportCommand::Pause);
    }

    fn seek_paused_frame_step_internal(
        &self,
        seek_start_secs: f64,
        base_secs: f64,
        direction: i32,
    ) {
        self.clear_pending_user_seek();
        let seek_start = self.clamp_exact_frame_target(seek_start_secs);
        let base = self.clamp_exact_frame_target(base_secs);
        self.clock
            .request_frame_step_seek(seek_start, base, direction);
        self.clear_audio_output_buffer();
        // 2026-05 root fix: `clock.set_playing(false)` の直書きは撤去 (上記
        // seek_paused_internal と同じ理由)。
        // `clock.set_paused_position(base)` は frame-step 固有の表示位置上書きで、
        // engine の transition_to_seeking が `engine_freeze_at(base)` を呼ぶのと
        // 同じ pts に揃うので冗長だが、frame-step 経路の即時応答性 (= engine 経路の
        // mpsc/Mutex 飛び越し) を保つために残す。
        self.clock.set_paused_position(base);
        let mut g = self.engine.lock().unwrap();
        g.handle_seek_request(base);
        g.apply_command(engine::actor::TransportCommand::Pause);
    }

    /// 前後 1 フレームへ移動し、一時停止する。
    pub fn step_frame(&self, direction: i32) {
        if direction == 0 {
            return;
        }
        self.clear_pending_user_seek();
        let pending_step_base = self.frame_step_base();
        let displayed_seq = self.displayed_frame_seq.load(Ordering::Acquire);
        let issued_display_seq = self.frame_step_issued_display_seq.load(Ordering::Acquire);
        if self.clock.is_seeking()
            && frame_step_waiting_for_display(pending_step_base, issued_display_seq, displayed_seq)
        {
            return;
        }
        let pending_step_base = if pending_step_base.is_some()
            && issued_display_seq != FRAME_STEP_NO_PENDING_SEQ
            && issued_display_seq != displayed_seq
        {
            None
        } else {
            pending_step_base
        };
        let base = frame_step_base_secs(
            pending_step_base,
            self.last_displayed_pts_secs(),
            self.position(),
        );
        let avg_fps = self.info.as_ref().map(|i| i.avg_fps).unwrap_or(0.0);
        let direction = direction.signum();
        let frame_interval = frame_step_interval_secs(avg_fps);
        if direction < 0 && base <= frame_interval * 0.25 {
            return;
        }
        if direction > 0
            && self.info.as_ref().is_some_and(|info| {
                info.duration_secs > 0.0 && base >= info.duration_secs - frame_interval * 1.25
            })
        {
            return;
        }
        let seek_start = frame_step_seek_start_secs(base, avg_fps, direction);
        self.frame_step_base_bits
            .store(base.to_bits(), Ordering::Release);
        self.frame_step_active.store(true, Ordering::Release);
        self.frame_step_issued_display_seq
            .store(displayed_seq, Ordering::Release);
        self.seek_paused_frame_step_internal(seek_start, base, direction);
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

    /// loop seek (= EOF からの自動ループ再開) 専用 target clamp。
    ///
    /// 通常の `clamp_seek_target` は `duration - 0.1` で頭打ちにするが、loop 用にこれを
    /// 使うと「loop_target_secs が duration 付近」のとき即 EOF → loop 再発火 → ... が
    /// 48ms 単位で繰り返される **スタッターループ** になる (T16, Claude R1-3)。
    /// Chapter / Bookmark ループの開始秒がたまたま末尾付近に設定された (例: 90 秒動画で
    /// チャプター開始が 89.5 秒) ケースで実害があった。
    ///
    /// 残再生 window が `SAFE_LOOP_MIN_REMAINING_SECS` (= 1.0 秒) 未満になる loop target は
    /// 0.0 に強制フォールバックする。これによりループは「全長を最初から」になり、stutter が
    /// 解消する (= ユーザー意図とは違う挙動だが、stutter loop よりは害が少ない無難な選択)。
    fn clamp_loop_seek_target(&self, target_secs: f64) -> f64 {
        const SAFE_LOOP_MIN_REMAINING_SECS: f64 = 1.0;
        let clamped = self.clamp_seek_target(target_secs);
        if let Some(info) = &self.info {
            if info.duration_secs > 0.0
                && (info.duration_secs - clamped) < SAFE_LOOP_MIN_REMAINING_SECS
            {
                return 0.0;
            }
        }
        clamped
    }

    fn clamp_exact_frame_target(&self, target_secs: f64) -> f64 {
        let lower = target_secs.max(0.0);
        if let Some(info) = &self.info {
            if info.duration_secs > 0.0 {
                return lower.min(info.duration_secs);
            }
        }
        lower
    }

    pub fn position(&self) -> f64 {
        self.clock.now_secs()
    }

    pub fn screenshot_target_secs(&self) -> f64 {
        self.last_displayed_pts_secs()
            .unwrap_or_else(|| self.position())
    }

    /// 最後に native presenter が `present()` 成功させた video frame の PTS (秒)。
    /// fast-swap で旧 source の最後の値が一時残るが、新 source の最初の present で
    /// 上書きされる。`f64::NAN` のときは `None` (= まだ何も表示していない)。
    /// `apply_normalize_gain_with_perf` などの perf event 出力時にも使う。
    pub(crate) fn last_displayed_pts_secs(&self) -> Option<f64> {
        let pts = f64::from_bits(self.last_displayed_pts_bits.load(Ordering::Acquire));
        if pts.is_finite() {
            Some(pts.max(0.0))
        } else {
            None
        }
    }

    /// `presenter.present()` 成功直後に書かれた A/V drift (signed ms)。
    /// `pts - clock.now_secs()` の値を `Source` 側で atomic に書く。
    /// + 方向 = 映像が音声より進んでいる、− 方向 = 遅れている。
    /// fast-swap でも同じ Arc を引き継ぐので overlay 表示が分断されない。
    pub fn av_drift_ms(&self) -> f32 {
        self.audio_diagnostics.load_av_drift_ms()
    }

    fn frame_step_base(&self) -> Option<f64> {
        let pts = f64::from_bits(self.frame_step_base_bits.load(Ordering::Acquire));
        if pts.is_finite() {
            Some(pts.max(0.0))
        } else {
            None
        }
    }

    fn clear_frame_step_target(&self) {
        self.frame_step_base_bits
            .store(f64::NAN.to_bits(), Ordering::Release);
        self.frame_step_active.store(false, Ordering::Release);
        self.frame_step_issued_display_seq
            .store(FRAME_STEP_NO_PENDING_SEQ, Ordering::Release);
    }

    pub fn is_frame_step_active(&self) -> bool {
        self.frame_step_active.load(Ordering::Acquire)
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

    /// 動画 decode thread が致命的なエラーで exit したかを返す。
    /// `tick()` はこのフラグを `error` に転写し、native overlay の
    /// 「準備中」を失敗表示へ切り替える (Codex P2 2026-05-15)。
    pub fn decode_failed(&self) -> bool {
        self.clock.decode_failed()
    }

    /// 音量ノーマライズの線形ゲイン (1.0 = 素通し)。
    pub fn normalize_gain(&self) -> f64 {
        self.clock.normalize_gain()
    }

    /// 出力セーフティリミッターが天井 (0 dBFS) を叩いた累積回数。
    /// native HUD は内部の `clock` から直接読むが、音楽ビュー (egui) はこの
    /// 公開 getter で seq の増加を検知してリミッター作動インジケータを点灯する。
    pub fn limiter_ceiling_hit_seq(&self) -> u64 {
        self.clock.limiter_ceiling_hit_seq()
    }

    /// 音量ノーマライズの線形ゲインを設定 (内部で ±24dB にクランプ)。
    ///
    /// 再生中の動画に呼んでも安全。新しい gain は次に `raw_pending` から `processed` へ
    /// 進む chunk から適用される。既存 `processed` の最大 ~100ms 分は旧 gain で鳴り続けるが、
    /// 100ms 程度の音量ズレは知覚しにくく、A/V offset は飛ばない。
    ///
    /// ## ⚠️ `clear_audio_output_buffer()` を呼ばないこと (= 2026-05-11 修正)
    ///
    /// 旧版の doc は「直後に clear」を推奨していたが、`clear_audio_output_buffer` は
    /// `raw_pending` (= 通常 5 秒分の先読み audio frame) も捨ててしまう。Norm では
    /// decoder flush しないため、捨てた直後に届く新しい audio frame の audible PTS が
    /// master clock から **5 秒先行**し、`set_audio_pts` の wall-rate cap で追従できず、
    /// **A/V offset = −5000ms 級の永続ズレ**が残った (= 1 回の Norm toggle で 5 秒、
    /// 累積で 10 秒 / 15 秒 / 20 秒 と永続的にズレた)。
    ///
    /// 本 method 単独では `processed` / `raw_pending` のいずれも触らない atomic store
    /// なので、上記問題は起きない。詳細は `docs/video-architecture.md` の
    /// 「Norm clear で audio が 5+ 秒先行する」節と
    /// `src/app/native_video.rs::apply_normalize_gain_with_perf` を参照。
    pub fn set_normalize_gain(&self, gain: f64) {
        self.clock.set_normalize_gain(gain);
    }

    pub fn audio_preroll_suspended(&self) -> bool {
        self.clock.audio_preroll_suspended()
    }

    pub fn set_audio_preroll_suspended(&self, suspended: bool) {
        self.clock.set_audio_preroll_suspended(suspended);
    }

    /// 音量ノーマライズの UI 状態 + 進捗 snapshot を native overlay に配信する。
    /// App 側は毎 update で呼び、overlay 側でボタン色 + 進捗パネル描画に使う。
    #[cfg(windows)]
    pub fn set_native_normalize_state(
        &self,
        state: crate::video::normalize_types::NormalizeOverlayState,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_normalize_overlay_state(state);
        }
    }

    /// ループ再生 ON/OFF を更新。App は毎 poll_video で settings 値を反映する。
    pub fn set_loop_enabled(&self, enabled: bool) {
        self.loop_enabled
            .store(enabled, std::sync::atomic::Ordering::Release);
    }

    /// EOF / 境界 tick からループ復帰先として使う秒値を atomic で書き込む。
    /// **入力サニタイズ**: `NaN` / `inf` は `0.0` に、負値は `0.0` にクランプする。
    /// 上限 (= duration) クランプは EOF 経路で `clamp_loop_seek_target` を再度通すので
    /// ここでは行わない (info() 未到着時に duration が 0 で潰されるのを避けるため)。
    pub fn set_loop_target_secs(&self, secs: f64) {
        let safe = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
        self.loop_target_bits
            .store(safe.to_bits(), std::sync::atomic::Ordering::Release);
    }

    /// 現在の loop seek target (秒) を読む。値はサニタイズ済み (`set_loop_target_secs`)
    /// だが duration クランプ + stutter loop 安全弁は未適用。EOF 経路では追加で
    /// `clamp_loop_seek_target` を通す (T16, 2026-05-16)。
    pub fn loop_target_secs(&self) -> f64 {
        f64::from_bits(
            self.loop_target_bits
                .load(std::sync::atomic::Ordering::Acquire),
        )
    }

    /// 現在の再生位置 (秒) を読む。`current_seek_serial` と組で境界 tick から使う薄い
    /// wrapper。内部的には `clock.now_secs()`。
    pub fn position_secs(&self) -> f64 {
        self.clock.now_secs()
    }

    #[cfg(windows)]
    pub fn native_presenter_hwnd(&self) -> u64 {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::hwnd)
            .unwrap_or(0)
    }

    /// Inc 7 hidden presenter (動画→音声モード): presenter ウィンドウ (+ HUD overlay) の
    /// 表示 / 非表示を要求する。native output が無い (音声ファイル等) ときは no-op。
    /// App (bin 専属) からのみ呼ばれる。lib build では app が stub のため dead に見える。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn set_native_window_visible(&self, visible: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_window_visible(visible);
        }
    }

    /// presenter ウィンドウが hide (consume-and-hold) 中か。exit の async 待ちで使う。
    /// native output が無いときは false。
    /// App (bin 専属) からのみ呼ばれる。lib build では app が stub のため dead に見える。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn native_presenter_hidden(&self) -> bool {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::is_presenter_hidden)
            .unwrap_or(false)
    }

    /// HUD overlay HWND (= bars / interactive UI 用の独立 top-level)。
    /// CP4 で presenter thread が `HudOverlayWindow::create` 成功時に store する。
    /// store されていなければ 0。
    ///
    /// `dsp_bridge.set_hud_hwnd(...)` 経由で bridge にも教える経路で使う
    /// (= raise allowlist の「mIV 既知 HWND」判定)。
    #[cfg(windows)]
    pub fn native_hud_hwnd(&self) -> u64 {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::hud_hwnd)
            .unwrap_or(0)
    }

    /// eframe 経由の pointer 活動は native presenter HWND を経由しないことがあるため、
    /// `push_native_event` で行われる cursor auto-hide タイマのリセットを明示的に
    /// 伝搬する。command channel を介すので 1〜2 フレームの遅延はあるが許容範囲。
    #[cfg(windows)]
    pub fn mark_cursor_activity(&self) {
        if let Some(output) = self.native_output.as_ref() {
            output.mark_cursor_activity();
        }
    }

    #[cfg(windows)]
    pub fn request_native_overlay_render(&self) {
        if let Some(output) = self.native_output.as_ref() {
            output.request_overlay_render();
        }
    }

    /// CP7: HUD overlay HWND の retry burst raise を presenter thread に依頼する。
    /// App.update で `dsp_bridge.hud_raise_hook` 経由で来た raise 要求を coalesce して
    /// 1 回だけ呼ぶ。HUD HWND が無いフォールバック経路では presenter 側で no-op になる。
    #[cfg(windows)]
    pub fn request_hud_raise(&self) {
        if let Some(output) = self.native_output.as_ref() {
            output.request_hud_raise();
        }
    }

    #[cfg(windows)]
    pub fn request_presenter_raise(&self) {
        if let Some(output) = self.native_output.as_ref() {
            output.request_presenter_raise();
        }
    }

    #[cfg(windows)]
    pub fn native_presenter_pending(&self) -> bool {
        self.native_output
            .as_ref()
            .map(|output| !output.first_presented() && !output.is_closed())
            .unwrap_or(false)
    }

    // VideoPlayer の native_video 系メソッド群は `app/native_video.rs` (bin 専属) からのみ
    // 呼ばれるため、lib build (`src/lib.rs` の `app` は constants のみの stub) では
    // dead に見える。bin (mimageviewer-core) では正常に使用されている。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn native_source_epoch(&self) -> Option<u64> {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::source_epoch)
    }

    /// App が信頼する現在の presenter placement 世代 (native_output があるときのみ)。
    /// close イベントの世代がこの値より小さければ stale として棄却する。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn native_committed_generation(&self) -> Option<u64> {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::committed_generation)
    }

    /// `PlacementSwitched` 受信時に committed 世代を進める (単調非減少)。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn bump_native_committed_generation(&self, generation: u64) {
        if let Some(output) = self.native_output.as_ref() {
            output.bump_committed_generation(generation);
        }
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

    /// 走行中の (headless で open した) プレイヤーに native presenter を新規 spawn して
    /// attach する (music VST シェル、Inc 6 ②-2)。`open()` が初回に native 出力を spawn
    /// するのと**同じ内部フィールド**を再利用するので、既存の `clock` / `video_rx` /
    /// `engine_event_tx` / audio pump / decoder / `DspBridge` / normalize / 解析状態を
    /// 一切作り直さない (= 音声無中断)。detach は既存の `take_native_output()` で行い、
    /// 返った `NativeVideoOutput` を drop すると presenter スレッドが cancel+join されて
    /// window が破棄される (音声スレッドには触れない)。
    ///
    /// 音声のみ (映像トラック無し) のプレイヤーで使う想定なので、`config.audio_only=true`
    /// を渡すこと (present ループが frameless で回る。§5.9 / Inc 6 ②-1)。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn attach_native_output_from_config(
        &mut self,
        config: NativeVideoOutputConfig,
    ) -> Result<(), String> {
        if self.native_output.is_some() {
            return Err("native output is already attached".to_string());
        }
        match NativeVideoOutput::spawn(
            self.decode.video_rx.clone(),
            Arc::clone(&self.clock),
            self.engine_event_tx.clone(),
            Arc::clone(&self.displayed_frame_seq),
            Arc::clone(&self.last_displayed_pts_bits),
            Arc::clone(&self.frame_step_active),
            Arc::clone(&self.duration_secs_bits),
            config,
            Arc::clone(&self.dynamic),
            Arc::clone(&self.audio_diagnostics),
        ) {
            Some(output) => {
                self.native_output = Some(output);
                Ok(())
            }
            None => Err("ネイティブプレゼンターのスレッド生成に失敗しました".to_string()),
        }
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
            frame_step_active: Arc::clone(&self.frame_step_active),
            duration_secs_bits: Arc::clone(&self.duration_secs_bits),
            dynamic: Arc::clone(&self.dynamic),
            audio_diagnostics: Arc::clone(&self.audio_diagnostics),
            source_epoch,
            fallback_file_name: self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("video")
                .to_string(),
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
    #[allow(dead_code)]
    pub(crate) fn switch_native_placement(
        &self,
        request_id: u64,
        placement: NativeVideoPlacement,
        owner_hwnd: u64,
        rect: windows::Win32::Foundation::RECT,
        activate_on_show: bool,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.switch_placement(request_id, placement, owner_hwnd, rect, activate_on_show);
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
    pub fn set_native_video_grade(&self, grade: crate::creative_lut::VideoGradeSnapshot) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_video_grade(grade);
        }
    }

    #[cfg(windows)]
    pub fn set_native_metadata(&self, metadata: Option<native_presenter::NativeOverlayMetadata>) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_metadata(metadata);
        }
    }

    #[cfg(windows)]
    pub(crate) fn set_native_side_panel_state(
        &self,
        mode: crate::settings::FsSidePanelMode,
        info_panel_open: crate::ui_helpers::MetadataPanelOpenState,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_side_panel_state(mode, info_panel_open);
        }
    }

    #[cfg(windows)]
    pub fn reset_native_side_panel_session(&self) {
        if let Some(output) = self.native_output.as_ref() {
            output.reset_side_panel_session();
        }
    }

    #[cfg(windows)]
    pub fn set_native_loop_enabled(&self, enabled: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_loop_enabled(enabled);
        }
    }

    /// HUD ボタン表示用のループモード (= ユーザー設定の display_mode) を presenter に伝える。
    /// 再生挙動 (= EOF で seek するか) には `set_native_loop_enabled` の bool を別途使う。
    /// 両者は app 側で `effective_loop_mode` を経由して算出される。
    #[cfg(windows)]
    pub fn set_native_loop_mode(&self, mode: crate::settings::VideoLoopMode) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_loop_mode(mode);
        }
    }

    #[cfg(windows)]
    pub fn set_native_continuous_mode(&self, mode: VideoContinuousMode) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_continuous_mode(mode);
        }
    }

    #[cfg(windows)]
    pub fn set_native_vst3_available(&self, available: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_vst3_available(available);
        }
    }

    #[cfg(windows)]
    pub fn set_native_hud_dimmed(&self, dimmed: bool) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_hud_dimmed(dimmed);
        }
    }

    #[cfg(windows)]
    pub fn set_native_text_contrast(&self, contrast: crate::settings::TextContrast) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_text_contrast(contrast);
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
    pub fn set_native_video_geometry(
        &self,
        num: u32,
        den: u32,
        orientation: display_metadata::VideoOrientation,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_video_geometry(num, den, orientation);
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
    pub fn set_native_ring_picker_overlay(
        &self,
        overlay: Option<native_presenter::NativeOverlayRingPicker>,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_ring_picker_overlay(overlay);
        }
    }

    #[cfg(windows)]
    pub fn set_native_ring_guide_overlay(
        &self,
        overlay: Option<native_presenter::NativeOverlayRingGuide>,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.set_ring_guide_overlay(overlay);
        }
    }

    #[cfg(windows)]
    pub fn set_native_playback_status(&self, first_frame_presented: bool, error: Option<String>) {
        if let Some(output) = self.native_output.as_ref() {
            let prep_status = self.decode.prep_progress.snapshot();
            output.set_playback_status(first_frame_presented, error, prep_status);
        }
    }

    /// native overlay にトーストを表示する。`linger` が `Some` のときその時間だけ
    /// 表示を維持し、`None` のとき presenter が `centered` から既定値を導く。
    #[cfg(windows)]
    pub fn show_native_overlay_toast(
        &self,
        text: String,
        centered: bool,
        linger: Option<std::time::Duration>,
    ) {
        if let Some(output) = self.native_output.as_ref() {
            output.show_toast(text, centered, linger);
        }
    }

    #[cfg(windows)]
    pub fn drain_native_presenter_events(&self) -> Vec<(u64, NativeVideoOutputEvent)> {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::drain_events)
            .unwrap_or_default()
    }

    #[cfg(windows)]
    pub(crate) fn native_overlay_input_routing_snapshot(
        &self,
    ) -> native_presenter::NativeOverlayInputRouting {
        self.native_output
            .as_ref()
            .map(NativeVideoOutput::overlay_input_routing_snapshot)
            .unwrap_or_default()
    }

    /// UI スレッドが毎フレーム呼ぶ。新しい info / video frame があれば反映する。
    /// 戻り値は次回再描画推奨時刻 (秒) — `ctx.request_repaint_after` に渡す目安。
    pub fn tick(&mut self, _ctx: &egui::Context) -> Option<std::time::Duration> {
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
                        // SAR (= sample aspect ratio) を native presenter に伝える。
                        // anamorphic 動画 (NTSC DVD など) で表示比を補正するために
                        // 1 度だけ送る。SAR=1:1 の動画では従来通りの isotropic 表示。
                        #[cfg(windows)]
                        self.set_native_video_geometry(
                            info.sar_num,
                            info.sar_den,
                            info.orientation,
                        );
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
                            // audio-only ファイル (映像トラック無し) は `has_video=false`。
                            // engine は has_video で `FirstFrameReady` 待ちを gate する
                            // (映像 thread が無く FirstFrameReady が来ないため)。
                            let has_video_effective = info.has_video;
                            // audio-only で audio 出力も起動できなかった (self.audio=None) →
                            // 再生できる出力がゼロ。engine は is_ready(false,false) で永久
                            // Buffering になるので、open エラーとして表面化し worker を畳む
                            // (無音のまま preparing で回り続けるのを防ぐ)。
                            if !has_video_effective && !has_audio_effective {
                                self.error = Some("音声出力を初期化できませんでした".to_string());
                                self.shutdown_workers_for_error();
                                return None;
                            }
                            let _ = self.engine_event_tx.try_send(EngineEvent::Decoder(
                                engine::state::DecoderEvent::InfoReceived {
                                    epoch: self.engine.lock().unwrap().current_seek_epoch(),
                                    duration_secs: info.duration_secs,
                                    has_audio: has_audio_effective,
                                    has_video: has_video_effective,
                                },
                            ));
                            self.info_event_emitted = true;
                        }

                        // resume 指定があれば最初の info 到着時に 1 度だけ実行。
                        // 末尾近く (残り 5 秒以下) なら 0 から再生 (= 完走済みと見なす)。
                        // 保存側 (`save_video_resume_position`) と同じ閾値で gate する。
                        if let Some(resume) = sanitize_resume_for_duration(
                            self.pending_resume_secs.take(),
                            info.duration_secs,
                        ) {
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
                            // open-time の resume は user 操作ではなく自動復元なので、
                            // 通常オープンやタイル/遅延オープンが渡した `OpenOptions.autoplay`
                            // をそのまま尊重する。
                            // user 操作の seek/seek_relative/toggle_play は
                            // 別経路で apply_command(Play) を呼ぶ。
                            self.engine.lock().unwrap().handle_seek_request(resume);
                        }
                        self.info = Some(info);
                    }
                    Err(e) => {
                        // T13 (Claude R-VENG-3): info_rx Err 経路でも他経路と同じく
                        // `shutdown_workers_for_error()` を呼ぶ。これを忘れると decoder
                        // panic / fatal の通知後も audio cpal stream / native presenter
                        // thread / thumbnail worker が Drop まで残り、UI が動画切替後も
                        // 旧 worker のリソース (= D3D11 keyed mutex, FFmpeg context) を
                        // 抱え続ける。他 2 経路 (init_error / decode_failed) はすぐ下で
                        // 同じ手順を踏んでいる。
                        self.error = Some(e);
                        self.shutdown_workers_for_error();
                        return None;
                    }
                }
            }
        }

        // Native presenter thread 内で起きた fatal init error を取り込む
        // (= CoInit / NativeWindowHost::create / NativeRenderCore::new 失敗)。
        // VideoPlayer::open() の同期エラーと同じ経路で UI に赤字エラーを表示し、
        // 同時に decoder/audio worker を停止して裏で再生パイプラインが残らないようにする。
        //
        // Race close: writer は (init_error 書き込み) → (closed.store(true, Release)) の順で
        // publish するので、reader が `closed=true` を Acquire load で観測した時点で
        // init_error も必ず見える。1 度目の take_init_error が None でも、その直後に
        // is_closed=true なら writer が我々を追い越して両方書いた可能性があるので
        // もう 1 度 take する (Codex P3 race 指摘)。
        #[cfg(windows)]
        if self.error.is_none() {
            if let Some(out) = self.native_output.as_ref() {
                let mut init_err = out.take_init_error();
                if init_err.is_none() && out.is_closed() {
                    init_err = out.take_init_error();
                }
                if let Some(err) = init_err {
                    self.error = Some(err);
                    self.shutdown_workers_for_error();
                }
            }
        }

        #[cfg(windows)]
        if self.error.is_none() && self.clock.decode_failed() {
            // D3D11VA 対応コーデックの HW デコード初期化/open が失敗した場合はここで
            // エラー停止する (= SW へは自動フォールバックしない、settings.rs の
            // `video_hw_decode` doc 参照)。古い内蔵 GPU ドライバ (AMD Vega / Intel
            // 旧世代) で実害が出るため、ユーザーが自力で対処できるよう設定への導線を
            // 文言に含める。設定パス/ラベルは `ui_dialogs/preferences/pages.rs` の
            // `page_video` と一致させること。
            self.error = Some(
                "動画のハードウェアデコードに失敗しました。環境設定 > 動画再生 の\
                 「ハードウェアデコードを有効にする」をオフにすると再生できる場合があります"
                    .to_string(),
            );
            self.shutdown_workers_for_error();
        }

        if self.error.is_some() {
            return None;
        }

        self.maybe_issue_pending_user_seek();

        // クロックの今時刻
        let now = self.clock.now_secs();

        // ── stuck seek (= 末尾より後ろを target にした seek) の保険解除 ──
        //
        // `info.duration_secs` はコンテナ尺で最終フレームの PTS より後ろのことが
        // 多く、その付近を target にした相対シークは backward seek 自体は成功する
        // (= seek 失敗経路の override clear が走らない) のに、video decoder が
        // target 以降のフレームを 1 枚も decode できず、post-seek フレーム / 音声の
        // 消費による override clear 経路も発火しない。結果 `seek_target_override` が
        // 固着し「シーク中...」が出続ける。
        //
        // `seek_relative` の境界判定でも回収するが、それは「次にもう一度 ←→ を
        // 押す」操作が前提。ここでは tick 側の最終保険として、`is_seeking()` のまま
        // `is_eof_reached()` (= demux がファイル全体を読み切った) が継続して true で
        // ある状態が `SEEK_STUCK_EOF_TIMEOUT` 続いたら override を強制クリアする。
        // - `is_eof_reached()` は `request_seek` で一旦クリアされ、demux が末尾まで
        //   読み切ったときだけ true になるので、進行中の通常 seek を誤検出しない。
        // - 通常の near-end seek は post-seek フレームが presenter / tick に届いた
        //   時点で override が clear され `is_seeking()` が false になるため、この
        //   timeout に達する前にラッチが解除される。
        // override をクリアするだけに留め、playing / 位置の更新は後段の既存 EOF
        // 処理 (native: ループ block / 非 native: line 末尾の EOF block) に任せる。
        const SEEK_STUCK_EOF_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1200);
        if self.clock.is_seeking() && self.clock.is_eof_reached() {
            let stuck_since = *self
                .seek_eof_stuck_since
                .get_or_insert_with(std::time::Instant::now);
            if stuck_since.elapsed() >= SEEK_STUCK_EOF_TIMEOUT {
                self.seek_eof_stuck_since = None;
                self.clear_pending_user_seek();
                let serial = self.clock.current_seek_serial();
                self.clock.clear_seek_target_override(serial);
                crate::logger::log(format!(
                    "[video] stuck seek override force-cleared after {}ms \
                     (seek target past last frame, serial={serial})",
                    SEEK_STUCK_EOF_TIMEOUT.as_millis(),
                ));
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "video",
                        "seek_override_force_clear_eof",
                        None,
                        0,
                        &[("serial", serde_json::Value::from(serial as i64))],
                    );
                }
            }
        } else {
            self.seek_eof_stuck_since = None;
        }

        #[cfg(windows)]
        self.pump_native_hover_thumbnail();

        #[cfg(windows)]
        if self.native_output.is_some() {
            // EOF 処理: native presenter 経路でも `clock.is_eof_reached()` を見て、
            // ループ ON ならループ seek、ループ OFF なら duration 位置で停止する。
            // native 経路はこの直後の early return で抜けるため非 native 経路の EOF
            // block (`set_position_at_eof` + `set_playing(false)`) には到達しない。
            // ここでループ OFF の停止を処理しないと、is_playing() のままクロックが
            // duration を超えて進み続ける (= ユーザー報告 2026-05「動画末尾を超えて
            // 再生が進む」)。
            // ★末尾の音声 drain と最終フレーム表示を待つ★ (Codex P1 第10ラウンド +
            // 第11ラウンド — demux EOF 時点で `is_eof_reached` が立つが、その直後に
            // audio worker が残フレームを drain しており、また pump 内には raw_pending /
            // processed / tx_queued の各 buffer が残っている。ここで即 seek すると
            // 末尾 ~10-100ms の音声 / 最終フレームが失われる)。
            // 完了条件:
            //   - decoder→presenter / decoder→audio pump channel 両方空
            //   - audio pump 内 buffer (processed + raw_pending + tx_queued) が全て quiet 閾値未満
            //   - presenter 内の自前 queue は直接観測できないが、video_rx_len==0 後の
            //     1 tick で消費されるので 16ms tick の遅延だけで実用上は十分
            //
            // ★閾値の設計★ (Codex P2 第13ラウンド + 第14ラウンド):
            // - `EOF_DRAIN_AUDIO_QUIET_TOL = 20ms`: 単発観測で「ほぼ drain 済み」とみなす上限。
            //   processed buffer は cpal-ready な実再生待ち音声なので、これより大きい値を
            //   許容すると末尾音声が切れる。
            // - `EOF_DRAIN_QUIET_TICKS = 3` (~48ms): pump 側の publish は
            //   (audio_tx_queued -= n) → (raw_pending push/pop) → VST/stretch 処理 →
            //   (publish processed) と段階を経るので、その handoff window 中に 3 counter が
            //   全て 0 を読む race がある。重めの VST3 plugin で 1 frame の処理が ~10-30ms
            //   かかる場合があるため、3 tick (~48ms) 連続で quiet を観測してから seek する。
            // 完全な解決には pump 側から「EOF drain 完了 / in-flight 数」を publish する形が
            // 良いが、連続観測ラッチで実用上は十分。VST/stretch が 48ms 超ブロックする状況は
            // UI 不応答相当 (= 通常運用ではほぼ起きない) なので、その race で末尾 1 frame が
            // 切れる確率は許容範囲とする。
            const EOF_DRAIN_AUDIO_QUIET_TOL: f64 = 0.020;
            const EOF_DRAIN_QUIET_TICKS: u32 = 3;
            let audio_drained = self.clock.audio_processed_secs() < EOF_DRAIN_AUDIO_QUIET_TOL
                && self.clock.audio_raw_pending_secs() < EOF_DRAIN_AUDIO_QUIET_TOL
                && self.clock.audio_tx_queued_secs() < EOF_DRAIN_AUDIO_QUIET_TOL;
            let channels_drained = self.audio_rx_len() == 0 && self.video_rx_len() == 0;
            // `loop_enabled` は **発火条件には含めない** (ループ ON/OFF どちらでも EOF
            // drain 完了を待つ)。発火後のアクションだけ loop_enabled で分岐する。
            let loop_enabled = self.loop_enabled.load(std::sync::atomic::Ordering::Acquire);
            let quiet_now = self.clock.is_eof_reached()
                && !self.clock.is_seeking()
                && self.is_playing()
                && channels_drained
                && audio_drained;
            let quiet_ticks = if quiet_now {
                self.eof_loop_quiet_ticks
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                    + 1
            } else {
                self.eof_loop_quiet_ticks
                    .store(0, std::sync::atomic::Ordering::Release);
                0
            };
            if quiet_ticks >= EOF_DRAIN_QUIET_TICKS {
                self.eof_loop_quiet_ticks
                    .store(0, std::sync::atomic::Ordering::Release);
                if loop_enabled {
                    // ループ ON: `loop_target_bits` (Full ループは 0.0、CH/BM ループは
                    // app が書き戻す「現区間の開始秒」) へ seek して再生継続。
                    // T16: `clamp_loop_seek_target` で「末尾近すぎる target は 0 に倒す」
                    // 安全弁を通し、48ms ごとの stutter loop を回避する。
                    let raw = self.loop_target_secs();
                    let target = self.clamp_loop_seek_target(raw);
                    self.clear_pending_user_seek();
                    self.clock.request_seek(target);
                    self.clear_audio_output_buffer();
                    // 2026-05 root fix: `clock.set_playing(true)` 直書きは撤去
                    // (engine 経由で更新)。
                    let mut g = self.engine.lock().unwrap();
                    g.handle_seek_request(target);
                    g.apply_command(engine::actor::TransportCommand::Play);
                } else {
                    // ループ OFF: 末端到達 → engine に `EofReached` を **同期的に** 渡す。
                    // engine の `transition_to_eof(duration)` が走り:
                    //   - 内部 MasterClock を Frozen(duration) に
                    //   - state = Eof / published_state.store(EOF, Release)
                    //   - av_clock.engine_freeze_at(duration) で AvClock も
                    //     Frozen(duration) + playing=false に
                    // が atomic に行われる。
                    //
                    // 旧コードは直接 `clock.set_position_at_eof(duration)` +
                    // `clock.set_playing(false)` を呼んでいたが、engine 側 state は
                    // Playing のままで二重管理になり、`set_playing(true)` の EOF replay
                    // 特例 (= handle_play の Eof arm) が発火せず replay できなかった
                    // (Codex P2-2 2026-05-17)。同期呼び出しで state を Eof に正しく
                    // 遷移させることでこの不整合を解消する。
                    //
                    // **duration_secs 不明 fallback** (Codex P2 2026-05-18): 旧 set_playing
                    // 直書きは duration 不明でも常に呼ばれていたので、duration 取れない
                    // ファイル (= ストリーミング系 / コンテナが duration を持たない種)
                    // でも EOF で停止していた。新コードで `duration_secs > 0` のときだけ
                    // EofReached を発火していると engine.state が Playing のまま残り、
                    // tick が無限に回り続ける退行になる。`duration_secs == 0` の場合は
                    // 現在位置 (= AvClock.now_secs) で freeze する。
                    let duration_for_eof = eof_freeze_position(
                        self.info.as_ref().map(|info| info.duration_secs),
                        self.clock.now_secs(),
                    );
                    let mut g = self.engine.lock().unwrap();
                    let cur_epoch = g.current_seek_epoch();
                    g.handle_decoder_event(engine::state::DecoderEvent::EofReached {
                        epoch: cur_epoch,
                        duration_secs: duration_for_eof,
                    });
                }
            }
            // 動画オープン中 (= 1 フレームも表示されていない) は preparing HUD の
            // 数値を 50ms ごとに更新するため強制 polling (Codex P3 第 13 ラウンド対応)。
            // egui スリープを防ぎ、`set_native_playback_status` が tick ごとに発火する。
            //
            // **engine_state も含む** (Codex P1 2026-05-17): 1 枚目表示済みでも engine が
            // Loading/Buffering/Seeking なら BufferReady などの readiness event 待ちが
            // engine_event_rx に積まれている可能性がある。次の tick が走らないと
            // drain_engine_events に到達せず transition_to_playing が永久に発火しない
            // 固着バグを構造的に塞ぐ。
            let preparing =
                self.displayed_frame_seq.load(Ordering::Relaxed) == 0 || self.is_engine_preparing();
            return if self.is_playing() || self.clock.is_seeking() {
                Some(std::time::Duration::from_millis(16))
            } else if preparing {
                Some(std::time::Duration::from_millis(50))
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
        //
        // **音声 drain gate** (2026-07-02, audio-only 対応): この非 native 経路は
        // headless audio (native_output=None) が必ず通る。audio-only では future_frames /
        // latest_renderable が常に空なので、drain gate 無しだと demux が読み切った瞬間
        // (= pump にまだ数秒の buffered audio が残る時点) に EofReached が発火して
        // **末尾音声が切れる**。native 経路 (上の early-return ブロック) と同じく、
        // audio 有効時は audio buffer が quiet になり連続 EOF_DRAIN_QUIET_TICKS 回
        // 観測してから EOF を確定する。audio 無し (video のみ / 非 native transient) は
        // 即 quiet 扱い。閾値の意味は native 側 doc コメント参照。
        const EOF_DRAIN_AUDIO_QUIET_TOL: f64 = 0.020;
        const EOF_DRAIN_QUIET_TICKS: u32 = 3;
        let audio_active_for_eof =
            self.audio.is_some() && self.info.as_ref().map(|i| i.has_audio).unwrap_or(false);
        let audio_drained = !audio_active_for_eof
            || (self.clock.audio_processed_secs() < EOF_DRAIN_AUDIO_QUIET_TOL
                && self.clock.audio_raw_pending_secs() < EOF_DRAIN_AUDIO_QUIET_TOL
                && self.clock.audio_tx_queued_secs() < EOF_DRAIN_AUDIO_QUIET_TOL
                && self.audio_rx_len() == 0);
        let seek_in_flight = self.clock.is_seeking();
        let eof_ready_now = self.clock.is_eof_reached()
            && self.future_frames.is_empty()
            && latest_renderable.is_none()
            && !seek_in_flight
            && self.is_playing()
            && audio_drained;
        let eof_quiet_ticks = if eof_ready_now {
            self.eof_loop_quiet_ticks
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                + 1
        } else {
            self.eof_loop_quiet_ticks
                .store(0, std::sync::atomic::Ordering::Release);
            0
        };
        if eof_ready_now && eof_quiet_ticks >= EOF_DRAIN_QUIET_TICKS {
            self.eof_loop_quiet_ticks
                .store(0, std::sync::atomic::Ordering::Release);
            if self.loop_enabled.load(std::sync::atomic::Ordering::Acquire) {
                // ループ再生 ON: `loop_target_bits` (= app が書き戻している
                // 「現区間の開始秒」 / Full ループでは 0.0) にシークし続行。
                // T16: `clamp_loop_seek_target` で「末尾近すぎる target は 0 に倒す」
                // 安全弁を通し、duration 直前 loop での stutter (48ms ごと再発火) を回避。
                let raw = self.loop_target_secs();
                let target = self.clamp_loop_seek_target(raw);
                self.clear_pending_user_seek();
                self.clock.request_seek(target);
                self.clear_audio_output_buffer();
                // 2026-05 root fix: `clock.set_playing(true)` 直書きは撤去
                // (engine 経由で更新)。
                // engine 側の epoch も同期 (= AvClock seek_serial と engine
                // current_seek_epoch の不整合を防ぐ)。loop 周回も autoplay 強制。
                // 呼び出し順注意: handle_seek_request → apply_command(Play)
                // (詳細は toggle_play を参照)。
                let mut g = self.engine.lock().unwrap();
                g.handle_seek_request(target);
                g.apply_command(engine::actor::TransportCommand::Play);
            } else {
                // 末端到達 → engine に EofReached を同期的に流し、state=Eof +
                // AvClock freeze(duration) + playing=false を atomic に確定する。
                // native 経路と同じ理由・処理 (詳細はそちらの doc コメント参照)。
                // duration_secs == 0 の fallback も同じ。
                let duration_for_eof = eof_freeze_position(
                    self.info.as_ref().map(|info| info.duration_secs),
                    self.clock.now_secs(),
                );
                let mut g = self.engine.lock().unwrap();
                let cur_epoch = g.current_seek_epoch();
                g.handle_decoder_event(engine::state::DecoderEvent::EofReached {
                    epoch: cur_epoch,
                    duration_secs: duration_for_eof,
                });
            }
        }

        // 最新フレームをテクスチャに反映
        let mut displayed_pts: Option<f64> = None;
        let upload_ms: f64 = 0.0;
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
                    let frame_step_active = self.is_frame_step_active();
                    if clock::pts_clears_seek_override(pts, now_for_clear) {
                        if self.clock.is_seeking() && frame_step_active {
                            self.clock.set_paused_position(pts);
                            self.clock.clear_seek_target_override(serial);
                        } else if frame_step_active {
                            // Exact frame-step is paused while the decoder resolves or after
                            // its selected frame has already frozen the clock.
                        } else if self.clock.is_audio_active() {
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
                decoder::VideoFrameData::Cpu(_b) => {
                    // override は frame.pts が override target 近傍のときだけ解除する。
                    // backward seek が外れて pts ≈ 元位置のフレームが新世代 serial で来た
                    // 場合に override を消すと、シークバーが target → 元位置にスナップバック
                    // する (= 「← シークが効かない」現象の本質)。target 近傍チェックを
                    // 入れて「シークが物理的に成功した」ときだけ通常クロックに戻す。
                    let now_after = self.clock.now_secs();
                    let frame_step_active = self.is_frame_step_active();
                    if clock::pts_clears_seek_override(frame.pts_secs, now_after) {
                        // Audio-active playback must let fill_output clear the override only
                        // when the first audible post-seek samples actually reach the output.
                        // Clearing from video here starts the visual clock before audio is ready
                        // and produces AV drift on high-rate files with deep audio queues.
                        if self.clock.is_seeking() && frame_step_active {
                            self.clock.set_paused_position(frame.pts_secs);
                            self.clock.clear_seek_target_override(frame.seek_serial);
                        } else if frame_step_active {
                            // Keep the frame-step pause anchored even if queued frames arrive
                            // while the decoder-side adjacent-frame seek is still pending.
                        } else if self.clock.is_audio_active() {
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
                    // CPU フレームの実際の pixel upload は native presenter
                    // (`native_presenter/mod.rs`) が `UpdateSubresource` 経由で行う。
                    // ここに到達する経路は native_output を持たない非 Windows ビルド等
                    // 限定的なケースで、実際の表示は行われない (PTS bookkeeping のみ)。
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

        // 再生中 / seek 中 / 動画オープン中 なら repaint 予約。
        // seek 中も polling 必須: post-seek 第一フレームが channel に積まれても
        // egui に repaint 要求が無いと UI が起きず channel が drain されない。
        //
        // 動画オープン中 (`displayed_frame_seq == 0` の間) も polling 必須
        // (Codex P3 第 13 ラウンド): native presenter の center HUD は
        // `set_native_playback_status` で送られた `prep_status` snapshot を見て
        // 「メタデータ読込中... NN MB / YY MB」を描画している。app.rs:13373 がこの
        // 呼び出しを行うのは egui の update tick の中でだけなので、ここで repaint を
        // 要求しないと egui がスリープして bytes_read が増えても overlay に届かない。
        // 50ms 間隔で polling すれば、HUD の数値は概ね 50ms ごとに更新される
        // (= 体感は十分滑らか、CPU 負担も無視できる)。
        // **engine_state も含む** (Codex P1 2026-05-17、上記 native 経路と同じ理由):
        // 1 枚目表示済みでも engine が Loading/Buffering/Seeking なら preparing 扱い。
        //
        // **audio-only では displayed_frame_seq が永久に 0** (映像 frame を表示しない)
        // なので、`==0` を無条件に preparing 扱いすると Paused/Eof でも 50ms repaint を
        // 返し続け egui がスリープできない (Codex P2, round 2)。has_video のときだけ
        // frame 未表示を preparing に含め、audio-only は engine state だけで判定する。
        let has_video = self.info.as_ref().map(|i| i.has_video).unwrap_or(true);
        let preparing = (has_video && self.displayed_frame_seq.load(Ordering::Relaxed) == 0)
            || self.is_engine_preparing();
        if self.is_playing() || seek_in_flight_for_display || preparing {
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
            if preparing {
                // 準備中は 50ms で十分。is_playing と組み合わさったときは min が効いて
                // より短い方が採用される。
                due = due.min(std::time::Duration::from_millis(50));
            }
            Some(due)
        } else {
            None
        }
    }

    #[cfg(windows)]
    fn pump_native_hover_thumbnail(&self) {
        let Some(output) = self.native_output.as_ref() else {
            return;
        };
        let request = self
            .native_hover_thumbnail_request
            .lock()
            .ok()
            .and_then(|request| *request);
        let Some(request) = request else {
            return;
        };
        let Some(thumb) = self.nearest_seek_thumbnail(request.target_secs, request.tolerance_secs)
        else {
            self.request_seek_thumbnail(request.target_secs, request.tolerance_secs);
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
            match_tolerance_secs: crate::video::thumbnail::cache_lookup_tolerance(
                request.tolerance_secs,
                self.info.as_ref().map(|info| info.avg_fps).unwrap_or(0.0),
            ),
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

    /// EngineActor の published state が terminal EOF なら true。
    /// AvClock の eof_reached は互換複製なので、resume ownership 判定ではこちらを使う。
    pub fn is_at_eof(&self) -> bool {
        self.engine_state_code() == engine::actor::state_code::EOF
    }

    /// engine が readiness 待ちの遷移中か (= Loading / Buffering / Seeking)。
    /// `tick()` の `preparing` 判定で「次の tick を 50ms 後に予約する」かを決める用途。
    ///
    /// **重要** (Codex P1 2026-05-17): 旧コードは `displayed_frame_seq == 0` だけを
    /// preparing 扱いにしていたが、`FirstFrameReady` が audio の `BufferReady` より先に
    /// 届くケース (= h264 hot GOP + cpal の 70ms 音声起動遅延) で 1 枚目表示後すぐ
    /// preparing=false となり、tick が repaint を予約せず engine が Buffering で固着
    /// していた。`BufferReady` イベントは engine_event_rx に積まれるが、次の UI tick が
    /// 走らないと `drain_engine_events` に到達しないため、transition_to_playing が
    /// 発火せず「1 枚目だけ出てそのまま静止」という症状になる。
    pub fn is_engine_preparing(&self) -> bool {
        matches!(
            self.engine_state_code(),
            engine::actor::state_code::LOADING
                | engine::actor::state_code::BUFFERING
                | engine::actor::state_code::SEEKING
        )
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

    /// presenter thread 内 init error を取り込んで `self.error` に転写する。
    /// App 側 (`update_video_state`) が `native_presenter_closed()` を観測したタイミングで
    /// 防御的に呼ぶ — `closed=true` を Acquire load で観測した時点で、writer の
    /// init_error 書き込み (Release-before) が必ず可視なので、ここで take すれば
    /// tick との二重 race も含めて確実に拾える。冪等。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn consume_native_init_error(&mut self) {
        if self.error.is_some() {
            return;
        }
        if let Some(out) = self.native_output.as_ref() {
            if let Some(err) = out.take_init_error() {
                self.error = Some(err);
                self.shutdown_workers_for_error();
            }
        }
    }

    /// 呼び出し元 (= `start_fs_load`) が `native_video_presenter_config` の
    /// 取得に同期的に失敗したとき、construct 済み player に init エラーを伝える
    /// 正規 API。`error` フィールドに message を立てた直後に
    /// `shutdown_workers_for_error()` を呼んで decoder / audio / thumb worker を
    /// 停止する (= `tick()` が `error.is_some()` で早期 return するため、
    /// 放置すると裏で再生パイプラインだけが回り続けることを防ぐ)。
    ///
    /// `VideoPlayer::open(..., native_output_config=None)` は fast-swap 経路で
    /// 意図的に呼ばれるシグナルなので open() 内ではエラー扱いしない。実エラー
    /// 判定は呼び出し元の責務。
    #[cfg(windows)]
    #[allow(dead_code)]
    pub(crate) fn fail_native_init(&mut self, message: String) {
        self.error = Some(message);
        self.shutdown_workers_for_error();
    }

    /// `error` を立てた直後に裏で動き続けている decoder / audio / thumbnail worker を
    /// 停止する。呼び出し元は 3 経路:
    ///   1. `open()` の spawn 失敗パス (config=Some だが presenter スレッド生成失敗)
    ///   2. `consume_native_init_error()` (presenter thread 内 init 失敗を tick で検知)
    ///   3. `fail_native_init()` (呼び出し元が `native_video_presenter_config` の
    ///      同期取得に失敗 = config=None を検知して通知)
    ///
    /// 内部的には `shutdown()` と同じ処理 (cancel フラグ + audio drop) に加えて
    /// thumbnail worker も解放する。完全 idempotent なので open() から複数回呼んでも安全。
    fn stop_video_output(&mut self) {
        let previous = std::mem::replace(&mut self.video_output, VideoOutputState::Inactive);
        if let VideoOutputState::RemoteHeadless(headless) = previous {
            drop(headless);
        }
    }

    fn shutdown_workers_for_error(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
        self.stop_video_output();
        self.pause_audio_output();
        self.clear_audio_output_buffer();
        self.audio.take();
        self.thumb_worker.take();
        #[cfg(windows)]
        {
            if let Some(mut frame) = self.gpu_latest.take() {
                frame.reset_unpresented_shared_output();
            }
            native_drain_unpresented_queue(&mut self.future_frames);
        }
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
        self.stop_video_output();
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
        self.stop_video_output();
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
        prep_progress: crate::video::avio_progress::PreparingProgress::new(),
        video_tap: crate::video::stream::video_tap::VideoTapController::disconnected(),
    }
}

fn dummy_audio_rx() -> crossbeam_channel::Receiver<decoder::AudioFrame> {
    let (_, rx) = crossbeam_channel::bounded(0);
    rx
}

fn dummy_video_rx() -> crossbeam_channel::Receiver<VideoFrame> {
    let (_, rx) = crossbeam_channel::bounded(0);
    rx
}

#[cfg(test)]
mod tests {
    fn test_video_frame(epoch: u64, pts: f64) -> super::VideoFrame {
        super::VideoFrame {
            width: 1,
            height: 1,
            data: super::VideoFrameData::Cpu(vec![0; 4]),
            sar_num: 1,
            sar_den: 1,
            pts_secs: pts,
            seek_serial: epoch,
            orientation: crate::video::display_metadata::VideoOrientation::IDENTITY,
        }
    }

    fn test_video_output_route(
        consumer: super::VideoOutputConsumer,
        capacity: usize,
    ) -> (
        crossbeam_channel::Sender<super::VideoFrame>,
        super::VideoOutputRoute,
        crossbeam_channel::Receiver<super::EngineEvent>,
        std::sync::Arc<super::AvClock>,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        let (video_tx, video_rx) = crossbeam_channel::bounded(capacity);
        let (event_tx, event_rx) = crossbeam_channel::bounded(64);
        let seek_serial = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let clock = std::sync::Arc::new(super::AvClock::new(1.0, seek_serial));
        let displayed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let route = super::route_video_output(
            consumer,
            video_rx,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::clone(&clock),
            event_tx,
            std::sync::Arc::clone(&displayed),
            std::sync::Arc::new(std::sync::atomic::AtomicU64::new(f64::NAN.to_bits())),
        );
        (video_tx, route, event_rx, clock, displayed)
    }

    fn verify_headless_queue_drain(
        video_tx: crossbeam_channel::Sender<super::VideoFrame>,
        event_rx: crossbeam_channel::Receiver<super::EngineEvent>,
        displayed: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        send_headless_test_frames(&video_tx);
        assert_headless_first_frame_ready(&event_rx);
        wait_for_displayed_frames(&displayed, 64);
    }

    fn send_headless_test_frames(video_tx: &crossbeam_channel::Sender<super::VideoFrame>) {
        for frame in 0..64 {
            video_tx
                .send_timeout(
                    test_video_frame(0, frame as f64 / 60.0),
                    std::time::Duration::from_secs(1),
                )
                .unwrap();
        }
    }

    fn assert_headless_first_frame_ready(
        event_rx: &crossbeam_channel::Receiver<super::EngineEvent>,
    ) {
        assert!(matches!(
            event_rx.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(super::EngineEvent::Decoder(
                super::engine::state::DecoderEvent::FirstFrameReady { epoch: 0, .. }
            ))
        ));
    }

    fn wait_for_displayed_frames(displayed: &std::sync::atomic::AtomicU64, expected: u64) {
        use std::sync::atomic::Ordering;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while displayed.load(Ordering::Acquire) < expected && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(displayed.load(Ordering::Acquire), expected);
    }

    fn test_playing_engine(
        clock: std::sync::Arc<super::AvClock>,
        seek_serial: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) -> super::EngineActor {
        let mut engine = super::EngineActor::new(
            super::OpenOptions {
                autoplay: true,
                ..Default::default()
            },
            seek_serial,
            clock,
        );
        engine.begin_loading();
        engine.handle_decoder_event(super::engine::state::DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 120.0,
            has_audio: true,
            has_video: true,
        });
        engine.handle_decoder_event(super::engine::state::DecoderEvent::FirstFrameReady {
            epoch: 0,
            pts: 0.0,
        });
        engine.handle_audio_event(super::engine::state::AudioEvent::BufferReady {
            epoch: 0,
            pts: 0.0,
            wall_now: std::time::Instant::now(),
        });
        engine
    }

    fn headless_route_for_clock(
        clock: std::sync::Arc<super::AvClock>,
    ) -> (
        crossbeam_channel::Sender<super::VideoFrame>,
        super::VideoOutputRoute,
        crossbeam_channel::Receiver<super::EngineEvent>,
    ) {
        let (video_tx, video_rx) = crossbeam_channel::bounded(1);
        let (event_tx, event_rx) = crossbeam_channel::bounded(8);
        let route = super::route_video_output(
            super::VideoOutputConsumer::RemoteHeadless,
            video_rx,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            clock,
            event_tx,
            std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            std::sync::Arc::new(std::sync::atomic::AtomicU64::new(f64::NAN.to_bits())),
        );
        (video_tx, route, event_rx)
    }

    #[test]
    fn presentation_output_keeps_the_decoder_receiver_for_existing_consumers() {
        let (video_tx, route, _, _, _) =
            test_video_output_route(super::VideoOutputConsumer::Presentation, 1);
        assert!(matches!(
            &route.state,
            super::VideoOutputState::Presentation
        ));
        assert!(route.init_error.is_none());

        video_tx.try_send(test_video_frame(0, 1.0)).unwrap();
        assert!(matches!(
            video_tx.try_send(test_video_frame(0, 2.0)),
            Err(crossbeam_channel::TrySendError::Full(_))
        ));
        assert_eq!(route.player_rx.recv().unwrap().pts_secs, 1.0);
    }

    #[test]
    fn remote_headless_output_continuously_releases_decoder_queue_capacity() {
        let (video_tx, route, event_rx, _, displayed) =
            test_video_output_route(super::VideoOutputConsumer::RemoteHeadless, 1);
        assert!(matches!(
            &route.state,
            super::VideoOutputState::RemoteHeadless(_)
        ));
        assert!(route.init_error.is_none());
        verify_headless_queue_drain(video_tx, event_rx, displayed);
    }

    #[test]
    fn remote_headless_output_follows_seek_epoch_started_after_consumer() {
        let (video_tx, route, event_rx, clock, displayed) =
            test_video_output_route(super::VideoOutputConsumer::RemoteHeadless, 1);
        assert!(matches!(
            &route.state,
            super::VideoOutputState::RemoteHeadless(_)
        ));

        clock.request_seek(20.0);
        let live_epoch = clock.current_seek_serial();
        assert_eq!(live_epoch, 1);
        video_tx.send(test_video_frame(0, 19.0)).unwrap();
        video_tx.send(test_video_frame(live_epoch, 20.0)).unwrap();

        assert!(matches!(
            event_rx.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(super::EngineEvent::Decoder(
                super::engine::state::DecoderEvent::FirstFrameReady {
                    epoch: 1,
                    pts: 20.0,
                }
            ))
        ));
        wait_for_displayed_frames(&displayed, 1);
    }

    #[test]
    fn dropping_remote_headless_output_releases_its_decoder_receiver() {
        let (video_tx, route, _, _, _) =
            test_video_output_route(super::VideoOutputConsumer::RemoteHeadless, 1);
        drop(route);
        assert!(matches!(
            video_tx.try_send(test_video_frame(0, 0.0)),
            Err(crossbeam_channel::TrySendError::Disconnected(_))
        ));
    }

    #[test]
    fn remote_headless_first_frame_allows_seek_to_leave_seeking() {
        let seek_serial = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let clock = std::sync::Arc::new(super::AvClock::new(1.0, seek_serial.clone()));
        let mut engine = test_playing_engine(clock.clone(), seek_serial);
        assert_eq!(
            engine.published_state_code(),
            super::engine::actor::state_code::PLAYING
        );

        engine.apply_command(super::engine::actor::TransportCommand::SeekAbsolute {
            target_secs: 30.0,
        });
        let epoch = clock.current_seek_serial();
        assert_eq!(
            engine.published_state_code(),
            super::engine::actor::state_code::SEEKING
        );
        engine.handle_decoder_event(super::engine::state::DecoderEvent::SeekCompleted {
            epoch,
            actual_pts: 30.0,
        });

        let (video_tx, _route, event_rx) = headless_route_for_clock(clock.clone());
        video_tx.send(test_video_frame(epoch, 30.0)).unwrap();
        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let super::EngineEvent::Decoder(event) = event else {
            panic!();
        };
        engine.handle_decoder_event(event);
        engine.handle_audio_event(super::engine::state::AudioEvent::BufferReady {
            epoch,
            pts: 30.0,
            wall_now: std::time::Instant::now(),
        });

        assert_eq!(
            engine.published_state_code(),
            super::engine::actor::state_code::PLAYING
        );
        assert!(clock.is_seeking());
        clock.clear_seek_target_override(epoch);
        assert!(!clock.is_seeking());
    }

    #[test]
    fn native_presenter_visibility_honors_hidden_startup_state() {
        let hidden =
            super::NativePresenterVisibility::new(super::NativeVideoInitialVisibility::Hidden);
        let visible =
            super::NativePresenterVisibility::new(super::NativeVideoInitialVisibility::Visible);
        assert!(hidden.is_hidden());
        assert!(!visible.is_hidden());
    }

    #[test]
    fn paused_visible_loop_refreshes_settled_resize_before_idling() {
        use super::VisibleVideoLoopAction::{ProcessFrames, RefreshSettledResizeThenIdle};

        assert_eq!(
            super::visible_video_loop_action(false, false, false, false),
            RefreshSettledResizeThenIdle
        );

        for state in [
            (true, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
            (false, false, false, true),
        ] {
            assert_eq!(
                super::visible_video_loop_action(state.0, state.1, state.2, state.3),
                ProcessFrames,
                "state={state:?}"
            );
        }
    }

    #[test]
    fn video_grade_render_change_ignores_overlay_only_snapshot_fields() {
        let previous = crate::creative_lut::VideoGradeSnapshot::default();
        let mut next = crate::creative_lut::VideoGradeSnapshot::default();
        next.slots = vec![Some("renamed slot".to_string())].into();
        assert!(!super::video_grade_render_changed(&previous, &next));

        next.adjustments.brightness = 1.0;
        assert!(super::video_grade_render_changed(&previous, &next));
    }

    #[test]
    fn frame_presentation_new_visible_retires_previous_visible() {
        let mut state = super::FramePresentationState::Empty;
        assert!(state.replace_visible("first", 7).is_none());

        let displaced = state
            .replace_visible("second", 11)
            .expect("previous visible frame must be displaced");
        match displaced {
            super::DisplacedPresentation::Visible { frame, fence } => {
                assert_eq!(frame, "first");
                assert_eq!(fence, 7);
            }
            super::DisplacedPresentation::Hidden { .. } => {
                panic!("visible replacement must retire a visible frame")
            }
        }
        assert!(state.is_visible());
    }

    #[test]
    fn frame_presentation_grade_change_requests_only_visible_represent() {
        let mut state = super::FramePresentationState::Empty;
        assert!(!state.should_represent_for_grade_change(true));

        assert!(state.replace_hidden("hidden").is_none());
        assert!(state.is_hidden());
        assert!(!state.should_represent_for_grade_change(true));

        let _ = state.replace_visible("visible", 3);
        assert!(state.should_represent_for_grade_change(true));
    }

    #[test]
    fn frame_presentation_unchanged_grade_does_not_request_idle_present() {
        let mut state = super::FramePresentationState::Empty;
        assert!(state.replace_visible("visible", 5).is_none());
        assert!(!state.should_represent_for_grade_change(false));
    }

    #[test]
    fn frame_presentation_hide_preserves_frame_without_represent_request() {
        let mut state = super::FramePresentationState::Empty;
        assert!(state.replace_visible("visible", 13).is_none());

        state.hide();

        assert!(state.is_hidden());
        assert!(!state.should_represent_for_grade_change(true));
        assert_eq!(state.frame(), Some(&"visible"));
    }

    #[test]
    fn hidden_presenter_drains_decoder_channel_and_holds_latest_frame() {
        let mut paced_queue = std::collections::VecDeque::from([0, 1, 2, 3, 4, 5, 6, 7]);
        let (tx, rx) = crossbeam_channel::bounded(8);
        for frame in 8..16 {
            tx.try_send(frame).expect("fill decoder video channel");
        }
        let mut drained = Vec::new();

        assert_eq!(
            super::drain_hidden_available_frames(&mut paced_queue, &rx, &mut drained),
            16
        );
        assert!(paced_queue.is_empty());
        assert!(rx.is_empty());
        tx.try_send(16)
            .expect("hidden drain must release decoder channel capacity");

        let mut state = super::FramePresentationState::Empty;
        let mut displaced = Vec::new();
        for frame in drained {
            if let Some(previous) = state.replace_hidden(frame) {
                match previous {
                    super::DisplacedPresentation::Hidden { frame } => displaced.push(frame),
                    super::DisplacedPresentation::Visible { .. } => {
                        panic!("hidden burst must not create visible presentation ownership")
                    }
                }
            }
        }
        assert_eq!(state.frame(), Some(&15));
        assert_eq!(displaced, (0..15).collect::<Vec<_>>());
        assert!(state.mark_current_visible(19));
        assert!(state.is_visible());
        assert_eq!(state.visible_frame(), Some(&15));
    }

    #[test]
    fn audio_mode_eof_freeze_does_not_exceed_known_duration() {
        let overrun_clock = 273.0;
        let duration = 264.0;

        assert_eq!(
            super::eof_freeze_position(Some(duration), overrun_clock),
            duration
        );
        assert!(
            super::eof_freeze_position(Some(duration), overrun_clock) <= duration,
            "native EOF must freeze the position display at the media duration"
        );
    }

    #[test]
    fn frame_presentation_failed_prime_keeps_visible_fallback() {
        let mut state = super::FramePresentationState::Empty;
        assert!(state.replace_visible("paused", 17).is_none());

        assert_eq!(state.frame(), Some(&"paused"));
        assert!(state.is_visible());
        assert_eq!(state.visible_frame(), Some(&"paused"));
    }

    #[cfg(windows)]
    #[test]
    fn child_presenter_focus_requires_activation() {
        assert!(super::native_child_should_set_focus(
            super::NativeVideoPlacement::DetachedViewerChild,
            true
        ));
        assert!(!super::native_child_should_set_focus(
            super::NativeVideoPlacement::DetachedViewerChild,
            false
        ));
        assert!(!super::native_child_should_set_focus(
            super::NativeVideoPlacement::FullscreenBorderless,
            true
        ));
    }

    #[cfg(windows)]
    #[test]
    fn native_video_placement_mapping_is_exhaustive() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum ExpectedMode {
            Child,
            Borderless,
            WindowedAt,
        }

        let rect = windows::Win32::Foundation::RECT {
            left: 11,
            top: 22,
            right: 333,
            bottom: 444,
        };
        let owner_hwnd = 0x1234_u64;
        for placement in [
            super::NativeVideoPlacement::MainWindowChild,
            super::NativeVideoPlacement::FullscreenBorderless,
            super::NativeVideoPlacement::DetachedViewerChild,
            super::NativeVideoPlacement::DetachedWindow,
        ] {
            // Exhaustive match: placement 追加時はこの test 自体が compile error になる。
            let (expected_mode, expected_owner, expected_hud) = match placement {
                super::NativeVideoPlacement::MainWindowChild => {
                    (ExpectedMode::Child, owner_hwnd, false)
                }
                super::NativeVideoPlacement::FullscreenBorderless => {
                    (ExpectedMode::Borderless, owner_hwnd, true)
                }
                super::NativeVideoPlacement::DetachedViewerChild => {
                    (ExpectedMode::Child, owner_hwnd, false)
                }
                super::NativeVideoPlacement::DetachedWindow => (ExpectedMode::WindowedAt, 0, false),
            };

            let actual_mode = match super::native_window_mode_for_placement(placement, rect) {
                super::native_window::NativeVideoWindowMode::Child { rect: actual } => {
                    assert_eq!(
                        (actual.left, actual.top, actual.right, actual.bottom),
                        (rect.left, rect.top, rect.right, rect.bottom)
                    );
                    ExpectedMode::Child
                }
                super::native_window::NativeVideoWindowMode::Borderless { rect: actual } => {
                    assert_eq!(
                        (actual.left, actual.top, actual.right, actual.bottom),
                        (rect.left, rect.top, rect.right, rect.bottom)
                    );
                    ExpectedMode::Borderless
                }
                super::native_window::NativeVideoWindowMode::WindowedAt { rect: actual } => {
                    assert_eq!(
                        (actual.left, actual.top, actual.right, actual.bottom),
                        (rect.left, rect.top, rect.right, rect.bottom)
                    );
                    ExpectedMode::WindowedAt
                }
                super::native_window::NativeVideoWindowMode::Windowed { .. } => {
                    panic!("placement mapping must not select legacy Windowed mode")
                }
            };
            assert_eq!(actual_mode, expected_mode, "placement={placement:?}");
            assert_eq!(
                super::native_window_owner_for_placement(owner_hwnd, placement),
                expected_owner,
                "placement={placement:?}"
            );
            assert_eq!(
                super::native_hud_overlay_enabled_for_values(true, placement, false),
                expected_hud,
                "placement={placement:?}"
            );
            assert!(!super::native_hud_overlay_enabled_for_values(
                false, placement, false
            ));
            assert!(!super::native_hud_overlay_enabled_for_values(
                true, placement, true
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_command_bus_coalesces_hud_state_and_keeps_actions_lossless() {
        let fault = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = super::native_command_bus(8, std::sync::Arc::clone(&fault));
        tx.send(super::NativeVideoOutputCommand::SetChecked { checked: false })
            .unwrap();
        tx.send(super::NativeVideoOutputCommand::ShowToast {
            text: "first".to_string(),
            centered: false,
            linger: None,
        })
        .unwrap();
        tx.send(super::NativeVideoOutputCommand::SetChecked { checked: true })
            .unwrap();
        tx.send(super::NativeVideoOutputCommand::ShowToast {
            text: "second".to_string(),
            centered: false,
            linger: None,
        })
        .unwrap();

        let commands = rx.drain();
        assert_eq!(commands.len(), 3);
        assert!(matches!(
            commands[0],
            super::NativeVideoOutputCommand::ShowToast { ref text, .. } if text == "first"
        ));
        assert!(matches!(
            commands[1],
            super::NativeVideoOutputCommand::SetChecked { checked: true }
        ));
        assert!(matches!(
            commands[2],
            super::NativeVideoOutputCommand::ShowToast { ref text, .. } if text == "second"
        ));
        assert!(!fault.load(std::sync::atomic::Ordering::Acquire));
    }

    #[cfg(windows)]
    #[test]
    fn native_output_event_bus_coalesces_mouse_and_keeps_key_lossless() {
        let fault = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = super::native_output_event_bus(8, std::sync::Arc::clone(&fault));
        tx.send(
            9,
            super::NativeVideoOutputEvent::Window(
                crate::video::native_window::NativeVideoWindowEvent::MouseMove(
                    crate::video::native_window::NativeVideoMouseEvent {
                        x: 1,
                        y: 2,
                        shift: false,
                        ctrl: false,
                    },
                ),
            ),
        );
        tx.send(
            9,
            super::NativeVideoOutputEvent::Window(
                crate::video::native_window::NativeVideoWindowEvent::MouseMove(
                    crate::video::native_window::NativeVideoMouseEvent {
                        x: 30,
                        y: 40,
                        shift: false,
                        ctrl: false,
                    },
                ),
            ),
        );
        tx.send(
            9,
            super::NativeVideoOutputEvent::Window(
                crate::video::native_window::NativeVideoWindowEvent::KeyDown(
                    crate::video::native_window::NativeVideoKeyEvent {
                        virtual_key: 0x41,
                        scan_code: 0,
                        extended: false,
                        shift: false,
                        ctrl: false,
                        alt: false,
                        repeat: false,
                    },
                ),
            ),
        );

        let events = rx.drain();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0].1,
            super::NativeVideoOutputEvent::Window(
                crate::video::native_window::NativeVideoWindowEvent::MouseMove(mouse)
            ) if mouse.x == 30 && mouse.y == 40
        ));
        assert!(matches!(
            events[1].1,
            super::NativeVideoOutputEvent::Window(
                crate::video::native_window::NativeVideoWindowEvent::KeyDown(_)
            )
        ));
        assert!(!fault.load(std::sync::atomic::Ordering::Acquire));
    }

    #[cfg(windows)]
    #[test]
    fn native_output_consumes_overlay_routing_as_latest_observation_snapshot() {
        let (output, tx) = super::NativeVideoOutput::disconnected_for_test_with_event_sender();
        let focused = crate::video::native_presenter::NativeOverlayInputRouting {
            wants_keyboard_input: true,
            ..Default::default()
        };
        tx.send(
            1,
            super::NativeVideoOutputEvent::OverlayInputRouting(focused),
        );

        assert!(output.drain_events().is_empty());
        assert!(output.overlay_input_routing_snapshot().wants_keyboard_input);

        tx.send(
            1,
            super::NativeVideoOutputEvent::OverlayInputRouting(Default::default()),
        );
        assert!(output.drain_events().is_empty());
        assert!(!output.overlay_input_routing_snapshot().wants_keyboard_input);
    }

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

    #[test]
    fn frame_step_seek_start_uses_near_base_target_for_backward() {
        let backward = super::frame_step_seek_start_secs(10.0, 60.0, -1);
        let expected_backward = 10.0 - (1.0 / 60.0) * 1.25;
        assert!((backward - expected_backward).abs() < 1.0e-9);

        let forward = super::frame_step_seek_start_secs(10.0, 60.0, 1);
        assert!(backward > forward);
        assert!(forward >= 9.0);
    }

    #[test]
    fn frame_step_base_prefers_pending_target() {
        let base = super::frame_step_base_secs(Some(10.0), Some(20.0), 30.0);
        assert!((base - 10.0).abs() < 1.0e-9);
    }

    #[test]
    fn frame_step_base_falls_back_to_displayed_then_position() {
        let displayed = super::frame_step_base_secs(None, Some(20.0), 30.0);
        assert!((displayed - 20.0).abs() < 1.0e-9);
        let position = super::frame_step_base_secs(None, None, 30.0);
        assert!((position - 30.0).abs() < 1.0e-9);
    }

    #[test]
    fn frame_step_waits_until_issued_seek_is_displayed() {
        assert!(super::frame_step_waiting_for_display(Some(10.0), 42, 42));
        assert!(!super::frame_step_waiting_for_display(Some(10.0), 42, 43));
    }

    #[test]
    fn frame_step_wait_gate_ignores_empty_pending_target() {
        assert!(!super::frame_step_waiting_for_display(None, 42, 42));
        assert!(!super::frame_step_waiting_for_display(
            Some(10.0),
            super::FRAME_STEP_NO_PENDING_SEQ,
            42
        ));
    }

    #[test]
    fn user_seek_coalesce_waits_while_seek_has_not_displayed() {
        let now = std::time::Instant::now();
        let state = super::UserSeekCoalesceState {
            pending_target_secs: Some(12.0),
            last_issued_at: Some(now),
            last_issued_display_seq: 7,
        };
        assert!(!super::user_seek_ready_to_issue(&state, true, 7, now));
    }

    #[test]
    fn user_seek_coalesce_allows_next_after_first_frame_or_timeout() {
        let now = std::time::Instant::now();
        let state = super::UserSeekCoalesceState {
            pending_target_secs: Some(12.0),
            last_issued_at: Some(now - super::USER_SEEK_REISSUE_AFTER),
            last_issued_display_seq: 7,
        };
        assert!(super::user_seek_ready_to_issue(&state, true, 8, now));
        assert!(super::user_seek_ready_to_issue(&state, true, 7, now));
    }

    #[test]
    fn remote_streaming_seek_never_defers_the_seek_serial() {
        let player =
            super::VideoPlayer::disconnected_for_test(std::path::PathBuf::from("fixture.mp4"), 0.0);
        let initial = player.current_seek_serial();
        player.seek_for_remote_streaming(10.0);
        let first = player.current_seek_serial();
        player.seek_for_remote_streaming(20.0);
        let second = player.current_seek_serial();
        assert!(first > initial);
        assert!(second > first);
    }
}
