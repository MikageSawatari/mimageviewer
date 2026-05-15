use std::collections::{HashMap, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG,
    D3D11_FENCE_FLAG_NONE, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11Device1,
    ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext1, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11RenderTargetView, ID3D11Resource, ID3D11Texture2D, ID3D11View,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGIKeyedMutex, IDXGIOutput,
    IDXGISwapChain1, IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::{
    CANDIDATEFORM, CFS_EXCLUDE, CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext,
    ImmSetCandidateWindow, ImmSetCompositionWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDC_ARROW, IDC_HAND, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENS, IDC_SIZEWE, IDC_WAIT,
    LoadCursorW, SetCursor,
};
use windows::core::Interface;
use windows_numerics::Matrix3x2;

use crate::video::decoder::{VideoFrame, VideoFrameData};

mod overlay_draw;
use self::overlay_draw::*;

pub mod hud_window;

/// `shared_texture_cache` (presenter 側 `OpenSharedResource1` キャッシュ) の上限。
///
/// 2026-05-15 (Codex 助言): 旧 64 → 8 に縮小、SwitchSource ハンドラで `.clear()` も
/// 追加。cap=64 は `OpenSharedResource1` の再呼び出しを減らすためだが、各エントリは
/// D3D11 共有 texture を保持して adapter memory を圧迫していた (4K で 32 MB/枚)。
/// 単一動画の通常再生では 1-3 個の slot が周回するのみで cap=8 で十分。動画切替
/// (SwitchSource) ごとに clear して旧動画分のキャッシュを残さない。
const SHARED_TEXTURE_CACHE_CAPACITY: usize = 8;

/// パネル / HUD が全て隠れた状態でこの秒数経過したらマウスカーソルを非表示にする。
/// `ui_fullscreen` (eframe) と `native_presenter` (D3D11 + 別 egui_ctx) で共有する。
/// lib にいる native_presenter から bin の `ui_fullscreen` を参照できないため、
/// シングルソースをここ (lib 側) に置く。
pub const CURSOR_HIDE_IDLE_SECS: f32 = 3.0;
const SEEK_STATUS_DELAY: Duration = Duration::from_millis(150);
const SEEK_STATUS_MIN_VISIBLE: Duration = Duration::from_millis(300);
const LIMITER_INDICATOR_VISIBLE: Duration = Duration::from_millis(500);

fn seek_status_visible_for_times(
    is_seeking: bool,
    started_at: Option<Instant>,
    visible_since: Option<Instant>,
    now: Instant,
) -> bool {
    let active_after_delay = is_seeking
        && started_at.is_some_and(|started| now.duration_since(started) >= SEEK_STATUS_DELAY);
    let held_after_completion =
        visible_since.is_some_and(|shown| now.duration_since(shown) < SEEK_STATUS_MIN_VISIBLE);
    active_after_delay || held_after_completion
}

fn format_video_volume_db_compact(volume: f64) -> String {
    let db = crate::settings::video_volume_linear_to_db(volume);
    if db <= crate::settings::VIDEO_VOLUME_MUTE_DB + 0.05 {
        "-∞dB".to_string()
    } else if db.abs() < 0.05 {
        "0.0dB".to_string()
    } else {
        format!("{db:+.1}dB")
    }
}

#[derive(Clone, Copy, Debug)]
struct CenterPauseControlsInputs {
    tile_overlay_visible: bool,
    is_playing: bool,
    frame_step_active: bool,
    initial_pause_controls_pending: bool,
    first_frame_presented: bool,
    has_video_error: bool,
    normalize_scanning: bool,
}

fn center_pause_controls_visible_for_state(inputs: CenterPauseControlsInputs) -> bool {
    !inputs.tile_overlay_visible
        && !inputs.is_playing
        && !inputs.frame_step_active
        && inputs.initial_pause_controls_pending
        && inputs.first_frame_presented
        && !inputs.has_video_error
        && !inputs.normalize_scanning
}

fn center_seek_status_visible(raw_seek_status_visible: bool, paused_center_visible: bool) -> bool {
    raw_seek_status_visible && !paused_center_visible
}

pub struct NativePresenterConfig {
    pub hwnd: HWND,
    pub width: u32,
    pub height: u32,
    pub test_overlay: bool,
    pub egui_overlay: bool,
    /// HUD overlay HWND の wndproc が拾った mouse / DPI / raise 要求を流す sender。
    /// `Some` のとき、presenter は HUD overlay HWND を作って egui overlay を
    /// その DComp tree にぶら下げる (CP4 反映)。`None` または HUD HWND 作成失敗時は
    /// 従来通り presenter HWND の DComp tree に egui overlay をぶら下げるフォールバック経路。
    pub hud_event_tx:
        Option<std::sync::mpsc::Sender<crate::video::native_window::NativeVideoWindowEvent>>,
}

pub struct NativeVideoPresenter {
    swap_chain: IDXGISwapChain1,
    waitable: HANDLE,
    d3d_device1: ID3D11Device1,
    d3d_device5: ID3D11Device5,
    d3d_context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    d3d_context1: ID3D11DeviceContext1,
    d3d_context4: ID3D11DeviceContext4,
    _dcomp_device: IDCompositionDevice,
    _dcomp_target: IDCompositionTarget,
    _root_visual: IDCompositionVisual,
    _background: NativeBlackBackground,
    _video_visual: IDCompositionVisual,
    backbuffer: Option<ID3D11Texture2D>,
    test_overlay: Option<NativeTestOverlay>,
    egui_overlay: Option<NativeEguiOverlay>,
    /// HUD overlay HWND (= bars / interactive UI 用の独立 top-level)。
    /// `NativePresenterConfig.hud_event_tx` が `Some` のときに `HudOverlayWindow::create` で
    /// 作成される。HUD HWND の DComp tree に egui overlay visual が乗る (CP4 反映)。
    /// 作成失敗時は `None` で従来通り presenter HWND の DComp tree に egui overlay を載せる
    /// フォールバック経路に入る。
    hud_window: Option<hud_window::HudOverlayWindow>,
    /// HUD HWND 用の独立 `IDCompositionTarget`。`hud_window` と一緒に保持する。
    /// **drop されると DComp tree が解除される**ので、`hud_window` と同じ寿命で必ず保持する
    /// (Codex プラン Step 2 / `_dcomp_target` パターン)。
    _hud_dcomp_target: Option<IDCompositionTarget>,
    /// HUD HWND 用の DComp root visual。`_hud_dcomp_target` と同じく drop 防止のため保持。
    _hud_root_visual: Option<IDCompositionVisual>,
    /// HUD `WM_NCHITTEST` フェイルセーフ用に `regions` を共有。CP5 で
    /// `NativeEguiOverlay::run` 末尾から書き込む。CP4 段階では初期値 (= 空 `Vec<RECT>`) のまま。
    hud_regions: Option<std::sync::Arc<std::sync::Mutex<hud_window::HudInteractiveRegions>>>,
    /// CP6: HUD raise の allowlist 判定 (`foreground_allows_hud_raise`) で参照する
    /// VST editor container HWND の snapshot。CP7 で App が `set_editor_hwnds_snapshot`
    /// で `dsp_bridge.editor_hwnds_snapshot()` を渡す。`None` のとき raise 判定は false 固定
    /// (= raise burst を起動しない)。
    editor_hwnds_snapshot:
        Option<std::sync::Arc<std::sync::RwLock<std::collections::HashSet<u64>>>>,
    /// CP6: `foreground_allows_hud_raise` 判定用の main HWND (mIV メインウィンドウ)。
    /// CP7 で App が `set_main_hwnd_for_raise_check` で設定する。0 なら未登録扱い。
    main_hwnd_for_raise: u64,
    /// CP9 実機 debug: 直近 log した region hash。`MIV_HUD_DEBUG=1` のとき
    /// region 変化時に 1 回だけログ出力するための重複抑制用。
    last_logged_region_hash: Option<u64>,
    /// 実機修正 (2026-05-12 P1 #3): 直近 LBUTTON down が検出された時刻。
    /// external_drag 判定に 100ms の delay を入れることで「short click」を
    /// drag と誤検出して top bar を hide させるバグを防ぐ。
    lbutton_down_since: Option<Instant>,
    fence_cache: Option<(u64, isize, ID3D11Fence)>,
    /// presenter 自前の D3D11 fence。`copy_frame_into_backbuffer` のフレームコピー後に
    /// `Signal` して値を進める。`run_native_video_output` 側の `present_retire` は
    /// `copy_fence_completed_value()` を見て、コピーが GPU 上で完了したフレームだけを
    /// 解放する (= 共有出力 slot を「presenter のコピー完了後」に返す保証)。
    /// fence 作成に失敗した環境では `None` で、`present_retire` は時間ベースの depth
    /// キャップにフォールバックする。
    copy_fence: Option<ID3D11Fence>,
    /// `copy_fence` に次に `Signal` する値 (1, 2, 3, ... と単調増加)。
    copy_fence_value: u64,
    /// 開いた共有出力テクスチャのキャッシュ。キーは `(NT shared handle 値, shared_texture_gen)`。
    /// `gen` を含める理由は `open_shared_texture` のドキュメントコメント参照
    /// (= handle 値再利用による前動画フレーム混入の防止)。
    shared_texture_cache: Vec<((u64, u64), ID3D11Texture2D)>,
    cpu_upload_scratch: Vec<u8>,
    pixel_probe_enabled: bool,
    pixel_probe_strict: bool,
    last_pixel_probe: Option<Instant>,
    video_compact: bool,
    /// Sample aspect ratio (= pixel aspect ratio)。1/1 = 正方ピクセル (= 従来挙動)。
    /// アナモフィック動画 (NTSC DVD 等で SAR=97/80 など) で `update_video_visual_transform`
    /// の M11/M22 を anisotropic にして表示比を補正する。decoder の VideoInfo 経由で
    /// `set_video_sar(num, den)` で設定される。
    sar_num: u32,
    sar_den: u32,
    width: u32,
    height: u32,
    surface_width: u32,
    surface_height: u32,
    /// 動画フレームの解像度が変わったとき、新しい swap chain を作るための DXGI factory。
    /// `new()` 生成時のものを保持する。
    factory: IDXGIFactory2,
    /// 解像度変更で差し替えた旧 video swap chain を遅延破棄するためのキュー。
    /// `SetContent` で新 swap chain に切り替えた後も DComp / DWM 側がしばらく旧 content を
    /// 参照しうるため、即 drop せず数世代分保持する (Codex 助言)。
    retired_video_surfaces: VecDeque<RetiredVideoSurface>,
}

/// 解像度変更で差し替えた旧 video swap chain。`retired_video_surfaces` で遅延保持し、
/// キューから押し出されたタイミングで Drop される (= swap chain + waitable を解放)。
struct RetiredVideoSurface {
    _swap_chain: IDXGISwapChain1,
    waitable: HANDLE,
    _backbuffer: Option<ID3D11Texture2D>,
}

impl Drop for RetiredVideoSurface {
    fn drop(&mut self) {
        if !self.waitable.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.waitable);
            }
        }
    }
}

/// 旧 video swap chain を保持する世代数。`SetContent` 切替後、DComp/DWM が旧 content を
/// 参照しなくなるまでの猶予 + presenter の非同期 GPU コピー完了の猶予を兼ねる。
///
/// 2026-05-15 (Codex 助言): 旧 3 → 1 に縮小。swap chain は `BufferCount=3` なので
/// 4K で 1 個約 95 MB。depth=3 だと active + retired 4 個で 4K 単独 ~380 MB の
/// adapter memory を占有していた。fast-swap で前動画分も加わると数 GB 級になり
/// `wgpu Out of Memory` panic に直結。depth=1 でも次フレームまで旧 surface は
/// 保持されるので「片チャンネル切替の瞬間に旧 surface が GPU 上に必要」要件を
/// 満たす (`SetContent` の Commit が DComp に届くまで 1 frame 程度のラグ)。
const RETIRED_VIDEO_SURFACE_DEPTH: usize = 1;

struct NativeBlackBackground {
    swap_chain: IDXGISwapChain1,
    _visual: IDCompositionVisual,
    backbuffer: Option<ID3D11Texture2D>,
    render_target: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
}

struct NativeTestOverlay {
    swap_chain: IDXGISwapChain1,
    _visual: IDCompositionVisual,
    backbuffer: Option<ID3D11Texture2D>,
    render_target: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
}

struct NativeEguiOverlay {
    surface: wgpu::Surface<'static>,
    visual: IDCompositionVisual,
    dcomp_device: IDCompositionDevice,
    root_visual: IDCompositionVisual,
    /// この visual を root にぶら下げる際に「どの sibling の後ろに配置するか」。
    /// `Some(v)` なら presenter フォールバック経路で `video_visual` の後ろに挟む。
    /// `None` なら HUD HWND の DComp root に単独で配置する (CP3 P1 #3 反映)。
    after_visual: Option<IDCompositionVisual>,
    _instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    present_mode: wgpu::PresentMode,
    alpha_mode: wgpu::CompositeAlphaMode,
    renderer: egui_wgpu::Renderer,
    egui_ctx: egui::Context,
    /// DComp / DPI / wgpu surface 用の HWND。CP4 以降、HUD 経路では HUD overlay HWND、
    /// presenter フォールバック経路では presenter HWND を渡す。
    /// CP8 で `WM_DPICHANGED` 経由の DPI 更新 (= `GetDpiForWindow(dcomp_hwnd)` 再計算)
    /// で参照する予定。CP3 時点ではコンストラクタの初期 `pixels_per_point` 計算のみで使い、
    /// その後は dead code。
    #[allow(dead_code)]
    dcomp_hwnd: HWND,
    /// IME context lookup / focus handoff の対象 HWND。常に presenter HWND。
    /// HUD HWND は `WS_EX_NOACTIVATE` で focus を取らないため、IME context を引くと
    /// 入力が動かない → 必ず presenter HWND を使う (Codex プラン P1 #2 反映)。
    focus_hwnd: HWND,
    started_at: Instant,
    pending_events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    pointer_pos: Option<egui::Pos2>,
    event_count: u64,
    dirty: bool,
    next_repaint_deadline: Option<Instant>,
    wants_pointer_input: bool,
    wants_keyboard_input: bool,
    video_position_secs: f64,
    video_duration_secs: f64,
    video_is_playing: bool,
    video_volume: f64,
    video_muted: bool,
    video_limiter_ceiling_hit_seq: u64,
    video_limiter_visible_until: Option<Instant>,
    video_playback_speed: f64,
    video_frame_step_active: bool,
    initial_pause_controls_pending: bool,
    video_is_seeking: bool,
    video_seek_serial: u64,
    seek_status_started_at: Option<Instant>,
    seek_status_visible_since: Option<Instant>,
    seek_status_visible: bool,
    video_speed_popup_open: bool,
    frame_step_hold: Option<NativeFrameStepHold>,
    video_loop_enabled: bool,
    /// HUD ボタン表示用のループモード (= ユーザー設定の display_mode)。
    /// 「BM 設定 + BM 無し動画」のとき、`video_loop_enabled` は effective から導出した
    /// bool が入るが、`video_loop_mode` は表示用に Bookmark のまま維持する。
    video_loop_mode: crate::settings::VideoLoopMode,
    video_checked: bool,
    vst3_available: bool,
    vst3_panel: Option<NativeOverlayVst3Panel>,
    first_frame_presented: bool,
    video_error: Option<String>,
    /// 動画オープン中の進捗 (phase / bytes_read / file_size)。
    /// `first_frame_presented = false` の間だけ center status HUD に出る。
    preparing_status: crate::video::avio_progress::PreparingStatus,
    toast: Option<NativeOverlayToast>,
    perf_visible: bool,
    perf_history: VecDeque<NativeOverlayPerfSample>,
    perf_latest: NativeOverlayPerfSnapshot,
    perf_last_dirty: Instant,
    perf_pause_gap_pending: bool,
    last_seek_target_secs: Option<f64>,
    last_thumbnail_request_secs: Option<f64>,
    last_thumbnail_request_at: Option<Instant>,
    hover_preview_target_secs: Option<f64>,
    hover_preview_pinned: bool,
    /// 実機修正 (2026-05-12 P1 #2): 直近 egui run で描画した preview rect (= サムネイル枠)。
    /// `compute_hud_regions` が region 計算で参照する。region を cursor x 追従で再計算すると
    /// 描画 rect (= `target_secs` 起点) とずれて「サムネ画像は固定なのに枠だけ動く」症状になる。
    /// `None` ならサムネ非表示。
    last_drawn_preview_rect: Option<egui::Rect>,
    /// 実機修正 (2026-05-12 A): 直近 egui run で描画した VST3 設定パネルの actual rect。
    /// パネルは `egui::Area::movable(true)` でドラッグ可能なので、デフォルト位置 (=
    /// `native_vst3_panel_rect`) からドラッグでずれた場合、region をその実位置に追従させる。
    /// `None` ならパネル非表示。
    last_drawn_vst3_panel_rect: Option<egui::Rect>,
    /// Off-screen clamp 復旧などで同じ VST3 panel 位置 command を複数フレーム連続発行しないための
    /// presenter-local 記憶。UI thread から新しい panel_pos snapshot が届いたらリセットする。
    last_emitted_vst3_panel_pos: Option<[f32; 2]>,
    /// 実機修正 (2026-05-12 P2): 直近 egui run で描画した paused center 制御 UI の
    /// 個別 rect 群 (`[replay_rect, play_rect, backdrop_rect]` の順)。
    /// `compute_hud_regions` が region 計算で読む (= 200×200pt 固定だと両ボタンの
    /// 外側 ~29pt とヒント帯が SetWindowRgn でクリップされる症状の修正)。
    /// union で 1 矩形にまとめず個別 rect で push することで、ヒント横幅に引っ張られた
    /// 「ボタン上部左右の不要な HUD 入力領域」を作らず VST 入力干渉を最小化する。
    /// `None` なら paused center 非表示。
    last_drawn_paused_center_rects: Option<[egui::Rect; 3]>,
    /// 直近 egui run で描画した toast の actual rect。
    /// HUD HWND は `SetWindowRgn` で領域外が物理的に clip されるため、toast も
    /// region に含めないと他 UI の region に重なった部分だけが見える。
    last_drawn_toast_rect: Option<egui::Rect>,
    /// 直近 egui run で描画した playback speed popup の actual rect。
    /// speed ボタンは下 HUD 右側にあるため、中央固定の概算 region だと popup が
    /// SetWindowRgn 外に落ちて見えたり、click hit-test が不安定になったりする。
    /// `None` なら popup 非表示または未描画。
    last_drawn_speed_popup_rect: Option<egui::Rect>,
    /// 直近 egui run で描画したブックマーク名編集ダイアログの actual rect。
    /// 中央モーダルだが、配置は `pos.y = (H - dialog_h) * 0.5` (dialog_h はレイアウト
    /// 用の過大見積もり) で、実コンテンツ高さとの差でダイアログ中心が画面中心より
    /// 上にずれる。画面中心固定の概算 region だと上端 (=「ブックマーク名」ラベル) が
    /// SetWindowRgn でクリップされるため、実描画 rect を region に使う。
    /// `None` ならダイアログ非表示または未描画。
    last_drawn_bookmark_editor_rect: Option<egui::Rect>,
    hover_thumbnail: Option<NativeOverlayThumbnail>,
    hover_texture: Option<egui::TextureHandle>,
    hover_texture_key: Option<(u32, u32, u64)>,
    timeline_markers: Vec<NativeOverlayTimelineMarker>,
    jump_entries: Vec<NativeOverlayJumpEntry>,
    bookmark_title_edit: Option<NativeBookmarkTitleEdit>,
    video_metadata: Option<NativeOverlayMetadata>,
    tile_overlay: Option<NativeOverlayTileOverlay>,
    tile_textures: HashMap<usize, (u64, egui::TextureHandle)>,
    jump_textures: HashMap<usize, (u64, egui::TextureHandle)>,
    top_bar_visible: bool,
    right_panel_visible: bool,
    jump_panel_visible: bool,
    /// 実機修正 (2026-05-12): 外部 drag (= HUD region 外で left button down 中、典型的には VST window
    /// のドラッグ) を検出するフラグ。`NativeVideoPresenter::cursor_polling_tick` で `GetAsyncKeyState
    /// (VK_LBUTTON)` の結果と egui の `pointer.any_down()` の差から判定して set する。
    /// true の間、`top_bar_visible()` / `hud_visible()` / `right_panel_visible()` 等の hover 判定を
    /// 強制 false にして、bar / panel が出ないようにする (= VST 上端帯にドラッグしても hover 表示で
    /// VST 入力が奪われないようにする)。
    external_drag_in_progress: bool,
    pending_overlay_commands: Vec<NativeOverlayCommand>,
    last_volume_target: Option<f64>,
    visual_attached: bool,
    pixels_per_point: f32,
    width: u32,
    height: u32,
    /// 最終ユーザー活動時刻 / overlay 表示時刻のうち最新のもの。
    /// `!overlay_visible` 期間中、ここから `CURSOR_HIDE_IDLE_SECS` 経過したら
    /// `SetCursor(None)` でカーソルを隠す。更新タイミング:
    /// - `push_native_event` (mouse / key 等の native event): `Some(now)` にリセット。
    /// - `mark_cursor_activity` (eframe 経由のキー入力反映): `Some(now)` にリセット。
    /// - `render_once` で `overlay_visible == true` のフレーム: 毎回 `Some(now)` で再 bump
    ///   (= overlay が見えている間は countdown を 0 にし続け、消えた瞬間から 3 秒測る)。
    /// `None` は初期状態のみ (フルスクリーン入場直後で初回 render 前)。
    cursor_last_activity: Option<Instant>,
    /// 直前 render で `SetCursor(None)` を打った sticky フラグ。次の活動 / overlay 表示
    /// が起きるまで true を維持し、`wants_periodic_tick()` が false を返して以降の
    /// 余計な tick を止める。3 秒経過判定後に確実に 1 回 `SetCursor(None)` を打つために
    /// 「!cursor_hidden の間は tick 継続」という形で利用する。
    cursor_hidden: bool,
    /// 実機修正 (2026-05-12 Codex P2 #6): cursor が `SetCursor(None)` で非表示にされたか
    /// (= `cursor_hidden` と同じ値) を **HUD wndproc から読める形で共有する atomic**。
    /// `update_cursor_icon` で書き込み、`WM_SETCURSOR` が読み出して隠れた cursor を復帰させる。
    cursor_was_hidden_shared: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 音量ノーマライズ UI 状態 (App から `SetNormalizeOverlayState` で配信される)。
    normalize_state: crate::video::normalize_types::NormalizeOverlayState,
}

#[derive(Clone, Copy, Debug)]
struct NativeFrameStepHold {
    direction: i32,
    last_step_at: Instant,
}

#[derive(Clone, Debug)]
struct NativeBookmarkTitleEdit {
    id: i64,
    title: String,
    request_focus: bool,
}

#[derive(Clone, Debug)]
struct NativeOverlayToast {
    text: String,
    started_at: Instant,
    centered: bool,
    /// このトーストを表示し続ける時間。`started_at` からこれを過ぎたら消す。
    /// `show_toast` で `linger` 指定があればその値、無ければ `centered` から導いた
    /// 既定値 (centered: 2.5s / それ以外: 1.8s) が入る。
    linger: Duration,
}

pub struct NativePresentOutcome {
    pub path: &'static str,
    pub shared_handle: u64,
    pub shared_cache_hit: bool,
    /// この present で参照した共有出力テクスチャの世代 ID (GPU 経路のみ、CPU 経路は 0)。
    pub shared_texture_gen: u64,
    /// この present で参照した GPU frame の fence value (GPU 経路のみ、CPU 経路は 0)。
    pub fence_value: u64,
    /// このフレームコピーの GPU 完了に対応する presenter copy fence の値。
    /// `present_retire` がこの値の到達を見てフレームを解放する。fence 未作成時は 0。
    pub copy_fence_value: u64,
    /// この present 完了後の video swap chain サーフェスサイズ。
    pub surface_width: u32,
    pub surface_height: u32,
    /// この present でフレーム実寸 / SAR が変わり transform を更新したか。
    pub geometry_changed: bool,
    /// この present で解像度変更により video swap chain を原子的に差し替えたか。
    pub surface_swapped: bool,
    /// 差し替え時の `WaitForCommitCompletion` 待ち時間 (ms)。差し替えが無ければ 0。
    pub commit_sync_ms: f64,
    pub wait_ms: f64,
    pub wait_timed_out: bool,
    pub fence_wait_ms: f64,
    pub open_shared_ms: f64,
    pub keyed_mutex_ms: f64,
    pub keyed_mutex_cast_ms: f64,
    pub keyed_mutex_acquire_ms: f64,
    pub copy_call_ms: f64,
    pub copy_ms: f64,
    pub present_waitable_ms: f64,
    pub present_call_ms: f64,
    pub present_ms: f64,
}

/// `copy_frame_into_backbuffer` の戻り値。GPU / CPU いずれの経路でフレームを
/// backbuffer へコピーしたかと、その計測値をまとめる。
struct FrameCopyMetrics {
    path: &'static str,
    shared_handle: u64,
    shared_cache_hit: bool,
    shared_texture_gen: u64,
    fence_value: u64,
    fence_wait_ms: f64,
    open_shared_ms: f64,
    keyed_mutex_ms: f64,
    keyed_mutex_cast_ms: f64,
    keyed_mutex_acquire_ms: f64,
    copy_call_ms: f64,
    /// このコピーの GPU 完了に対応する presenter copy fence の値。`present_retire` は
    /// `copy_fence_completed_value() >= この値` になったフレームだけを解放する。
    /// fence 未作成時は 0 (= ゲートに使わない、depth キャップのみ)。
    copy_fence_value: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeOverlayPerfSnapshot {
    pub elapsed_secs: f64,
    pub presented: u64,
    pub gpu: u64,
    pub cpu: u64,
    pub late_drop: u64,
    pub wait_timeout: u64,
    pub actual_fps: f64,
    pub max_late_ms: f64,
    pub max_total_ms: f64,
    pub max_interval_ms: f64,
    /// **video pacing health 指標** (= video_pts − master_clock、ms 単位、符号付き)。
    /// 通常 ≈ 0。これだけでは Norm 経路バグなど audio が clock から乖離した場合の
    /// 体感ズレを検出できないので、ユーザー表示は `av_offset_ms` を主にする。
    pub av_drift_ms: f32,
    /// **ユーザー体感の音映像差** (= video_pts − audio_audible_pts、ms、符号付き)。
    /// + = 映像が音声より進んでいる、− = 映像が音声より遅れている。
    /// audio inactive (動画 only / 音声起動失敗) または seek 直後など offset 未確定時は
    /// `f32::NAN`。audio の有無は `audio_active` を見る。
    pub av_offset_ms: f32,
    /// audio stream が clock source として active か。`av_offset_ms` は seek 直後に一時
    /// NaN になるので、lead / underrun の表示可否はこの値で判定する。
    pub audio_active: bool,
    /// audio が master clock より何 ms 先行しているか (callback 直近値)。
    /// 通常 ≈ 0、Norm clear 後の big jump 直後は 5000+ ms に張り付くことがある。
    pub audio_lead_ms: f32,
    /// callback で silence を出力中か (= cpal underrun 中)。pump 復活で false に戻る。
    pub audio_underrun_active: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeOverlayPerfSample {
    pub arrival: Instant,
    pub interval_ms: f32,
    pub total_ms: f32,
    pub copy_ms: f32,
    pub present_waitable_ms: f32,
    pub present_call_ms: f32,
    pub late_ms: f32,
    /// 本 sample の直前選択で表示前に捨てた frame 数。
    /// perf graph の赤縦線は frame interval の揺れではなく、この値だけを marker にする。
    pub late_drop_delta: u32,
    /// PTS delta between consecutive presented frames (= 1/native_fps の生値)。
    /// 再生速度の影響を受けない (= 30fps なら常に 33.33ms)。Y 軸 / gap 判定には
    /// `playback_speed` で割って実 frame interval に正規化したものを使う。
    pub source_delta_ms: f32,
    /// この sample が記録された時点の再生速度倍率 (= 0.25..=4.0)。
    /// 0.5x なら実 frame interval は `source_delta_ms / 0.5 = 2 * source_delta_ms`。
    pub playback_speed: f32,
    /// 本 sample 取得時点の video pacing drift (signed ms、video_pts − master_clock)。
    pub av_drift_ms: f32,
    /// 本 sample 取得時点の **体感音映像差** (signed ms、video_pts − audio_audible_pts)。
    /// audio inactive または seek 直後など offset 未確定時は `f32::NAN`。
    /// グラフのサブトラック描画はこちらを優先。
    pub av_offset_ms: f32,
    /// 本 sample 取得時点で audio stream が active だったか。
    pub audio_active: bool,
    /// 本 sample 取得時点で audio が master clock から先行している量 (ms)。
    /// 通常 ≈ 0、Norm clear 直後は >>0 に張り付く。
    pub audio_lead_ms: f32,
    /// 本 sample 取得時点で audio underrun 中だったか (橙背景帯の描画用)。
    pub audio_underrun_active: bool,
}

#[derive(Clone, Debug)]
pub struct NativeOverlayThumbnail {
    pub target_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOverlayTimelineMarkerKind {
    Pin,
    Bookmark,
    Chapter,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeOverlayTimelineMarker {
    pub pts_secs: f64,
    pub kind: NativeOverlayTimelineMarkerKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayMetadata {
    pub file_name: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub original_url: Option<String>,
    pub description: Option<String>,
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub video_codec: String,
    pub video_decoder: String,
    pub audio_codec: Option<String>,
    pub avg_fps: f64,
    pub bit_rate_bps: i64,
    pub chapter_count: usize,
    pub hw_decode_active: bool,
    pub gpu_path_active: bool,
    pub d3d11va_supported: bool,
    /// open 時に確定したデインターレースモード (Auto/On/Off)。
    pub deinterlace_mode: crate::settings::VideoDeinterlaceMode,
    /// 直近フレームのプレゼン経路 (動的)。
    pub last_present_path: crate::video::decoder::PresentPathSnapshot,
    /// bwdif フィルタの現在状態 (動的)。
    pub deinterlace_status: crate::video::decoder::DeinterlaceStatusSnapshot,
    /// 再生中に一度でもインターレースが検出されたか (latched、動的)。
    pub interlace_detected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayVst3Panel {
    pub visible: bool,
    pub video_compact: bool,
    pub panel_pos: Option<[f32; 2]>,
    pub state_text: String,
    pub disabled_reason: Option<String>,
    pub slots: Vec<NativeOverlayVst3Slot>,
    pub chain_slots: Vec<NativeOverlayVst3ChainSlot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayVst3Slot {
    pub idx: usize,
    pub path: String,
    pub name: String,
    pub state: NativeOverlayVst3SlotState,
    pub bypass: bool,
    pub gui_visible: bool,
    pub latency_ms: Option<f64>,
    pub auto_bypassed_for_latency: bool,
    pub placeholder: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOverlayVst3SlotState {
    Loading,
    Loaded,
    Error,
    Placeholder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayVst3ChainSlot {
    pub idx: usize,
    pub key_label: String,
    pub name: Option<String>,
    pub plugin_count: usize,
}

#[derive(Clone)]
pub struct NativeOverlayTileThumbnail {
    pub target_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone)]
pub struct NativeOverlayTileOverlay {
    pub interval_secs: f64,
    pub timestamps: Vec<f64>,
    pub tile_w: u32,
    pub tile_h: u32,
    pub columns: usize,
    pub progress_done: usize,
    pub progress_total: usize,
    pub finished: bool,
    pub tiles: Vec<Option<NativeOverlayTileThumbnail>>,
    // ホイールで動画を切り替えた直後など metadata が None の数フレームでも、
    // 上部バーのタイトル行にファイル名を出すための fallback。
    pub fallback_file_name: String,
    // S タイル表示中の動画→動画 source swap では通常の center status HUD を
    // 描画しないため、動画オープン中の AVIO 進捗をタイル overlay 側で持つ。
    pub video_open_status: Option<crate::video::avio_progress::PreparingStatus>,
}

impl NativeOverlayTileOverlay {
    pub fn preparing() -> Self {
        Self::preparing_with_filename(String::new())
    }

    pub fn preparing_with_filename(file_name: String) -> Self {
        Self::preparing_with_open_status(
            file_name,
            crate::video::avio_progress::PreparingStatus {
                phase: crate::video::avio_progress::prep_phase::OPENING,
                bytes_read: 0,
                file_size: 0,
            },
        )
    }

    pub fn preparing_with_open_status(
        file_name: String,
        open_status: crate::video::avio_progress::PreparingStatus,
    ) -> Self {
        Self {
            interval_secs: 0.0,
            timestamps: Vec::new(),
            tile_w: 160,
            tile_h: 90,
            columns: 1,
            progress_done: 0,
            progress_total: 0,
            finished: false,
            tiles: Vec::new(),
            fallback_file_name: file_name,
            video_open_status: Some(open_status),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NativeOverlayJumpEntry {
    pub pts_secs: f64,
    pub kind: NativeOverlayTimelineMarkerKind,
    pub title: Option<String>,
    pub bookmark_id: Option<i64>,
    pub thumbnail: Option<NativeOverlayThumbnail>,
}

#[derive(Clone, Copy, Debug)]
struct NativePixelSample {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    format: i32,
    b: u8,
    g: u8,
    r: u8,
    a: u8,
}

struct SourceKeyedMutexAcquire {
    guard: Option<KeyedMutexReadGuard>,
    cast_ms: f64,
    acquire_ms: f64,
}

struct KeyedMutexReadGuard {
    mutex: IDXGIKeyedMutex,
    released_to_reader: Option<Arc<AtomicBool>>,
}

impl Drop for KeyedMutexReadGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mutex.ReleaseSync(0);
        }
        if let Some(released) = &self.released_to_reader {
            released.store(false, Ordering::Release);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeOverlayInputRouting {
    pub wants_pointer_input: bool,
    pub wants_keyboard_input: bool,
    pub text_input_active: bool,
    /// この egui パスが wheel イベントを `WheelNavigate` / `TileColumnsDelta` コマンドへ
    /// 変換したか。true のとき同じ raw wheel イベントを `Window(MouseWheel)` として App へ
    /// 二重転送しない (= overlay コマンドと App 側 wheel ハンドラの二重適用を防ぐ)。
    /// タイルグリッドが `Order::Background` だと grid の余白上で egui の
    /// `wants_pointer_input()` が false になり、これが無いと Ctrl+ホイールでの列数変更が
    /// 2 ステップ進んでしまう。
    pub consumed_wheel: bool,
}

pub struct NativeOverlayInputOutcome {
    pub routing: NativeOverlayInputRouting,
    pub commands: Vec<NativeOverlayCommand>,
    /// CP5 で計算した HUD interactive regions (= 物理ピクセル単位 RECT 集合)。
    /// 表示中の bar / panel / popup / hover thumbnail などの矩形を含む。
    /// activation zone は **含めない** (= 上下端の VST 入力を奪わないため、Codex 5 P1 #1)。
    /// `NativeVideoPresenter::handle_window_events` の戻り値受信側で
    /// `apply_hud_regions(&hud_regions)` を呼んで `SetWindowRgn` を更新する。
    pub hud_regions: Vec<RECT>,
}

impl NativeOverlayInputOutcome {
    fn empty() -> Self {
        Self {
            routing: NativeOverlayInputRouting::default(),
            commands: Vec::new(),
            hud_regions: Vec::new(),
        }
    }
}

fn native_video_fullscreen_shortcut_key(
    key: &crate::video::native_window::NativeVideoKeyEvent,
) -> bool {
    if key.alt {
        return false;
    }
    matches!(
        key.virtual_key,
        0x0D // Enter
            | 0x1B // Escape
            | 0x20 // Space
            | 0x23 // End
            | 0x24 // Home
            | 0x25 // Left
            | 0x26 // Up
            | 0x27 // Right
            | 0x28 // Down
            | 0x42 // B
            | 0x46 // F (perf overlay toggle、v0.9.x で旧 P から移動)
            | 0x4A // J
            | 0x4B // K
            | 0x4C // L
            | 0x4D // M
            | 0x50 // P (pin current frame、v0.9.x で perf から再割り当て)
            | 0x53 // S
            | 0x57 // W
            | 0xA6 // VK_BROWSER_BACK — マウス戻るボタン (driver / AHK 経由)。
                   // overlay が wants_keyboard_input でも fullscreen ショートカットとして
                   // App ハンドラへ流す (= マウスナビゲーションが overlay text input 等で
                   // 殺されないようにする、Codex P2)。
            | 0xA7 // VK_BROWSER_FORWARD — 同上。
    )
}

#[derive(Clone, Debug)]
pub enum NativeOverlayCommand {
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
    SeekToStartAndPlay,
    TogglePlay,
    ToggleMute,
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
    ToggleLoop,
    AddBookmarkAt {
        target_secs: f64,
    },
    SetPinAt {
        target_secs: f64,
    },
    SetBookmarkTitle {
        id: i64,
        title: String,
    },
    DeleteBookmark {
        id: i64,
    },
    DeletePin,
    OpenExternalUrl {
        url: String,
    },
    /// 音量ノーマライズボタンの左クリック (3 状態モデルでトグル動作)。
    ToggleNormalize,
    /// 音量ノーマライズボタンの右クリック (どの状態からでもグローバル OFF 化)。
    DisableNormalize,
    /// スキャン中の進捗パネル × / ESC でキャンセル。
    CancelNormalizeScan,
}

impl NativeOverlayInputRouting {
    pub fn should_forward_to_ui(
        self,
        event: &crate::video::native_window::NativeVideoWindowEvent,
    ) -> bool {
        use crate::video::native_window::NativeVideoWindowEvent as NativeEvent;

        match event {
            NativeEvent::KeyDown(key) | NativeEvent::KeyUp(key) => {
                if !self.text_input_active && native_video_fullscreen_shortcut_key(key) {
                    true
                } else {
                    !self.wants_keyboard_input
                }
            }
            NativeEvent::Text(_) | NativeEvent::Ime(_) => !self.wants_keyboard_input,
            // overlay が wheel を WheelNavigate / TileColumnsDelta に変換済みなら、
            // 同じ raw wheel を App へ二重転送しない。
            NativeEvent::MouseWheel(_) => !self.wants_pointer_input && !self.consumed_wheel,
            NativeEvent::MouseMove(_) | NativeEvent::MouseButton(_) | NativeEvent::MouseLeave => {
                !self.wants_pointer_input
            }
            // 内部処理イベント (presenter thread が直接消費する)。UI 転送しない。
            NativeEvent::GeometryChanged { .. }
            | NativeEvent::DpiChanged { .. }
            | NativeEvent::RequestRaiseHud => false,
        }
    }
}

fn create_present_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let mut device = None;
    let mut context = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let flags = D3D11_CREATE_DEVICE_FLAG(D3D11_CREATE_DEVICE_BGRA_SUPPORT.0);
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            flags,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )
        .map_err(|e| format!("D3D11CreateDevice presenter: {e:?}"))?;
    }
    let device =
        device.ok_or_else(|| "D3D11CreateDevice presenter returned null device".to_string())?;
    let context =
        context.ok_or_else(|| "D3D11CreateDevice presenter returned null context".to_string())?;
    crate::logger::log(format!(
        "native-presenter: presenter D3D11 device created (feature_level=0x{:X})",
        feature_level.0
    ));
    Ok((device, context))
}

fn configure_overlay_fonts(ctx: &egui::Context) {
    crate::ui_fonts::configure_fonts(ctx);
}

fn configure_overlay_style(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        // native HUD は 46pt の小さな操作面なので、ヘルプ text は hover 直後に出す。
        // egui default の 0.5s delay だと「初回だけ待つ / 隣ボタンは即時」という
        // grace-time 由来の不揃いな挙動に見える。
        style.interaction.tooltip_delay = 0.0;
    });
}

/// CP9 実機 debug: `MIV_HUD_DEBUG=1` で起動したか。
/// 起動後一度評価された値を cache (= env を変えても再評価しない、Once セマンティクス)。
pub(crate) fn hud_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_HUD_DEBUG").is_some())
}

fn hud_repaint_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_HUD_DEBUG_REPAINT").is_some())
}

impl NativeVideoPresenter {
    pub fn new(config: NativePresenterConfig) -> Result<Self, String> {
        unsafe {
            let (d3d_device, d3d_context) = create_present_d3d11_device()?;
            let d3d_device1: ID3D11Device1 = d3d_device
                .cast()
                .map_err(|e| format!("cast ID3D11Device1: {e:?}"))?;
            let d3d_device5: ID3D11Device5 = d3d_device
                .cast()
                .map_err(|e| format!("cast ID3D11Device5: {e:?}"))?;
            let d3d_context4: ID3D11DeviceContext4 = d3d_context
                .cast()
                .map_err(|e| format!("cast ID3D11DeviceContext4: {e:?}"))?;
            let d3d_context1: ID3D11DeviceContext1 = d3d_context
                .cast()
                .map_err(|e| format!("cast ID3D11DeviceContext1: {e:?}"))?;
            let dxgi_device: IDXGIDevice = d3d_device
                .cast()
                .map_err(|e| format!("cast IDXGIDevice: {e:?}"))?;
            let adapter = dxgi_device
                .GetAdapter()
                .map_err(|e| format!("IDXGIDevice::GetAdapter: {e:?}"))?;
            let factory: IDXGIFactory2 = adapter
                .GetParent()
                .map_err(|e| format!("IDXGIAdapter::GetParent: {e:?}"))?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: config.width,
                Height: config.height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 3,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            };
            let swap_chain = factory
                .CreateSwapChainForComposition(&d3d_device, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition: {e:?}"))?;
            let swap_chain2: IDXGISwapChain2 = swap_chain
                .cast()
                .map_err(|e| format!("cast IDXGISwapChain2: {e:?}"))?;
            swap_chain2
                .SetMaximumFrameLatency(1)
                .map_err(|e| format!("SetMaximumFrameLatency: {e:?}"))?;
            let waitable = swap_chain2.GetFrameLatencyWaitableObject();

            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)
                .map_err(|e| format!("DCompositionCreateDevice: {e:?}"))?;
            let target = dcomp_device
                .CreateTargetForHwnd(config.hwnd, true)
                .map_err(|e| format!("CreateTargetForHwnd: {e:?}"))?;
            let root_visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual root: {e:?}"))?;
            let background = NativeBlackBackground::new(
                &factory,
                &d3d_device,
                &d3d_device1,
                &d3d_context,
                &dcomp_device,
                config.width,
                config.height,
            )?;
            root_visual
                .AddVisual(&background._visual, false, None::<&IDCompositionVisual>)
                .map_err(|e| format!("IDCompositionVisual::AddVisual background: {e:?}"))?;
            let video_visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual video: {e:?}"))?;
            video_visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent video: {e:?}"))?;
            root_visual
                .AddVisual(&video_visual, true, &background._visual)
                .map_err(|e| format!("IDCompositionVisual::AddVisual video: {e:?}"))?;
            // CP4: HUD overlay HWND を生成 (= bars / interactive UI 用の独立 top-level)。
            // `hud_event_tx` が `Some` のときだけ作る。失敗時は presenter フォールバック経路
            // (= presenter HWND の DComp tree に egui overlay を載せる) に入る。
            //
            // HUD HWND が成功したら:
            //   - HUD 用 IDCompositionTarget + root_visual を作って struct で保持
            //   - egui overlay を HUD root に `after_visual=None` で attach
            //   - egui の dcomp_hwnd は HUD HWND、focus_hwnd は presenter HWND
            // 失敗したら:
            //   - egui overlay を presenter root に `after_visual=Some(&video_visual)` で attach
            //   - dcomp_hwnd / focus_hwnd ともに presenter HWND (= CP3 までと同じフォールバック挙動)
            let mut hud_window: Option<hud_window::HudOverlayWindow> = None;
            let mut hud_dcomp_target: Option<IDCompositionTarget> = None;
            let mut hud_root_visual: Option<IDCompositionVisual> = None;
            let mut hud_regions: Option<
                std::sync::Arc<std::sync::Mutex<hud_window::HudInteractiveRegions>>,
            > = None;
            let cursor_was_hidden = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            if let Some(hud_tx) = config.hud_event_tx.as_ref() {
                let regions = std::sync::Arc::new(std::sync::Mutex::new(
                    hud_window::HudInteractiveRegions::default(),
                ));
                // Codex CP7 P1 #2 反映: HUD HWND の初期 screen 座標を presenter HWND の
                // `GetWindowRect` で取得 (= secondary monitor / 負座標 monitor 対応)。
                // 失敗時は `(0, 0)` フォールバック (= primary monitor 想定)。
                let (hud_x, hud_y) = {
                    use windows::Win32::Foundation::RECT;
                    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
                    let mut rc = RECT::default();
                    if GetWindowRect(config.hwnd, &mut rc).is_ok() {
                        (rc.left, rc.top)
                    } else {
                        (0, 0)
                    }
                };
                let cfg = hud_window::HudOverlayConfig {
                    owner_hwnd: config.hwnd,
                    focus_hwnd: config.hwnd,
                    x: hud_x,
                    y: hud_y,
                    width: config.width,
                    height: config.height,
                    event_tx: hud_tx.clone(),
                    regions: std::sync::Arc::clone(&regions),
                    cursor_was_hidden: std::sync::Arc::clone(&cursor_was_hidden),
                };
                match hud_window::HudOverlayWindow::create(cfg) {
                    Ok(hud) => {
                        // HUD 用 DComp target / root visual を作る。drop 防止のため struct で保持。
                        match dcomp_device.CreateTargetForHwnd(hud.hwnd(), true) {
                            Ok(target) => match dcomp_device.CreateVisual() {
                                Ok(root_for_hud) => match target.SetRoot(&root_for_hud) {
                                    Ok(()) => {
                                        hud_window = Some(hud);
                                        hud_dcomp_target = Some(target);
                                        hud_root_visual = Some(root_for_hud);
                                        hud_regions = Some(regions);
                                        crate::logger::log(
                                            "[native-video] HUD overlay HWND created".to_string(),
                                        );
                                    }
                                    Err(err) => {
                                        crate::logger::log(format!(
                                            "native-presenter: HUD DComp target SetRoot failed, fallback: {err:?}"
                                        ));
                                    }
                                },
                                Err(err) => {
                                    crate::logger::log(format!(
                                        "native-presenter: HUD DComp CreateVisual failed, fallback: {err:?}"
                                    ));
                                }
                            },
                            Err(err) => {
                                crate::logger::log(format!(
                                    "native-presenter: HUD CreateTargetForHwnd failed, fallback: {err:?}"
                                ));
                            }
                        }
                    }
                    Err(err) => {
                        crate::logger::log(format!(
                            "native-presenter: HUD overlay HWND creation failed, fallback: {err}"
                        ));
                    }
                }
            }

            // CP4 + P2 #1 反映: egui overlay の attach は HUD 経路 → presenter フォールバック
            // 経路の 2 段階で試す。HUD 経路で `NativeEguiOverlay::new` が失敗したら、
            // HUD fields を全部 None にクリアしてから presenter フォールバック経路で
            // retry する。これがないと「HUD HWND は作られたが overlay は disable」という
            // 中途半端な状態 (= `hud_hwnd_out` に非 0、bars が見えない) になる。
            let mut egui_overlay: Option<NativeEguiOverlay> = None;
            if config.egui_overlay {
                // Step 1: HUD 経路で attach を試みる。
                if let Some(hud_root) = hud_root_visual.as_ref() {
                    let hud_hwnd_val = hud_window
                        .as_ref()
                        .map(|h| h.hwnd())
                        .unwrap_or_else(|| HWND(std::ptr::null_mut()));
                    let overlay_visual = dcomp_device
                        .CreateVisual()
                        .map_err(|e| format!("CreateVisual egui overlay (HUD): {e:?}"))?;
                    match NativeEguiOverlay::new(
                        overlay_visual,
                        &dcomp_device,
                        hud_root,
                        None,
                        hud_hwnd_val,
                        config.hwnd,
                        config.width,
                        config.height,
                        std::sync::Arc::clone(&cursor_was_hidden),
                    ) {
                        Ok(mut overlay) => {
                            if let Err(err) = overlay.render_once() {
                                crate::logger::log(format!(
                                    "native-presenter: HUD egui overlay initial render failed: {err}"
                                ));
                                log_event(
                                    "egui_overlay_error",
                                    &[("error", Value::from(err.to_string()))],
                                );
                            }
                            egui_overlay = Some(overlay);
                        }
                        Err(err) => {
                            // HUD 経路で失敗 → HUD fields を全部 drop して presenter フォールバック retry。
                            // `hud_window` / `hud_dcomp_target` / `hud_root_visual` / `hud_regions` を
                            // None にすることで HUD HWND が destroy され、`hud_hwnd_out` も 0 のままになる。
                            crate::logger::log(format!(
                                "native-presenter: HUD egui overlay init failed, falling back to presenter DComp tree: {err}"
                            ));
                            log_event(
                                "egui_overlay_error",
                                &[
                                    ("error", Value::from(err.to_string())),
                                    ("phase", Value::from("hud_path_fallback")),
                                ],
                            );
                            hud_window = None;
                            hud_dcomp_target = None;
                            hud_root_visual = None;
                            hud_regions = None;
                        }
                    }
                }

                // Step 2: フォールバック経路。Step 1 が完全成功なら skip、HUD HWND なし or
                // HUD 経路で失敗した場合は presenter root の DComp tree に attach する。
                if egui_overlay.is_none() {
                    let overlay_visual = dcomp_device
                        .CreateVisual()
                        .map_err(|e| format!("CreateVisual egui overlay (fallback): {e:?}"))?;
                    match NativeEguiOverlay::new(
                        overlay_visual,
                        &dcomp_device,
                        &root_visual,
                        Some(&video_visual),
                        config.hwnd,
                        config.hwnd,
                        config.width,
                        config.height,
                        std::sync::Arc::clone(&cursor_was_hidden),
                    ) {
                        Ok(mut overlay) => {
                            if let Err(err) = overlay.render_once() {
                                crate::logger::log(format!(
                                    "native-presenter: egui overlay initial render failed: {err}"
                                ));
                                log_event(
                                    "egui_overlay_error",
                                    &[("error", Value::from(err.to_string()))],
                                );
                            }
                            egui_overlay = Some(overlay);
                        }
                        Err(err) => {
                            crate::logger::log(format!(
                                "native-presenter: egui overlay disabled after init failure: {err}"
                            ));
                            log_event(
                                "egui_overlay_error",
                                &[("error", Value::from(err.to_string()))],
                            );
                        }
                    }
                }
            }

            let test_overlay = if config.test_overlay && egui_overlay.is_none() {
                let overlay = NativeTestOverlay::new(
                    &factory,
                    &d3d_device,
                    &d3d_device1,
                    &d3d_context,
                    &d3d_context1,
                    &dcomp_device,
                    config.width,
                    config.height,
                )?;
                root_visual
                    .AddVisual(&overlay._visual, true, &video_visual)
                    .map_err(|e| format!("IDCompositionVisual::AddVisual overlay: {e:?}"))?;
                Some(overlay)
            } else {
                None
            };
            target
                .SetRoot(&root_visual)
                .map_err(|e| format!("IDCompositionTarget::SetRoot: {e:?}"))?;
            dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit: {e:?}"))?;

            // presenter 自前の copy fence。フレームコピーの GPU 完了を `present_retire` 側で
            // 待ち合わせるために使う。作成失敗は致命的ではない (= `present_retire` が時間
            // ベース depth キャップへフォールバック) ので、Err にせず None で続行する。
            let copy_fence: Option<ID3D11Fence> = {
                let mut fence = None;
                match d3d_device5.CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence) {
                    Ok(()) => fence,
                    Err(e) => {
                        crate::logger::log(format!(
                            "native-presenter: copy fence CreateFence failed ({e:?}); \
                             present_retire falls back to depth cap"
                        ));
                        None
                    }
                }
            };

            let mut this = Self {
                swap_chain,
                waitable,
                d3d_device1,
                d3d_device5,
                d3d_context,
                d3d_context1,
                d3d_context4,
                _dcomp_device: dcomp_device,
                _dcomp_target: target,
                _root_visual: root_visual,
                _background: background,
                _video_visual: video_visual,
                backbuffer: None,
                test_overlay,
                egui_overlay,
                hud_window,
                _hud_dcomp_target: hud_dcomp_target,
                _hud_root_visual: hud_root_visual,
                hud_regions,
                editor_hwnds_snapshot: None,
                main_hwnd_for_raise: 0,
                last_logged_region_hash: None,
                lbutton_down_since: None,
                fence_cache: None,
                copy_fence,
                copy_fence_value: 0,
                shared_texture_cache: Vec::new(),
                cpu_upload_scratch: Vec::new(),
                pixel_probe_enabled: std::env::var_os("MIV_NATIVE_VIDEO_PIXEL_PROBE").is_some(),
                pixel_probe_strict: std::env::var_os("MIV_NATIVE_VIDEO_PIXEL_PROBE_STRICT")
                    .is_some(),
                last_pixel_probe: None,
                video_compact: false,
                sar_num: 1,
                sar_den: 1,
                width: config.width,
                height: config.height,
                surface_width: config.width,
                surface_height: config.height,
                factory,
                retired_video_surfaces: VecDeque::new(),
            };
            this.recreate_backbuffer(true)?;
            this.wait_for_initial_composition_ready();
            log_event(
                "init",
                &[
                    ("width", Value::from(config.width as i64)),
                    ("height", Value::from(config.height as i64)),
                    ("buffer_count", Value::from(3)),
                    ("latency", Value::from(1)),
                    ("test_overlay", Value::from(config.test_overlay)),
                    ("egui_overlay", Value::from(config.egui_overlay)),
                ],
            );
            Ok(this)
        }
    }

    fn wait_for_initial_composition_ready(&self) {
        let wait_t0 = Instant::now();
        let commit_ok = unsafe { self._dcomp_device.WaitForCommitCompletion() }.is_ok();
        let after_commit_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        let flush_ok = unsafe { DwmFlush() }.is_ok();
        let total_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        crate::logger::log(format!(
            "native-presenter: initial composition ready commit_ok={commit_ok} \
             flush_ok={flush_ok} commit_ms={after_commit_ms:.2} total_ms={total_ms:.2}"
        ));
        log_event(
            "initial_composition_ready",
            &[
                ("commit_ok", Value::from(commit_ok)),
                ("flush_ok", Value::from(flush_ok)),
                ("commit_ms", Value::from(after_commit_ms)),
                ("total_ms", Value::from(total_ms)),
            ],
        );
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return Ok(());
        }
        self.width = width;
        self.height = height;
        self._background
            .resize(&self.d3d_device1, &self.d3d_context, width, height)?;
        self.update_video_visual_transform()?;
        if let Some(overlay) = self.test_overlay.as_mut() {
            overlay.resize(
                &self.d3d_device1,
                &self.d3d_context,
                &self.d3d_context1,
                width,
                height,
            )?;
        }
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.resize(width, height)?;
        }
        log_event(
            "resize",
            &[
                ("width", Value::from(width as i64)),
                ("height", Value::from(height as i64)),
                ("surface_width", Value::from(self.surface_width as i64)),
                ("surface_height", Value::from(self.surface_height as i64)),
            ],
        );
        Ok(())
    }

    pub fn set_video_compact(&mut self, compact: bool) -> Result<(), String> {
        if self.video_compact == compact {
            return Ok(());
        }
        self.video_compact = compact;
        self.update_video_visual_transform()?;
        log_event(
            "video_compact",
            &[("compact", Value::from(self.video_compact))],
        );
        Ok(())
    }

    pub fn set_video_sar(&mut self, num: u32, den: u32) -> Result<(), String> {
        let num = num.max(1);
        let den = den.max(1);
        if self.sar_num == num && self.sar_den == den {
            return Ok(());
        }
        self.sar_num = num;
        self.sar_den = den;
        self.update_video_visual_transform()?;
        log_event(
            "video_sar",
            &[
                ("sar_num", Value::from(self.sar_num as i64)),
                ("sar_den", Value::from(self.sar_den as i64)),
            ],
        );
        Ok(())
    }

    pub fn present(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
    ) -> Result<NativePresentOutcome, String> {
        let new_w = frame.width.max(1);
        let new_h = frame.height.max(1);
        let new_sar_num = frame.sar_num.max(1);
        let new_sar_den = frame.sar_den.max(1);
        let size_changed = self.surface_width != new_w || self.surface_height != new_h;

        if size_changed {
            // 解像度変更: 新 swap chain を別途用意して原子的に差し替える。
            self.present_with_surface_swap(
                frame,
                sync_interval,
                new_w,
                new_h,
                new_sar_num,
                new_sar_den,
            )
        } else {
            // 解像度は不変。既存 swap chain をそのまま再利用する。
            self.present_reusing_surface(frame, sync_interval, new_sar_num, new_sar_den)
        }
    }

    /// 解像度不変時の present。既存の `swap_chain` / `backbuffer` をそのまま使う。
    /// SAR だけ変わった場合は transform を再計算する (swap chain content は正しいので
    /// 中間状態は生じない)。
    fn present_reusing_surface(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
        new_sar_num: u32,
        new_sar_den: u32,
    ) -> Result<NativePresentOutcome, String> {
        let wait_t0 = Instant::now();
        let wait_result = unsafe { WaitForSingleObject(self.waitable, 100) };
        let wait_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        let timed_out = wait_result == WAIT_TIMEOUT;

        let sar_changed = self.sar_num != new_sar_num || self.sar_den != new_sar_den;
        if sar_changed {
            // SAR だけ変わった (= 同一解像度で SAR 違いの動画へ切替)。swap chain の
            // サイズは正しいので transform を作り直すだけでよい。
            self.sar_num = new_sar_num;
            self.sar_den = new_sar_den;
            self.update_video_visual_transform()?;
        }

        let copy_t0 = Instant::now();
        let backbuffer = self
            .backbuffer
            .clone()
            .ok_or_else(|| "native presenter backbuffer is not initialized".to_string())?;
        let copy = self.copy_frame_into_backbuffer(frame, &backbuffer)?;
        let copy_ms = copy_t0.elapsed().as_secs_f64() * 1000.0;

        let present_t0 = Instant::now();
        let hr = unsafe { self.swap_chain.Present(sync_interval, Default::default()) };
        if hr.is_err() {
            return Err(format!("IDXGISwapChain::Present: {hr:?}"));
        }
        let present_call_ms = present_t0.elapsed().as_secs_f64() * 1000.0;

        Ok(NativePresentOutcome {
            path: copy.path,
            shared_handle: copy.shared_handle,
            shared_cache_hit: copy.shared_cache_hit,
            shared_texture_gen: copy.shared_texture_gen,
            fence_value: copy.fence_value,
            copy_fence_value: copy.copy_fence_value,
            surface_width: self.surface_width,
            surface_height: self.surface_height,
            geometry_changed: sar_changed,
            surface_swapped: false,
            commit_sync_ms: 0.0,
            wait_ms,
            wait_timed_out: timed_out,
            fence_wait_ms: copy.fence_wait_ms,
            open_shared_ms: copy.open_shared_ms,
            keyed_mutex_ms: copy.keyed_mutex_ms,
            keyed_mutex_cast_ms: copy.keyed_mutex_cast_ms,
            keyed_mutex_acquire_ms: copy.keyed_mutex_acquire_ms,
            copy_call_ms: copy.copy_call_ms,
            copy_ms,
            present_waitable_ms: wait_ms,
            present_call_ms,
            present_ms: present_call_ms,
        })
    }

    /// 解像度変更時の present。**新しい video swap chain を別途生成し、最初の正しい
    /// フレームを `Present` 済みにしてから、`SetContent` + `SetTransform2` を 1 回の
    /// `Commit` で原子的に差し替える**。旧 swap chain は Commit まで visual に
    /// 繋がったまま (= 正しい映像のまま) なので、黒や「左上にずれた中間フレーム」が
    /// 一切表示されない。`ResizeBuffers` (旧 content を破棄する) は使わない。
    ///
    /// 旧 swap chain は `WaitForCommitCompletion` 後も即 drop せず `retired_video_surfaces`
    /// に数世代分残す (DComp/DWM がまだ旧 content を参照しうるため。Codex 助言)。
    fn present_with_surface_swap(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
        new_w: u32,
        new_h: u32,
        new_sar_num: u32,
        new_sar_den: u32,
    ) -> Result<NativePresentOutcome, String> {
        let geom_t0 = Instant::now();
        // 1. 新 swap chain + backbuffer を用意する。ここでは visual には繋がない。
        let (new_swap_chain, new_waitable) = self.create_video_swap_chain(new_w, new_h)?;
        let new_backbuffer = self.create_swap_chain_backbuffer(&new_swap_chain)?;

        // 2. 最初のフレームを新 backbuffer へコピーする。
        let copy_t0 = Instant::now();
        let copy = self.copy_frame_into_backbuffer(frame, &new_backbuffer)?;
        let copy_ms = copy_t0.elapsed().as_secs_f64() * 1000.0;

        // 3. 新 swap chain を Present (= 新 swap chain は「正しいフレーム投入済み」状態)。
        let present_t0 = Instant::now();
        unsafe { new_swap_chain.Present(sync_interval, Default::default()) }
            .ok()
            .map_err(|e| format!("IDXGISwapChain::Present (new surface): {e:?}"))?;
        let present_call_ms = present_t0.elapsed().as_secs_f64() * 1000.0;

        // 4. content + transform を 1 回の Commit で原子的に差し替える。旧 swap chain は
        //    この Commit が反映されるまで visual に繋がったまま (= 正しい映像のまま)。
        //
        //    ⚠️ transform はローカル値 (new_*) から計算し、`self.*` はまだ touch しない。
        //    `SetContent` / `SetTransform2` / `Commit` のいずれかが失敗して `?` で早期
        //    return しても、presenter の状態 (`surface_width/height`, `sar_num/den`,
        //    `swap_chain`, `backbuffer`) は旧 surface のまま完全に一貫している。さもないと
        //    「`surface_*` だけ新サイズに進み swap_chain は旧のまま」という不整合に陥り、
        //    次回以降の `present` が `present_reusing_surface` 経路へ誤って入って固着する
        //    (Codex P2)。`self.*` の更新は Commit 成功後 (手順 6) に一括で行う。
        let (m11, m22, offset_x, offset_y) = compute_video_visual_transform(
            new_w,
            new_h,
            self.width,
            self.height,
            new_sar_num,
            new_sar_den,
            self.video_compact,
        );
        let transform = Matrix3x2 {
            M11: m11,
            M12: 0.0,
            M21: 0.0,
            M22: m22,
            M31: offset_x,
            M32: offset_y,
        };
        unsafe {
            self._video_visual
                .SetContent(&new_swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent (surface swap): {e:?}"))?;
            self._video_visual
                .SetTransform2(&transform)
                .map_err(|e| format!("IDCompositionVisual::SetTransform2 (surface swap): {e:?}"))?;
            self._dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit (surface swap): {e:?}"))?;
        }

        // 5. Commit が DWM の compositor tick まで反映され切るまで待つ。
        let commit_sync_ms = self.wait_for_video_transform_commit();

        // 6. Commit 成功。ここで初めて `self.*` を新 surface へ確定する (一括更新)。
        self.sar_num = new_sar_num;
        self.sar_den = new_sar_den;
        self.surface_width = new_w;
        self.surface_height = new_h;
        let old_swap_chain = std::mem::replace(&mut self.swap_chain, new_swap_chain);
        let old_waitable = std::mem::replace(&mut self.waitable, new_waitable);
        let old_backbuffer = self.backbuffer.replace(new_backbuffer);
        self.retired_video_surfaces.push_back(RetiredVideoSurface {
            _swap_chain: old_swap_chain,
            waitable: old_waitable,
            _backbuffer: old_backbuffer,
        });
        while self.retired_video_surfaces.len() > RETIRED_VIDEO_SURFACE_DEPTH {
            self.retired_video_surfaces.pop_front();
        }

        let geom_ms = geom_t0.elapsed().as_secs_f64() * 1000.0;
        crate::logger::log(format!(
            "native-presenter: video surface swapped to {new_w}x{new_h} sar={}/{} \
             geom_ms={geom_ms:.2} commit_sync_ms={commit_sync_ms:.2}",
            self.sar_num, self.sar_den
        ));
        log_event(
            "video_surface_swap",
            &[
                ("surface_width", Value::from(new_w as i64)),
                ("surface_height", Value::from(new_h as i64)),
                ("sar_num", Value::from(self.sar_num as i64)),
                ("sar_den", Value::from(self.sar_den as i64)),
                ("geom_ms", Value::from(geom_ms)),
                ("commit_sync_ms", Value::from(commit_sync_ms)),
                (
                    "retired_len",
                    Value::from(self.retired_video_surfaces.len() as i64),
                ),
            ],
        );

        Ok(NativePresentOutcome {
            path: copy.path,
            shared_handle: copy.shared_handle,
            shared_cache_hit: copy.shared_cache_hit,
            shared_texture_gen: copy.shared_texture_gen,
            fence_value: copy.fence_value,
            copy_fence_value: copy.copy_fence_value,
            surface_width: self.surface_width,
            surface_height: self.surface_height,
            geometry_changed: true,
            surface_swapped: true,
            commit_sync_ms,
            wait_ms: 0.0,
            wait_timed_out: false,
            fence_wait_ms: copy.fence_wait_ms,
            open_shared_ms: copy.open_shared_ms,
            keyed_mutex_ms: copy.keyed_mutex_ms,
            keyed_mutex_cast_ms: copy.keyed_mutex_cast_ms,
            keyed_mutex_acquire_ms: copy.keyed_mutex_acquire_ms,
            copy_call_ms: copy.copy_call_ms,
            copy_ms,
            present_waitable_ms: 0.0,
            present_call_ms,
            present_ms: present_call_ms,
        })
    }

    /// 新しい video swap chain を生成する (`new()` の生成ロジックと同一)。
    /// 戻り値は `(swap_chain, frame-latency waitable)`。
    fn create_video_swap_chain(
        &self,
        width: u32,
        height: u32,
    ) -> Result<(IDXGISwapChain1, HANDLE), String> {
        let width = width.max(1);
        let height = height.max(1);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 3,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
        };
        unsafe {
            let swap_chain = self
                .factory
                .CreateSwapChainForComposition(&self.d3d_device1, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition video: {e:?}"))?;
            let swap_chain2: IDXGISwapChain2 = swap_chain
                .cast()
                .map_err(|e| format!("cast IDXGISwapChain2 video: {e:?}"))?;
            swap_chain2
                .SetMaximumFrameLatency(1)
                .map_err(|e| format!("SetMaximumFrameLatency video: {e:?}"))?;
            let waitable = swap_chain2.GetFrameLatencyWaitableObject();
            Ok((swap_chain, waitable))
        }
    }

    /// 指定 swap chain の backbuffer を取得して黒クリアして返す (`Present` はしない)。
    fn create_swap_chain_backbuffer(
        &self,
        swap_chain: &IDXGISwapChain1,
    ) -> Result<ID3D11Texture2D, String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("IDXGISwapChain::GetBuffer (new surface): {e:?}"))?
        };
        let mut backbuffer_view = None;
        unsafe {
            self.d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut backbuffer_view))
                .map_err(|e| format!("CreateRenderTargetView (new surface): {e:?}"))?;
        }
        let backbuffer_view: ID3D11RenderTargetView = backbuffer_view
            .ok_or_else(|| "CreateRenderTargetView returned null (new surface)".to_string())?;
        unsafe {
            self.d3d_context
                .ClearRenderTargetView(&backbuffer_view, &[0.0, 0.0, 0.0, 1.0]);
        }
        Ok(backbuffer)
    }

    /// 1 フレームを指定 backbuffer へコピーする (GPU 共有テクスチャ経路 / CPU upload 経路)。
    /// `present_reusing_surface` と `present_with_surface_swap` の両方から使う共通処理。
    fn copy_frame_into_backbuffer(
        &mut self,
        frame: &VideoFrame,
        backbuffer: &ID3D11Texture2D,
    ) -> Result<FrameCopyMetrics, String> {
        let mut metrics = FrameCopyMetrics {
            path: "",
            shared_handle: 0,
            shared_cache_hit: false,
            shared_texture_gen: 0,
            fence_value: 0,
            fence_wait_ms: 0.0,
            open_shared_ms: 0.0,
            keyed_mutex_ms: 0.0,
            keyed_mutex_cast_ms: 0.0,
            keyed_mutex_acquire_ms: 0.0,
            copy_call_ms: 0.0,
            copy_fence_value: 0,
        };
        match &frame.data {
            VideoFrameData::Cpu(bytes) => {
                let probe_this_frame = self.pixel_probe_due();
                let src_probe = if probe_this_frame {
                    Some(sample_cpu_rgba_pixel(
                        bytes,
                        frame.width,
                        frame.height,
                        DXGI_FORMAT_B8G8R8A8_UNORM.0,
                    )?)
                } else {
                    None
                };
                copy_cpu_rgba_to_swapchain_bgra(
                    bytes,
                    &mut self.cpu_upload_scratch,
                    frame.width,
                    frame.height,
                )?;
                unsafe {
                    let copy_call_t0 = Instant::now();
                    self.d3d_context.UpdateSubresource(
                        backbuffer,
                        0,
                        None,
                        self.cpu_upload_scratch.as_ptr().cast(),
                        frame.width.saturating_mul(4),
                        0,
                    );
                    metrics.copy_call_ms = copy_call_t0.elapsed().as_secs_f64() * 1000.0;
                    if probe_this_frame {
                        let backbuffer_probe =
                            self.sample_texture_pixel(backbuffer, "backbuffer")?;
                        self.log_pixel_probe(
                            "cpu_upload",
                            0,
                            0,
                            0.0,
                            src_probe,
                            Some(backbuffer_probe),
                        );
                        if self.pixel_probe_strict {
                            compare_pixel_probe(
                                "cpu_upload",
                                src_probe.unwrap(),
                                backbuffer_probe,
                            )?;
                        }
                    }
                }
                metrics.path = "cpu_upload";
            }
            VideoFrameData::Gpu(gpu_frame) => {
                if gpu_frame.ten_bit {
                    return Err("10-bit D3D11 frame is not supported by native presenter".into());
                }
                if gpu_frame.width != frame.width || gpu_frame.height != frame.height {
                    return Err("D3D11 frame metadata size mismatch".into());
                }
                metrics.shared_handle = gpu_frame.shared_handle.0 as usize as u64;
                metrics.shared_texture_gen = gpu_frame.shared_texture_gen;
                metrics.fence_value = gpu_frame.fence_value;
                let probe_this_frame = self.pixel_probe_due();
                let fence = self.open_fence(gpu_frame.fence_gen, gpu_frame.fence_shared_handle)?;
                let fence_t0 = Instant::now();
                unsafe {
                    self.d3d_context4
                        .Wait(&fence, gpu_frame.fence_value)
                        .map_err(|e| format!("D3D11 fence wait: {e:?}"))?;
                }
                metrics.fence_wait_ms = fence_t0.elapsed().as_secs_f64() * 1000.0;
                let open_shared_t0 = Instant::now();
                let (src, cache_hit) = self
                    .open_shared_texture(gpu_frame.shared_handle, gpu_frame.shared_texture_gen)?;
                metrics.shared_cache_hit = cache_hit;
                metrics.open_shared_ms = open_shared_t0.elapsed().as_secs_f64() * 1000.0;
                let keyed_mutex_t0 = Instant::now();
                let keyed_mutex = self.acquire_source_keyed_mutex(
                    &src,
                    gpu_frame.shared_output_released_to_reader.clone(),
                )?;
                metrics.keyed_mutex_ms = keyed_mutex_t0.elapsed().as_secs_f64() * 1000.0;
                metrics.keyed_mutex_cast_ms = keyed_mutex.cast_ms;
                metrics.keyed_mutex_acquire_ms = keyed_mutex.acquire_ms;
                let _keyed_mutex_guard = keyed_mutex.guard;
                let src_probe = if probe_this_frame {
                    Some(self.sample_texture_pixel(&src, "source")?)
                } else {
                    None
                };
                unsafe {
                    let dst_res: ID3D11Resource = backbuffer
                        .cast()
                        .map_err(|e| format!("cast backbuffer resource: {e:?}"))?;
                    let src_res: ID3D11Resource = src
                        .cast()
                        .map_err(|e| format!("cast source resource: {e:?}"))?;
                    let copy_box = D3D11_BOX {
                        left: 0,
                        top: 0,
                        front: 0,
                        right: gpu_frame.width,
                        bottom: gpu_frame.height,
                        back: 1,
                    };
                    let copy_call_t0 = Instant::now();
                    self.d3d_context.CopySubresourceRegion(
                        &dst_res,
                        0,
                        0,
                        0,
                        0,
                        &src_res,
                        0,
                        Some(&copy_box),
                    );
                    metrics.copy_call_ms = copy_call_t0.elapsed().as_secs_f64() * 1000.0;
                    if probe_this_frame {
                        let backbuffer_probe =
                            self.sample_texture_pixel(backbuffer, "backbuffer")?;
                        self.log_pixel_probe(
                            "d3d11_shared",
                            gpu_frame.fence_gen,
                            gpu_frame.fence_value,
                            metrics.fence_wait_ms,
                            src_probe,
                            Some(backbuffer_probe),
                        );
                        if self.pixel_probe_strict {
                            compare_pixel_probe(
                                "d3d11_shared",
                                src_probe.unwrap(),
                                backbuffer_probe,
                            )?;
                        }
                    }
                }
                metrics.path = "d3d11_shared";
            }
        }
        // フレームコピー (UpdateSubresource / CopySubresourceRegion) を GPU タイムライン上で
        // 待ち合わせるための fence signal。`run_native_video_output` 側の `present_retire` が
        // `copy_fence_completed_value()` でこの値の到達を見て、コピーが GPU 上で完了した
        // フレームだけ共有出力 slot を解放する (= presenter のコピー完了前に producer が
        // slot を再利用して上書きするレースを構造的に塞ぐ)。
        if let Some(fence) = self.copy_fence.as_ref() {
            self.copy_fence_value += 1;
            let value = self.copy_fence_value;
            unsafe {
                match self.d3d_context4.Signal(fence, value) {
                    Ok(()) => metrics.copy_fence_value = value,
                    Err(e) => crate::logger::log(format!(
                        "native-presenter: copy fence Signal({value}) failed: {e:?}"
                    )),
                }
            }
        }
        Ok(metrics)
    }

    /// presenter copy fence の現在の完了値。`run_native_video_output` の `present_retire` が
    /// これを見て「コピーが GPU 上で完了したフレーム」を判定する。fence 未作成時は
    /// `u64::MAX` を返さず `None` を返し、呼び出し側は depth キャップのみで運用する。
    pub fn copy_fence_completed_value(&self) -> Option<u64> {
        self.copy_fence
            .as_ref()
            .map(|fence| unsafe { fence.GetCompletedValue() })
    }

    /// `shared_texture_cache` を全エントリ破棄する。各 entry は D3D11 `OpenSharedResource1`
    /// で開いた共有 texture を保持しており、4K で約 32 MB / 1080p で 8 MB を adapter
    /// memory に占有する。動画切替 (`SwitchSource`) 時は前動画の texture を残しても
    /// 二度と参照されないので即時破棄する (Codex 助言、2026-05-15)。残ったエントリは
    /// 次の `open_shared_texture` 呼出で再生成される (1-2ms オーバーヘッド、許容範囲)。
    pub fn clear_shared_texture_cache(&mut self) {
        let cleared = self.shared_texture_cache.len();
        self.shared_texture_cache.clear();
        if cleared > 0 {
            crate::logger::log(format!(
                "[native-presenter] shared_texture_cache cleared on source switch (entries={cleared})"
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "shared_texture_cache_cleared",
                    None,
                    0,
                    &[("entries", serde_json::Value::from(cleared as i64))],
                );
            }
        }
    }

    /// HUD overlay HWND の生 u64 値。HUD HWND が生成されていなければ 0。
    /// `run_native_video_output` が `hud_hwnd_out` に store するための accessor。
    #[allow(dead_code)]
    pub fn hud_hwnd(&self) -> u64 {
        self.hud_window
            .as_ref()
            .map(|hud| hud.hwnd().0 as u64)
            .unwrap_or(0)
    }

    /// HUD overlay HWND を最前面に上げ直す。**allowlist 判定なしの low-level API**。
    /// 通常は `try_raise_hud_to_top` を使って allowlist 判定を通す (Codex CP7 P1 #1 反映)。
    /// このメソッドは内部 (= `try_raise_hud_to_top`) からのみ呼ばれる想定で `pub` にしているが、
    /// 直接呼ぶと VST popup / mIV 設定ダイアログが foreground でも raise してしまうので注意。
    #[allow(dead_code)]
    pub fn raise_hud_to_top(&self) {
        if let Some(hud) = self.hud_window.as_ref() {
            hud.raise_to_top();
        }
    }

    /// HUD overlay HWND を最前面に上げ直す。**allowlist 判定込み版** (CP7 P1 #1 反映)。
    /// `RaiseHudToTop` command / `RequestRaiseHud` event / polling のすべての raise 経路で
    /// これを呼ぶ。`foreground_allows_hud_raise` が false (= VST popup / file dialog /
    /// mIV 設定ダイアログ等が foreground) のとき raise を skip して `false` を返す。
    /// 成功時は `true` を返す。HUD HWND が無いフォールバック経路でも `false`。
    #[allow(dead_code)]
    pub fn try_raise_hud_to_top(&self, presenter_hwnd: u64) -> bool {
        if self.hud_window.is_none() {
            return false;
        }
        // allowlist 判定。editor_hwnds_snapshot が未登録なら raise しない (= 安全側)。
        let editor_hwnds = match self.editor_hwnds_snapshot.as_ref() {
            Some(arc) => match arc.read() {
                Ok(g) => g.clone(),
                Err(_) => return false,
            },
            None => return false,
        };
        let hud_hwnd_val = self
            .hud_window
            .as_ref()
            .map(|h| h.hwnd().0 as u64)
            .unwrap_or(0);
        if !crate::video::dsp::foreground_allows_hud_raise(
            presenter_hwnd,
            hud_hwnd_val,
            self.main_hwnd_for_raise,
            &editor_hwnds,
        ) {
            return false;
        }
        if let Some(hud) = self.hud_window.as_ref() {
            hud.raise_to_top();
        }
        true
    }

    /// HUD HWND の geometry (= 位置・サイズ) を mirror する。`GeometryChanged` 受信時に
    /// presenter loop から呼ぶ。HUD HWND が無いなら no-op。
    #[allow(dead_code)]
    pub fn set_hud_geometry(&self, x: i32, y: i32, w: u32, h: u32) {
        if let Some(hud) = self.hud_window.as_ref() {
            hud.set_geometry(x, y, w, h);
        }
    }

    /// HUD `WM_NCHITTEST` フェイルセーフ用の `regions` shared lock。
    /// CP5 で `NativeEguiOverlay::run` 末尾から書き込む。HUD 無いなら `None`。
    #[allow(dead_code)]
    pub fn hud_regions_handle(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<hud_window::HudInteractiveRegions>>> {
        self.hud_regions.as_ref().map(std::sync::Arc::clone)
    }

    /// HUD HWND の `SetWindowRgn` を `regions` に合わせて更新する。CP5 で
    /// `NativeEguiOverlay::run` 末尾から `regions` 計算後に呼ばれる。
    #[allow(dead_code)]
    pub fn apply_hud_regions(&mut self, regions: &[RECT]) {
        if let Some(hud) = self.hud_window.as_mut() {
            hud.apply_regions(regions);
        }
    }

    /// CP8: HUD HWND の `WM_DPICHANGED` を受けて overlay の pixels_per_point を更新する。
    /// `dirty = true` 化されるので次フレームの render で region 物理ピクセル換算が新 DPI 基準になる。
    /// 戻り値: 値が変わったかどうか。
    #[allow(dead_code)]
    pub fn set_overlay_pixels_per_point(&mut self, ppp: f32) -> bool {
        self.egui_overlay
            .as_mut()
            .map(|o| o.set_pixels_per_point(ppp))
            .unwrap_or(false)
    }

    /// CP8 (Codex CP8 P2 反映): **overlay (egui_wgpu) の surface だけ** を resize する。
    /// presenter 全体 (= background / video transform / swap chain) は触らない。
    ///
    /// `DpiChanged` 経由で HUD HWND の `suggested_rect` に合わせて overlay surface を
    /// resize するときに使う。presenter HWND は別 monitor / 別 DPI 経路 (`WM_DPICHANGED`)
    /// で別途 resize される (現状未配線)。HUD の suggested_rect で presenter video まで
    /// 引っ張られると video transform が壊れるので、専用経路に分ける。
    ///
    /// 通常の `resize(width, height)` は presenter 全体を resize するので、
    /// `WM_WINDOWPOSCHANGED` 経由で presenter HWND 自身の geometry が変わったときに使う。
    #[allow(dead_code)]
    pub fn resize_overlay_surface_only(&mut self, width: u32, height: u32) -> Result<(), String> {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.resize(width, height)?;
        }
        Ok(())
    }

    /// CP6: HUD raise 判定で参照する `editor_hwnds` snapshot を登録する。
    /// CP7 で App が `dsp_bridge.editor_hwnds_snapshot()` を渡してくる。
    /// `None` のとき raise 判定は強制 false (= raise burst を起動しない)。
    #[allow(dead_code)]
    pub fn set_editor_hwnds_snapshot(
        &mut self,
        snapshot: Option<std::sync::Arc<std::sync::RwLock<std::collections::HashSet<u64>>>>,
    ) {
        self.editor_hwnds_snapshot = snapshot;
    }

    /// CP6: `foreground_allows_hud_raise` 判定用の既知 mIV HWND (= main HWND) を登録。
    /// presenter HWND と HUD HWND は presenter 自身が知っているので、外部から
    /// 渡すのは main HWND だけ。CP7 で App が設定する。
    #[allow(dead_code)]
    pub fn set_main_hwnd_for_raise_check(&mut self, main_hwnd: u64) {
        self.main_hwnd_for_raise = main_hwnd;
    }

    /// CP6: presenter thread loop から 50ms 周期で呼ばれる cursor polling。
    /// 戻り値: `true` なら呼び出し側で `raise_hud_to_top()` + retry burst を起動すべき。
    /// HUD HWND が無い (= CP4 フォールバック経路 / CP7 で flip 前) なら何もせず `false`。
    ///
    /// 役割:
    ///   1. `GetCursorPos` + `ScreenToClient(presenter_hwnd)` で cursor の presenter 座標を取得。
    ///   2. presenter HWND の client rect 範囲チェック (= 別モニターに移ったケースは弾く、
    ///      Codex 4 P1/P2 #3): 範囲外なら一度だけ synthetic `MouseLeave` を流して以降何もしない。
    ///   3. 範囲内: 直近 80ms 以内に HUD/presenter wndproc 経由の本物 `WM_MOUSEMOVE` が
    ///      届いていなければ synthetic `MouseMove` を overlay の `push_native_event` に流す
    ///      (= region 外 cursor でも hover 表示遷移を成立させる)。
    ///   4. cursor が activation zone (= 上端 0..76pt / 下端 H-220..H pt) 内、かつ
    ///      `editor_hwnds_snapshot` から `foreground_allows_hud_raise` が true を返した場合、
    ///      raise を要求する (= 戻り値 true)。判定不能なら false で skip。
    #[allow(dead_code)]
    pub fn cursor_polling_tick(
        &mut self,
        presenter_hwnd: u64,
        last_native_mouse_at: Option<std::time::Instant>,
        pointer_present_synthetic: &mut bool,
    ) -> bool {
        use windows::Win32::Foundation::{HWND, POINT};
        use windows::Win32::Graphics::Gdi::ScreenToClient;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

        // 実機修正 (2026-05-12): 外部 drag 検出。
        // `GetAsyncKeyState(VK_LBUTTON)` の最上位 bit が立っていれば左ボタン押下中。
        // ただし HUD region 内クリックは egui の `pointer.any_down()` でも true になる
        // (= 内部 drag、e.g. seek bar の drag)。両方の差分で「HUD 外で down している」
        // = 外部 drag (e.g. VST window のドラッグ) を判定する。
        //
        // ## 実機修正 (2026-05-12 P1 #3): 100ms delay を入れる
        //
        // 旧版は LBUTTON DOWN 直後のフレームで即 external_drag = true と判定していたが、
        // これだと「user が HUD top bar の button をクリックしたフレーム」と「event が
        // egui に届くフレーム」の間に polling が走って external_drag flip → top bar 非表示
        // になり、click event の処理時には button が描画されておらず click が失われる
        // (= ユーザー報告「VST ボタンを押しても反応しない、一瞬上のホバーバーが消える」)。
        //
        // LBUTTON DOWN を検出した最初のフレームでは `lbutton_down_since` だけ記録し、
        // external_drag は false のまま (= bar 表示維持)。100ms 経過後も DOWN 継続中で
        // egui に any_down が伝わっていなければ「真の外部 drag」と判定する。
        // 通常 click は数 ms ~ 数十 ms で UP するので、この delay で click は誤検出されない。
        let global_lbutton_down =
            unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 };
        let egui_pointer_down = self
            .egui_overlay
            .as_ref()
            .map(|o| o.egui_ctx.input(|i| i.pointer.any_down()))
            .unwrap_or(false);
        if global_lbutton_down {
            if self.lbutton_down_since.is_none() {
                self.lbutton_down_since = Some(std::time::Instant::now());
            }
        } else {
            self.lbutton_down_since = None;
        }
        let lbutton_down_long_enough = self
            .lbutton_down_since
            .map(|t| t.elapsed() >= std::time::Duration::from_millis(100))
            .unwrap_or(false);
        // Codex P2 #5 反映: HUD HWND が `SetCapture` で mouse を取っている間は HUD 起点クリック
        // (= seek drag や bar button hold) なので external_drag に分類しない。これで 100ms heuristic
        // を抜けた長押し操作 (e.g. seek bar drag が長引くケース) でも top bar を消さない。
        let hud_has_capture = if let Some(hud) = self.hud_window.as_ref() {
            let hud_hwnd = hud.hwnd();
            !hud_hwnd.0.is_null()
                && unsafe {
                    let cur = windows::Win32::UI::Input::KeyboardAndMouse::GetCapture();
                    cur.0 == hud_hwnd.0
                }
        } else {
            false
        };
        let external_drag = global_lbutton_down
            && !egui_pointer_down
            && lbutton_down_long_enough
            && !hud_has_capture;
        if let Some(overlay) = self.egui_overlay.as_mut() {
            if overlay.external_drag_in_progress != external_drag {
                overlay.external_drag_in_progress = external_drag;
                // 状態変化 → 次フレームで hover 判定が変わるので render dirty 化
                overlay.dirty = true;
            }
        }

        // HUD HWND が無い経路 (= CP4 フォールバック / CP7 で flip 前) は polling 不要。
        if self.hud_window.is_none() {
            return false;
        }
        let hwnd = HWND(presenter_hwnd as *mut _);
        let mut pt = POINT::default();
        let cursor_ok = unsafe { GetCursorPos(&mut pt) }.is_ok();
        if !cursor_ok {
            return false;
        }
        let in_range = unsafe { ScreenToClient(hwnd, &mut pt) }.as_bool() && {
            pt.x >= 0 && pt.y >= 0 && (pt.x as u32) < self.width && (pt.y as u32) < self.height
        };

        if !in_range {
            // 範囲外: overlay に残っている pointer_pos を 1 度だけ clear して終了。
            // 最後の mouse move が HUD HWND 由来の real event だった場合、
            // pointer_present_synthetic は false のままなので、それだけを見ると
            // right panel / seek HUD が stale hover で残り続ける。
            let overlay_has_pointer = self
                .egui_overlay
                .as_ref()
                .is_some_and(NativeEguiOverlay::has_pointer_pos);
            if overlay_has_pointer {
                if hud_debug_enabled() {
                    crate::logger::log(
                        "[HUD-DEBUG] polling MouseLeave (cursor out of client rect)".to_string(),
                    );
                }
                if let Some(overlay) = self.egui_overlay.as_mut() {
                    overlay.push_native_event(
                        crate::video::native_window::NativeVideoWindowEvent::MouseLeave,
                    );
                }
            }
            *pointer_present_synthetic = false;
            return false;
        }

        // 範囲内: 直近 80ms 以内に本物 mouse が届いていなければ synthetic MouseMove。
        // Codex CP6 再 P3 反映: `pointer_present_synthetic` は **synthetic を実際に push
        // した時のみ** `true` にする。recent_native で skip した場合はフラグを動かさない
        // (= 直前に synthetic だったなら true のまま、本物経路だけなら false のまま)。
        // これにより client rect 外に出たとき、native `WM_MOUSELEAVE` と二重で synthetic
        // `MouseLeave` を流すリスクを排除する (= 「synthetic を流した状態」だけが
        // synthetic leave を必要とする)。
        let now = std::time::Instant::now();
        let recent_native = last_native_mouse_at
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(80));
        let needs_synthetic_move = self
            .egui_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.needs_synthetic_pointer_move(pt.x, pt.y));
        if !recent_native && needs_synthetic_move {
            if hud_debug_enabled() {
                crate::logger::log(format!(
                    "[HUD-DEBUG] polling synthetic MouseMove x={} y={} (no native mouse in 80ms)",
                    pt.x, pt.y
                ));
            }
            if let Some(overlay) = self.egui_overlay.as_mut() {
                overlay.push_native_event(
                    crate::video::native_window::NativeVideoWindowEvent::MouseMove(
                        crate::video::native_window::NativeVideoMouseEvent {
                            x: pt.x,
                            y: pt.y,
                            shift: false,
                            ctrl: false,
                        },
                    ),
                );
                *pointer_present_synthetic = true;
            }
        }
        // recent_native / unchanged-position の場合は `pointer_present_synthetic` を変更しない
        // (前回値維持)。同じ座標の PointerMoved を投げ続けると egui の tooltip
        // delay が毎回リセットされ、HUD tooltips が出なくなる。

        // activation zone 判定 (= 上端 76pt 帯 / 下端 220pt 帯)。
        // pixels_per_point は overlay から取得 (= CP3 で導入した overlay の pixels_per_point)。
        let ppp = self
            .egui_overlay
            .as_ref()
            .map(|o| o.pixels_per_point.max(1.0))
            .unwrap_or(1.0);
        let top_band_px = (76.0_f32 * ppp).round() as i32;
        let bottom_band_top = self.height as i32 - (220.0_f32 * ppp).round() as i32;
        let in_activation_zone = pt.y < top_band_px || pt.y >= bottom_band_top;
        if !in_activation_zone {
            return false;
        }

        // raise allowlist 判定 (= mIV 既知 HWND / editor allowlist / popup 検出)。
        // editor_hwnds_snapshot が未登録なら raise 判定 false で skip。
        let editor_hwnds = match self.editor_hwnds_snapshot.as_ref() {
            Some(arc) => match arc.read() {
                Ok(g) => g.clone(),
                Err(_) => return false,
            },
            None => return false,
        };
        let hud_hwnd_val = self
            .hud_window
            .as_ref()
            .map(|h| h.hwnd().0 as u64)
            .unwrap_or(0);
        crate::video::dsp::foreground_allows_hud_raise(
            presenter_hwnd,
            hud_hwnd_val,
            self.main_hwnd_for_raise,
            &editor_hwnds,
        )
    }

    pub fn handle_window_events(
        &mut self,
        events: &[crate::video::native_window::NativeVideoWindowEvent],
    ) -> Result<NativeOverlayInputOutcome, String> {
        let outcome = if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.push_native_events(events);
            overlay.render_if_dirty()?
        } else {
            NativeOverlayInputOutcome::empty()
        };
        self.publish_hud_regions(&outcome);
        Ok(outcome)
    }

    /// Codex CP5 P1 反映: HUD HWND region 適用 + shared snapshot 更新を一箇所にまとめる。
    /// egui overlay が render を経由するすべての経路 (= `handle_window_events` /
    /// `tick_overlay_video_state` / その他 command 経由の overlay 更新) で必ず呼ぶ。
    /// 経路漏れがあると HUD region が stale になり、消えた UI の場所で HUD が
    /// 入力を取り続ける (= VST に入力が抜けない) regression を引き起こす。
    /// HUD HWND が無い (フォールバック経路) なら両方 no-op。
    fn publish_hud_regions(&mut self, outcome: &NativeOverlayInputOutcome) {
        // CP9 実機 debug log: `MIV_HUD_DEBUG=1` で起動したら region 変化を log する。
        if hud_debug_enabled() && self.hud_window.is_some() {
            let new_hash = hud_window::hash_regions_for_debug(&outcome.hud_regions);
            if self.last_logged_region_hash != Some(new_hash) {
                self.last_logged_region_hash = Some(new_hash);
                let rects_str: String = outcome
                    .hud_regions
                    .iter()
                    .map(|r| {
                        format!(
                            "({},{} {}x{})",
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(overlay) = self.egui_overlay.as_ref() {
                    crate::logger::log(format!(
                        "[HUD-DEBUG] regions changed n={} ptr={:?} top_bar={} hud_vis={} right={} jump={} vst3={} thumb={} pin={} rects=[{}]",
                        outcome.hud_regions.len(),
                        overlay.pointer_pos,
                        overlay.top_bar_visible,
                        overlay.hud_visible(),
                        overlay.right_panel_visible(),
                        overlay.jump_panel_visible,
                        overlay.vst3_panel_visible(),
                        overlay.hover_thumbnail.is_some(),
                        overlay.hover_preview_pinned,
                        rects_str,
                    ));
                }
            }
        }

        if let Some(regions_arc) = self.hud_regions.as_ref() {
            if let Ok(mut guard) = regions_arc.lock() {
                guard.regions = outcome.hud_regions.clone();
            }
        }
        self.apply_hud_regions(&outcome.hud_regions);
    }

    pub fn update_overlay_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        limiter_ceiling_hit_seq: u64,
        playback_speed: f64,
        frame_step_active: bool,
        initial_pause_controls_pending: bool,
        is_seeking: bool,
        seek_serial: u64,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.update_video_state(
                position_secs,
                duration_secs,
                is_playing,
                volume,
                muted,
                limiter_ceiling_hit_seq,
                playback_speed,
                frame_step_active,
                initial_pause_controls_pending,
                is_seeking,
                seek_serial,
            );
        }
    }

    pub fn set_overlay_perf_visible(&mut self, visible: bool) -> bool {
        self.egui_overlay
            .as_mut()
            .map(|overlay| overlay.set_perf_visible(visible))
            .unwrap_or(false)
    }

    pub fn push_overlay_perf_sample(
        &mut self,
        sample: NativeOverlayPerfSample,
        snapshot: NativeOverlayPerfSnapshot,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.push_perf_sample(sample, snapshot);
        }
    }

    pub fn set_overlay_hover_thumbnail(&mut self, thumbnail: Option<NativeOverlayThumbnail>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_hover_thumbnail(thumbnail);
        }
    }

    pub fn set_overlay_hover_preview_pinned(&mut self, pinned: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_hover_preview_pinned(pinned);
        }
    }

    pub fn set_overlay_timeline_markers(&mut self, markers: Vec<NativeOverlayTimelineMarker>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_timeline_markers(markers);
        }
    }

    pub fn set_overlay_jump_entries(&mut self, entries: Vec<NativeOverlayJumpEntry>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_jump_entries(entries);
        }
    }

    pub fn set_overlay_metadata(&mut self, metadata: Option<NativeOverlayMetadata>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_metadata(metadata);
        }
    }

    pub fn set_overlay_tile_overlay(&mut self, tile_overlay: Option<NativeOverlayTileOverlay>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_tile_overlay(tile_overlay);
        }
    }

    pub fn set_overlay_loop_enabled(&mut self, enabled: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_loop_enabled(enabled);
        }
    }

    /// HUD ボタン表示用のループモード (= ユーザー設定の display_mode) を overlay に伝える。
    pub fn set_overlay_loop_mode(&mut self, mode: crate::settings::VideoLoopMode) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_loop_mode(mode);
        }
    }

    pub fn set_overlay_checked(&mut self, checked: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_checked(checked);
        }
    }

    pub fn set_overlay_vst3_available(&mut self, available: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_vst3_available(available);
        }
    }

    pub fn set_overlay_vst3_panel(&mut self, panel: Option<NativeOverlayVst3Panel>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_vst3_panel(panel);
        }
    }

    pub fn set_overlay_playback_status(
        &mut self,
        first_frame_presented: bool,
        error: Option<String>,
        prep_status: crate::video::avio_progress::PreparingStatus,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_playback_status(first_frame_presented, error, prep_status);
        }
    }

    /// 音量ノーマライズ UI 状態を overlay に配信。boutton 色 + 進捗パネル描画に使う。
    pub fn set_overlay_normalize_state(
        &mut self,
        state: crate::video::normalize_types::NormalizeOverlayState,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_normalize_state(state);
        }
    }

    /// eframe 経由でルーティングされた活動 (Space キー pause/resume 等) を
    /// overlay に伝搬する。`push_native_event` を経由しないため明示的に呼ぶ必要がある。
    pub fn mark_overlay_cursor_activity(&mut self) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.mark_cursor_activity();
        }
    }

    pub fn show_overlay_toast(
        &mut self,
        text: String,
        centered: bool,
        linger: Option<std::time::Duration>,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.show_toast(text, centered, linger);
        }
    }

    pub fn set_pixel_probe(&mut self, enabled: bool, strict: bool) {
        self.pixel_probe_enabled = enabled;
        self.pixel_probe_strict = strict;
        self.last_pixel_probe = None;
    }

    pub fn tick_overlay_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        limiter_ceiling_hit_seq: u64,
        playback_speed: f64,
        frame_step_active: bool,
        initial_pause_controls_pending: bool,
        is_seeking: bool,
        seek_serial: u64,
    ) -> Result<NativeOverlayInputOutcome, String> {
        let outcome = if let Some(overlay) = self.egui_overlay.as_mut() {
            let force_tick_render =
                overlay.wants_periodic_tick() || overlay.repaint_due(Instant::now());
            overlay.update_video_state(
                position_secs,
                duration_secs,
                is_playing,
                volume,
                muted,
                limiter_ceiling_hit_seq,
                playback_speed,
                frame_step_active,
                initial_pause_controls_pending,
                is_seeking,
                seek_serial,
            );
            if force_tick_render {
                overlay.dirty = true;
            }
            overlay.render_if_dirty()?
        } else {
            NativeOverlayInputOutcome::empty()
        };
        // Codex CP5 P1 反映: tick 経路でも overlay UI が時間経過で表示/非表示に変わるので、
        // 必ず HUD region に反映する。漏らすと periodic 表示状態 (= toast / hover preview /
        // tile overlay refresh 等) と region がズレて VST にクリックが奪われる。
        self.publish_hud_regions(&outcome);
        Ok(outcome)
    }

    pub fn overlay_hud_visible(&self) -> bool {
        self.egui_overlay
            .as_ref()
            .map(NativeEguiOverlay::hud_visible)
            .unwrap_or(false)
    }

    pub fn overlay_wants_periodic_tick(&self) -> bool {
        self.egui_overlay
            .as_ref()
            .map(NativeEguiOverlay::wants_periodic_tick)
            .unwrap_or(false)
    }

    pub fn overlay_repaint_due(&self, now: Instant) -> bool {
        self.egui_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.repaint_due(now))
    }

    pub fn overlay_needs_render(&self) -> bool {
        self.egui_overlay
            .as_ref()
            .map(NativeEguiOverlay::needs_render)
            .unwrap_or(false)
    }

    fn open_fence(
        &mut self,
        fence_gen: u64,
        fence_shared_handle: HANDLE,
    ) -> Result<ID3D11Fence, String> {
        let handle_key = fence_shared_handle.0 as isize;
        let needs_open = !matches!(
            self.fence_cache,
            Some((cached_gen, cached_handle, _)) if cached_gen == fence_gen && cached_handle == handle_key
        );
        if needs_open {
            let mut fence = None;
            unsafe {
                self.d3d_device5
                    .OpenSharedFence(fence_shared_handle, &mut fence)
                    .map_err(|e| format!("OpenSharedFence: {e:?}"))?;
            }
            let fence = fence.ok_or_else(|| "OpenSharedFence returned null".to_string())?;
            self.fence_cache = Some((fence_gen, handle_key, fence));
        }
        Ok(self.fence_cache.as_ref().unwrap().2.clone())
    }

    /// 共有出力テクスチャを `OpenSharedResource1` で開く。結果は `(handle 値, gen)` を
    /// キーにキャッシュする。
    ///
    /// ⚠️ **`gen` をキーに含めるのが必須**: NT shared handle の値は、decoder 側で
    /// 共有出力 slot を evict (`CloseHandle`) した後に OS が別の slot 用へ再利用しうる。
    /// handle 値だけでキャッシュすると、動画切替で「前動画のテクスチャ」を stale なまま
    /// 返してしまい、新動画の再生中に前動画のフレームが 1 枚混入する (2026-05-15 報告の
    /// frame 225)。`gen` (`SharedOutputSlot::texture_gen`、プロセス内ユニーク・単調増加)
    /// を組にすることで、handle 値が再利用されても別エントリとして必ず開き直す。
    /// `open_fence` の `fence_gen` と同じ防御。
    fn open_shared_texture(
        &mut self,
        shared_handle: HANDLE,
        shared_texture_gen: u64,
    ) -> Result<(ID3D11Texture2D, bool), String> {
        let handle_key = shared_handle.0 as usize as u64;
        let cache_key = (handle_key, shared_texture_gen);
        if let Some(pos) = self
            .shared_texture_cache
            .iter()
            .position(|(cached_key, _)| *cached_key == cache_key)
        {
            let texture = self.shared_texture_cache[pos].1.clone();
            if pos != 0 {
                let entry = self.shared_texture_cache.remove(pos);
                self.shared_texture_cache.insert(0, entry);
            }
            return Ok((texture, true));
        }

        let texture: ID3D11Texture2D = unsafe {
            self.d3d_device1
                .OpenSharedResource1(shared_handle)
                .map_err(|e| format!("OpenSharedResource1 frame texture: {e:?}"))?
        };
        self.shared_texture_cache
            .insert(0, (cache_key, texture.clone()));
        if self.shared_texture_cache.len() > SHARED_TEXTURE_CACHE_CAPACITY {
            self.shared_texture_cache.pop();
        }
        Ok((texture, false))
    }

    /// `present_with_surface_swap` の `Commit` 直後に呼ぶ。content + transform の Commit が
    /// composition engine に処理され切るまで presenter thread を待たせる。これにより
    /// 旧 swap chain を `retired_video_surfaces` へ移して以降の present に進む時点で、
    /// DComp は確実に新 swap chain を指している。
    ///
    /// `WaitForCommitCompletion` だけでは、実機で 3840→1920 のような縮小 swap 時に
    /// 新 content が旧 transform で 1 refresh 見えることがあった。黒フレームは挿入せず、
    /// `DwmFlush` で DWM 側の表示反映まで同期して content / transform のペアを固定する。
    /// 戻り値は待ちに要した時間 (ms)。
    fn wait_for_video_transform_commit(&self) -> f64 {
        let t0 = Instant::now();
        unsafe {
            let _ = self._dcomp_device.WaitForCommitCompletion();
            let _ = DwmFlush();
        }
        t0.elapsed().as_secs_f64() * 1000.0
    }

    fn update_video_visual_transform(&self) -> Result<(), String> {
        let (m11, m22, offset_x, offset_y) = compute_video_visual_transform(
            self.surface_width,
            self.surface_height,
            self.width,
            self.height,
            self.sar_num,
            self.sar_den,
            self.video_compact,
        );
        let transform = Matrix3x2 {
            M11: m11,
            M12: 0.0,
            M21: 0.0,
            M22: m22,
            M31: offset_x,
            M32: offset_y,
        };
        unsafe {
            self._video_visual
                .SetTransform2(&transform)
                .map_err(|e| format!("IDCompositionVisual::SetTransform2 video: {e:?}"))?;
            self._dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit video transform: {e:?}"))?;
        }
        Ok(())
    }

    fn pixel_probe_due(&mut self) -> bool {
        if !self.pixel_probe_enabled {
            return false;
        }
        let now = Instant::now();
        let due = self
            .last_pixel_probe
            .map(|last| now.duration_since(last) >= Duration::from_secs(1))
            .unwrap_or(true);
        if due {
            self.last_pixel_probe = Some(now);
        }
        due
    }

    fn acquire_source_keyed_mutex(
        &self,
        texture: &ID3D11Texture2D,
        released_to_reader: Option<Arc<AtomicBool>>,
    ) -> Result<SourceKeyedMutexAcquire, String> {
        let cast_t0 = Instant::now();
        let Ok(mutex) = texture.cast::<IDXGIKeyedMutex>() else {
            return Ok(SourceKeyedMutexAcquire {
                guard: None,
                cast_ms: cast_t0.elapsed().as_secs_f64() * 1000.0,
                acquire_ms: 0.0,
            });
        };
        let cast_ms = cast_t0.elapsed().as_secs_f64() * 1000.0;
        let acquire_t0 = Instant::now();
        unsafe {
            mutex
                .AcquireSync(1, 10)
                .map_err(|e| format!("source keyed mutex AcquireSync(1): {e:?}"))?;
        }
        let acquire_ms = acquire_t0.elapsed().as_secs_f64() * 1000.0;
        Ok(SourceKeyedMutexAcquire {
            guard: Some(KeyedMutexReadGuard {
                mutex,
                released_to_reader,
            }),
            cast_ms,
            acquire_ms,
        })
    }

    fn sample_texture_pixel(
        &self,
        texture: &ID3D11Texture2D,
        label: &str,
    ) -> Result<NativePixelSample, String> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Width == 0 || desc.Height == 0 {
                return Err(format!("pixel probe {label}: empty texture desc"));
            }
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                return Err(format!(
                    "pixel probe {label}: unsupported format {:?}",
                    desc.Format
                ));
            }

            let x = desc.Width / 2;
            let y = desc.Height / 2;
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: 1,
                Height: 1,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            self.d3d_device1
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|e| format!("pixel probe {label}: CreateTexture2D staging: {e:?}"))?;
            let staging =
                staging.ok_or_else(|| format!("pixel probe {label}: staging texture null"))?;
            let src_res: ID3D11Resource = texture
                .cast()
                .map_err(|e| format!("pixel probe {label}: cast source: {e:?}"))?;
            let staging_res: ID3D11Resource = staging
                .cast()
                .map_err(|e| format!("pixel probe {label}: cast staging: {e:?}"))?;
            let src_box = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x.saturating_add(1),
                bottom: y.saturating_add(1),
                back: 1,
            };
            self.d3d_context.CopySubresourceRegion(
                &staging_res,
                0,
                0,
                0,
                0,
                &src_res,
                0,
                Some(&src_box),
            );

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.d3d_context
                .Map(&staging_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| format!("pixel probe {label}: Map staging: {e:?}"))?;
            let ptr = mapped.pData.cast::<u8>();
            let sample = NativePixelSample {
                x,
                y,
                width: desc.Width,
                height: desc.Height,
                format: desc.Format.0,
                b: *ptr,
                g: *ptr.add(1),
                r: *ptr.add(2),
                a: *ptr.add(3),
            };
            self.d3d_context.Unmap(&staging_res, 0);
            Ok(sample)
        }
    }

    fn log_pixel_probe(
        &self,
        path: &str,
        fence_gen: u64,
        fence_value: u64,
        fence_wait_ms: f64,
        source: Option<NativePixelSample>,
        backbuffer: Option<NativePixelSample>,
    ) {
        let src = source.unwrap_or(NativePixelSample {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            format: 0,
            b: 0,
            g: 0,
            r: 0,
            a: 0,
        });
        let dst = backbuffer.unwrap_or(src);
        crate::logger::log(format!(
            "native-presenter: pixel_probe path={path} fence_gen={fence_gen} fence_value={fence_value} \
             fence_wait_ms={fence_wait_ms:.3} src@{},{} size={}x{} fmt={} bgra=({},{},{},{}) \
             backbuffer@{},{} size={}x{} fmt={} bgra=({},{},{},{})",
            src.x,
            src.y,
            src.width,
            src.height,
            src.format,
            src.b,
            src.g,
            src.r,
            src.a,
            dst.x,
            dst.y,
            dst.width,
            dst.height,
            dst.format,
            dst.b,
            dst.g,
            dst.r,
            dst.a,
        ));
        log_event(
            "pixel_probe",
            &[
                ("path", Value::from(path)),
                ("fence_gen", Value::from(fence_gen as i64)),
                ("fence_value", Value::from(fence_value as i64)),
                ("fence_wait_ms", Value::from(fence_wait_ms)),
                ("source_b", Value::from(src.b as i64)),
                ("source_g", Value::from(src.g as i64)),
                ("source_r", Value::from(src.r as i64)),
                ("source_a", Value::from(src.a as i64)),
                ("source_x", Value::from(src.x as i64)),
                ("source_y", Value::from(src.y as i64)),
                ("source_width", Value::from(src.width as i64)),
                ("source_height", Value::from(src.height as i64)),
                ("source_format", Value::from(src.format as i64)),
                ("backbuffer_b", Value::from(dst.b as i64)),
                ("backbuffer_g", Value::from(dst.g as i64)),
                ("backbuffer_r", Value::from(dst.r as i64)),
                ("backbuffer_a", Value::from(dst.a as i64)),
                ("backbuffer_x", Value::from(dst.x as i64)),
                ("backbuffer_y", Value::from(dst.y as i64)),
                ("backbuffer_width", Value::from(dst.width as i64)),
                ("backbuffer_height", Value::from(dst.height as i64)),
                ("backbuffer_format", Value::from(dst.format as i64)),
            ],
        );
    }

    fn recreate_backbuffer(&mut self, present_initial_black: bool) -> Result<(), String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("IDXGISwapChain::GetBuffer: {e:?}"))?
        };
        let mut backbuffer_view = None;
        unsafe {
            self.d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut backbuffer_view))
                .map_err(|e| format!("CreateRenderTargetView: {e:?}"))?;
        }
        let backbuffer_view: ID3D11RenderTargetView =
            backbuffer_view.ok_or_else(|| "CreateRenderTargetView returned null".to_string())?;
        unsafe {
            self.d3d_context
                .ClearRenderTargetView(&backbuffer_view, &[0.0, 0.0, 0.0, 1.0]);
            if present_initial_black {
                self.swap_chain
                    .Present(1, Default::default())
                    .ok()
                    .map_err(|e| format!("initial IDXGISwapChain::Present: {e:?}"))?;
            }
        }
        self.backbuffer = Some(backbuffer);
        Ok(())
    }
}

impl NativeBlackBackground {
    fn new(
        factory: &IDXGIFactory2,
        d3d_device: &ID3D11Device,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        dcomp_device: &IDCompositionDevice,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let width = width.max(1);
        let height = height.max(1);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let swap_chain = unsafe {
            factory
                .CreateSwapChainForComposition(d3d_device, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition background: {e:?}"))?
        };
        let visual = unsafe {
            let visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual background: {e:?}"))?;
            visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent background: {e:?}"))?;
            visual
        };
        let mut this = Self {
            swap_chain,
            _visual: visual,
            backbuffer: None,
            render_target: None,
            width,
            height,
        };
        this.recreate_backbuffer(d3d_device1, d3d_context)?;
        log_event(
            "background_init",
            &[
                ("width", Value::from(this.width as i64)),
                ("height", Value::from(this.height as i64)),
            ],
        );
        Ok(this)
    }

    fn resize(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return Ok(());
        }
        self.render_target = None;
        self.backbuffer = None;
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width,
                    height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|e| format!("background IDXGISwapChain::ResizeBuffers: {e:?}"))?;
        }
        self.width = width;
        self.height = height;
        self.recreate_backbuffer(d3d_device1, d3d_context)?;
        Ok(())
    }

    fn recreate_backbuffer(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
    ) -> Result<(), String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("background IDXGISwapChain::GetBuffer: {e:?}"))?
        };
        let mut render_target = None;
        unsafe {
            d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut render_target))
                .map_err(|e| format!("background CreateRenderTargetView: {e:?}"))?;
        }
        let render_target: ID3D11RenderTargetView = render_target
            .ok_or_else(|| "background CreateRenderTargetView returned null".to_string())?;
        unsafe {
            d3d_context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.0, 1.0]);
            self.swap_chain
                .Present(1, Default::default())
                .ok()
                .map_err(|e| format!("background IDXGISwapChain::Present: {e:?}"))?;
        }
        self.backbuffer = Some(backbuffer);
        self.render_target = Some(render_target);
        Ok(())
    }
}

impl NativeEguiOverlay {
    fn new(
        visual: IDCompositionVisual,
        dcomp_device: &IDCompositionDevice,
        root_visual: &IDCompositionVisual,
        after_visual: Option<&IDCompositionVisual>,
        dcomp_hwnd: HWND,
        focus_hwnd: HWND,
        width: u32,
        height: u32,
        cursor_was_hidden_shared: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..Default::default()
        });
        let surface = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                    visual.as_raw() as *mut core::ffi::c_void,
                ))
                .map_err(|e| format!("wgpu CompositionVisual surface: {e:?}"))?
        };
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|e| format!("wgpu request_adapter for DComp overlay: {e:?}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mIV native egui overlay"),
            ..Default::default()
        }))
        .map_err(|e| format!("wgpu request_device for DComp overlay: {e:?}"))?;
        let caps = surface.get_capabilities(&adapter);
        let format = choose_overlay_surface_format(&caps.formats)?;
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::AutoVsync) {
            wgpu::PresentMode::AutoVsync
        } else {
            *caps
                .present_modes
                .first()
                .ok_or_else(|| "wgpu DComp overlay surface has no present modes".to_string())?
        };
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            wgpu::CompositeAlphaMode::Auto
        };
        let renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        let egui_ctx = egui::Context::default();
        configure_overlay_fonts(&egui_ctx);
        configure_overlay_style(&egui_ctx);
        let pixels_per_point = pixels_per_point_for_hwnd(dcomp_hwnd);
        let this = Self {
            surface,
            visual,
            dcomp_device: dcomp_device.clone(),
            root_visual: root_visual.clone(),
            after_visual: after_visual.cloned(),
            _instance: instance,
            adapter,
            device,
            queue,
            format,
            present_mode,
            alpha_mode,
            renderer,
            egui_ctx,
            dcomp_hwnd,
            focus_hwnd,
            started_at: Instant::now(),
            pending_events: Vec::new(),
            modifiers: egui::Modifiers::default(),
            pointer_pos: None,
            event_count: 0,
            dirty: true,
            next_repaint_deadline: None,
            wants_pointer_input: false,
            wants_keyboard_input: false,
            video_position_secs: 0.0,
            video_duration_secs: 0.0,
            video_is_playing: false,
            video_volume: 1.0,
            video_muted: false,
            video_limiter_ceiling_hit_seq: 0,
            video_limiter_visible_until: None,
            video_playback_speed: 1.0,
            video_frame_step_active: false,
            initial_pause_controls_pending: false,
            video_is_seeking: false,
            video_seek_serial: 0,
            seek_status_started_at: None,
            seek_status_visible_since: None,
            seek_status_visible: false,
            video_speed_popup_open: false,
            frame_step_hold: None,
            video_loop_enabled: false,
            video_loop_mode: crate::settings::VideoLoopMode::Off,
            video_checked: false,
            vst3_available: false,
            vst3_panel: None,
            first_frame_presented: false,
            video_error: None,
            preparing_status: crate::video::avio_progress::PreparingStatus {
                phase: crate::video::avio_progress::prep_phase::OPENING,
                bytes_read: 0,
                file_size: 0,
            },
            toast: None,
            perf_visible: false,
            perf_history: VecDeque::with_capacity(256),
            perf_latest: NativeOverlayPerfSnapshot::default(),
            perf_last_dirty: Instant::now(),
            perf_pause_gap_pending: false,
            last_seek_target_secs: None,
            last_thumbnail_request_secs: None,
            last_thumbnail_request_at: None,
            hover_preview_target_secs: None,
            hover_preview_pinned: false,
            last_drawn_preview_rect: None,
            last_drawn_vst3_panel_rect: None,
            last_emitted_vst3_panel_pos: None,
            last_drawn_paused_center_rects: None,
            last_drawn_toast_rect: None,
            last_drawn_speed_popup_rect: None,
            last_drawn_bookmark_editor_rect: None,
            hover_thumbnail: None,
            hover_texture: None,
            hover_texture_key: None,
            timeline_markers: Vec::new(),
            jump_entries: Vec::new(),
            bookmark_title_edit: None,
            video_metadata: None,
            tile_overlay: None,
            tile_textures: HashMap::new(),
            jump_textures: HashMap::new(),
            top_bar_visible: false,
            right_panel_visible: false,
            jump_panel_visible: false,
            external_drag_in_progress: false,
            pending_overlay_commands: Vec::new(),
            last_volume_target: None,
            visual_attached: false,
            pixels_per_point,
            width: width.max(1),
            height: height.max(1),
            cursor_last_activity: None,
            cursor_hidden: false,
            cursor_was_hidden_shared,
            normalize_state: crate::video::normalize_types::NormalizeOverlayState::default(),
        };
        this.configure();
        log_event(
            "egui_overlay_init",
            &[
                ("width", Value::from(this.width as i64)),
                ("height", Value::from(this.height as i64)),
                ("format", Value::from(format!("{:?}", this.format))),
                (
                    "present_mode",
                    Value::from(format!("{:?}", this.present_mode)),
                ),
                ("alpha_mode", Value::from(format!("{:?}", this.alpha_mode))),
                ("adapter", Value::from(this.adapter.get_info().name)),
                ("pixels_per_point", Value::from(this.pixels_per_point)),
                ("visual_attached", Value::from(this.visual_attached)),
            ],
        );
        Ok(this)
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return Ok(());
        }
        self.width = width;
        self.height = height;
        self.configure();
        self.dirty = true;
        self.render_once().map(|_| ())
    }

    /// CP8: DPI 変更を反映する。`pixels_per_point = dpi as f32 / 96.0`。
    /// 戻り値: 値が変わったかどうか。変わった場合は呼び出し側で next render の
    /// region 再計算を期待する (= `dirty = true`)。
    fn set_pixels_per_point(&mut self, ppp: f32) -> bool {
        let new_ppp = ppp.max(0.5).min(8.0); // 50%-800% の範囲にクランプ
        if (self.pixels_per_point - new_ppp).abs() < f32::EPSILON {
            return false;
        }
        self.pixels_per_point = new_ppp;
        self.dirty = true;
        true
    }

    fn push_native_events(
        &mut self,
        events: &[crate::video::native_window::NativeVideoWindowEvent],
    ) {
        for event in events {
            self.push_native_event(event.clone());
        }
    }

    fn push_native_event(&mut self, event: crate::video::native_window::NativeVideoWindowEvent) {
        use crate::video::native_window::{
            NativeVideoImeEvent, NativeVideoMouseButton, NativeVideoWindowEvent as NativeEvent,
        };

        self.event_count = self.event_count.saturating_add(1);
        // カーソル auto-hide 用のアクティビティタイマ更新。MouseLeave は「カーソルが
        // ウィンドウから出た」ので活動とみなさない (= 隠す方向に進める)。
        if !matches!(event, NativeEvent::MouseLeave) {
            self.cursor_last_activity = Some(Instant::now());
            self.cursor_hidden = false;
        }
        match event {
            NativeEvent::KeyDown(key) | NativeEvent::KeyUp(key) => {
                let modifiers = egui_modifiers(key.shift, key.ctrl, key.alt);
                self.modifiers = modifiers;
                if !self.text_input_active() && native_video_fullscreen_shortcut_key(&key) {
                    return;
                }
                if let Some(egui_key) = egui_key_from_virtual_key(key.virtual_key) {
                    let pressed = matches!(event, NativeEvent::KeyDown(_));
                    self.pending_events.push(egui::Event::Key {
                        key: egui_key,
                        physical_key: Some(egui_key),
                        pressed,
                        repeat: key.repeat,
                        modifiers,
                    });
                    self.dirty = true;
                }
            }
            NativeEvent::Text(ch) => {
                self.pending_events.push(egui::Event::Text(ch.to_string()));
                self.dirty = true;
            }
            NativeEvent::Ime(ime) => {
                let ime = match ime {
                    NativeVideoImeEvent::Enabled => egui::ImeEvent::Enabled,
                    NativeVideoImeEvent::Preedit(text) => egui::ImeEvent::Preedit(text),
                    NativeVideoImeEvent::Commit(text) => egui::ImeEvent::Commit(text),
                    NativeVideoImeEvent::Disabled => egui::ImeEvent::Disabled,
                };
                self.pending_events.push(egui::Event::Ime(ime));
                self.dirty = true;
            }
            NativeEvent::MouseMove(mouse) => {
                let pos = self.native_pos(mouse.x, mouse.y);
                self.pointer_pos = Some(pos);
                self.modifiers = egui_modifiers(mouse.shift, mouse.ctrl, false);
                self.pending_events.push(egui::Event::PointerMoved(pos));
                self.dirty = true;
            }
            NativeEvent::MouseButton(button) => {
                let pos = self.native_pos(button.x, button.y);
                let modifiers = egui_modifiers(button.shift, button.ctrl, false);
                self.pointer_pos = Some(pos);
                self.modifiers = modifiers;
                self.pending_events.push(egui::Event::PointerMoved(pos));
                let egui_button = match button.button {
                    NativeVideoMouseButton::Left => egui::PointerButton::Primary,
                    NativeVideoMouseButton::Right => egui::PointerButton::Secondary,
                    NativeVideoMouseButton::Middle => egui::PointerButton::Middle,
                    NativeVideoMouseButton::Extra1 => egui::PointerButton::Extra1,
                    NativeVideoMouseButton::Extra2 => egui::PointerButton::Extra2,
                };
                self.pending_events.push(egui::Event::PointerButton {
                    pos,
                    button: egui_button,
                    pressed: button.down,
                    modifiers,
                });
                self.dirty = true;
            }
            NativeEvent::MouseWheel(wheel) => {
                let pos = self.native_pos(wheel.x, wheel.y);
                let modifiers = egui_modifiers(wheel.shift, wheel.ctrl, false);
                self.pointer_pos = Some(pos);
                self.modifiers = modifiers;
                let over_scroll_panel = self.pointer_over_scroll_panel(pos);
                if wheel.ctrl && self.tile_overlay.is_some() {
                    self.pending_overlay_commands
                        .push(NativeOverlayCommand::TileColumnsDelta {
                            delta: if wheel.delta > 0 { -1 } else { 1 },
                        });
                } else if !wheel.ctrl && !over_scroll_panel {
                    self.pending_overlay_commands
                        .push(NativeOverlayCommand::WheelNavigate {
                            delta: if wheel.delta < 0 { 1 } else { -1 },
                        });
                }
                self.pending_events.push(egui::Event::PointerMoved(pos));
                self.pending_events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, wheel.delta as f32 / 120.0),
                    modifiers,
                });
                self.dirty = true;
            }
            NativeEvent::MouseLeave => {
                self.pointer_pos = None;
                self.pending_events.push(egui::Event::PointerGone);
                self.dirty = true;
            }
            // 内部処理イベント (presenter thread が直接消費する)。overlay には流さない。
            NativeEvent::GeometryChanged { .. }
            | NativeEvent::DpiChanged { .. }
            | NativeEvent::RequestRaiseHud => {}
        }
    }

    fn update_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        limiter_ceiling_hit_seq: u64,
        playback_speed: f64,
        frame_step_active: bool,
        initial_pause_controls_pending: bool,
        is_seeking: bool,
        seek_serial: u64,
    ) {
        let position_secs = finite_nonnegative(position_secs);
        let duration_secs = finite_nonnegative(duration_secs);
        let volume = finite_video_volume(volume);
        let playback_speed = crate::video::clock::clamp_playback_speed(playback_speed);
        let now = Instant::now();
        let duration_changed = (self.video_duration_secs - duration_secs).abs() > 0.001;
        let position_changed = (self.video_position_secs - position_secs).abs() >= 0.25;
        let playing_changed = self.video_is_playing != is_playing;
        let volume_changed = (self.video_volume - volume).abs() >= 0.005;
        let muted_changed = self.video_muted != muted;
        let limiter_changed = self.video_limiter_ceiling_hit_seq != limiter_ceiling_hit_seq;
        let speed_changed = (self.video_playback_speed - playback_speed).abs() >= 1.0e-6;
        let frame_step_changed = self.video_frame_step_active != frame_step_active;
        let initial_pause_changed =
            self.initial_pause_controls_pending != initial_pause_controls_pending;
        let seeking_changed = self.video_is_seeking != is_seeking;
        let seek_serial_changed = self.video_seek_serial != seek_serial;
        if !is_playing {
            self.perf_pause_gap_pending = true;
        }
        if seeking_changed || (is_seeking && seek_serial_changed) {
            if is_seeking {
                self.seek_status_started_at = Some(now);
            } else {
                self.seek_status_started_at = None;
            }
        }
        self.video_position_secs = position_secs;
        self.video_duration_secs = duration_secs;
        self.video_is_playing = is_playing;
        self.video_volume = volume;
        self.video_muted = muted;
        if limiter_changed {
            if limiter_ceiling_hit_seq > self.video_limiter_ceiling_hit_seq {
                self.video_limiter_visible_until = now.checked_add(LIMITER_INDICATOR_VISIBLE);
            } else {
                self.video_limiter_visible_until = None;
            }
        }
        self.video_limiter_ceiling_hit_seq = limiter_ceiling_hit_seq;
        self.video_playback_speed = playback_speed;
        self.video_frame_step_active = frame_step_active;
        self.initial_pause_controls_pending = initial_pause_controls_pending;
        self.video_is_seeking = is_seeking;
        self.video_seek_serial = seek_serial;
        if speed_changed {
            // 速度変更前のサンプルは旧 playback_speed のまま `perf_history` に残るが、
            // Y 軸スケールと gap 判定は最新サンプル群の median から導出されるため、
            // 過渡期に旧サンプルが新スケール基準で再解釈されて色がちらつく。新速度
            // で素のグラフから始めるためにクリアする。
            self.perf_history.clear();
            self.perf_pause_gap_pending = false;
        }
        if duration_changed
            || position_changed
            || playing_changed
            || volume_changed
            || muted_changed
            || limiter_changed
            || speed_changed
            || frame_step_changed
            || initial_pause_changed
            || seeking_changed
            || seek_serial_changed
        {
            self.dirty = true;
        }
        self.schedule_seek_status_repaint(now);
        self.schedule_limiter_indicator_repaint(now);
    }

    fn native_pos(&self, x: i32, y: i32) -> egui::Pos2 {
        egui::pos2(
            x as f32 / self.pixels_per_point,
            y as f32 / self.pixels_per_point,
        )
    }

    fn set_perf_visible(&mut self, visible: bool) -> bool {
        if self.perf_visible == visible {
            return false;
        }
        self.perf_visible = visible;
        self.dirty = true;
        true
    }

    fn push_perf_sample(
        &mut self,
        mut sample: NativeOverlayPerfSample,
        snapshot: NativeOverlayPerfSnapshot,
    ) {
        if let Some(prev) = self.perf_history.back() {
            // synthetic arrival は実時間ベースの間隔で前進させる必要がある。
            // 0.5x 再生では実 interval は source_delta_ms の 2 倍 = 横スクロールも
            // 2 倍遅くなるべき。speed-adjusted な effective interval を使う。
            let expected_ms = native_perf_expected_frame_ms_from_samples([*prev, sample])
                .unwrap_or_else(|| {
                    let sample_eff = native_perf_effective_interval_ms(&sample);
                    if sample_eff.is_finite() && sample_eff > 0.5 {
                        sample_eff
                    } else if prev.interval_ms.is_finite() && prev.interval_ms > 0.5 {
                        prev.interval_ms
                    } else {
                        16.67
                    }
                });
            sample.arrival =
                prev.arrival + Duration::from_secs_f32((expected_ms / 1000.0).clamp(0.001, 0.5));
            if self.perf_pause_gap_pending {
                sample.interval_ms = expected_ms;
            }
        }
        self.perf_pause_gap_pending = false;
        self.perf_latest = snapshot;
        self.perf_history.push_back(sample);
        while self.perf_history.len() > 1400 {
            self.perf_history.pop_front();
        }
        while self.perf_history.front().is_some_and(|front| {
            sample
                .arrival
                .saturating_duration_since(front.arrival)
                .as_secs_f32()
                > 6.5
        }) {
            self.perf_history.pop_front();
        }
        if self.perf_visible && self.perf_last_dirty.elapsed() >= Duration::from_millis(100) {
            self.perf_last_dirty = Instant::now();
            self.dirty = true;
        }
    }

    fn set_hover_thumbnail(&mut self, thumbnail: Option<NativeOverlayThumbnail>) {
        let new_key = thumbnail
            .as_ref()
            .map(|t| (t.width, t.height, thumbnail_rgba_key(t)));
        let old_key = self
            .hover_thumbnail
            .as_ref()
            .map(|t| (t.width, t.height, thumbnail_rgba_key(t)));
        if new_key == old_key {
            return;
        }
        self.hover_thumbnail = thumbnail;
        if self.hover_thumbnail.is_none() {
            self.hover_texture = None;
            self.hover_texture_key = None;
        }
        self.dirty = true;
    }

    fn set_hover_preview_pinned(&mut self, pinned: bool) {
        if self.hover_preview_pinned == pinned {
            return;
        }
        self.hover_preview_pinned = pinned;
        self.dirty = true;
    }

    fn set_timeline_markers(&mut self, markers: Vec<NativeOverlayTimelineMarker>) {
        if timeline_markers_match(&self.timeline_markers, &markers) {
            return;
        }
        self.timeline_markers = markers;
        self.dirty = true;
    }

    fn set_jump_entries(&mut self, entries: Vec<NativeOverlayJumpEntry>) {
        if jump_entries_match(&self.jump_entries, &entries) {
            return;
        }
        self.jump_entries = entries;
        self.dirty = true;
    }

    fn set_metadata(&mut self, metadata: Option<NativeOverlayMetadata>) {
        if self.video_metadata == metadata {
            return;
        }
        self.video_metadata = metadata;
        self.dirty = true;
    }

    fn set_normalize_state(&mut self, state: crate::video::normalize_types::NormalizeOverlayState) {
        if self.normalize_state == state {
            return;
        }
        self.normalize_state = state;
        self.dirty = true;
    }

    fn set_loop_enabled(&mut self, enabled: bool) {
        if self.video_loop_enabled == enabled {
            return;
        }
        self.video_loop_enabled = enabled;
        self.dirty = true;
    }

    fn set_loop_mode(&mut self, mode: crate::settings::VideoLoopMode) {
        if self.video_loop_mode == mode {
            return;
        }
        self.video_loop_mode = mode;
        self.dirty = true;
    }

    fn set_checked(&mut self, checked: bool) {
        if self.video_checked == checked {
            return;
        }
        self.video_checked = checked;
        self.dirty = true;
    }

    fn set_vst3_available(&mut self, available: bool) {
        if self.vst3_available == available {
            return;
        }
        self.vst3_available = available;
        if !available {
            self.vst3_panel = None;
            self.last_emitted_vst3_panel_pos = None;
        }
        self.dirty = true;
    }

    fn set_vst3_panel(&mut self, panel: Option<NativeOverlayVst3Panel>) {
        if self.vst3_panel == panel {
            return;
        }
        let old_pos = self.vst3_panel.as_ref().and_then(|panel| panel.panel_pos);
        let new_pos = panel.as_ref().and_then(|panel| panel.panel_pos);
        if old_pos != new_pos {
            self.last_emitted_vst3_panel_pos = None;
        }
        self.vst3_panel = panel;
        self.dirty = true;
    }

    fn set_playback_status(
        &mut self,
        first_frame_presented: bool,
        error: Option<String>,
        prep_status: crate::video::avio_progress::PreparingStatus,
    ) {
        // first_frame_presented / error が変わらなくても、準備中フェーズの数値
        // (bytes_read など) は毎 tick 増えるので、`first_frame_presented = false`
        // の間は常に dirty を立てて再描画する。
        let prep_changed = !first_frame_presented
            && (self.preparing_status.phase != prep_status.phase
                || self.preparing_status.bytes_read != prep_status.bytes_read
                || self.preparing_status.file_size != prep_status.file_size);
        let other_changed =
            self.first_frame_presented != first_frame_presented || self.video_error != error;
        if !other_changed && !prep_changed {
            return;
        }
        self.first_frame_presented = first_frame_presented;
        self.video_error = error;
        self.preparing_status = prep_status;
        self.dirty = true;
    }

    fn seek_status_visible_at(&self, now: Instant) -> bool {
        seek_status_visible_for_times(
            self.video_is_seeking,
            self.seek_status_started_at,
            self.seek_status_visible_since,
            now,
        )
    }

    fn schedule_seek_status_repaint(&mut self, now: Instant) {
        let seek_delay_deadline = self
            .video_is_seeking
            .then(|| {
                self.seek_status_started_at
                    .and_then(|started| started.checked_add(SEEK_STATUS_DELAY))
                    .filter(|deadline| *deadline > now)
            })
            .flatten();
        let hold_deadline = self
            .seek_status_visible_since
            .and_then(|shown| shown.checked_add(SEEK_STATUS_MIN_VISIBLE))
            .filter(|deadline| *deadline > now);
        let deadline = match (seek_delay_deadline, hold_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        if let Some(deadline) = deadline {
            self.next_repaint_deadline = Some(
                self.next_repaint_deadline
                    .map_or(deadline, |current| current.min(deadline)),
            );
        }
    }

    fn limiter_indicator_visible_at(&mut self, now: Instant) -> bool {
        if let Some(until) = self.video_limiter_visible_until {
            if now < until {
                return true;
            }
            self.video_limiter_visible_until = None;
            self.dirty = true;
        }
        false
    }

    fn schedule_limiter_indicator_repaint(&mut self, now: Instant) {
        if let Some(deadline) = self
            .video_limiter_visible_until
            .filter(|deadline| *deadline > now)
        {
            self.next_repaint_deadline = Some(
                self.next_repaint_deadline
                    .map_or(deadline, |current| current.min(deadline)),
            );
        }
    }

    fn update_seek_status_for_render(&mut self, now: Instant) -> bool {
        let active_after_delay = self.video_is_seeking
            && self
                .seek_status_started_at
                .is_some_and(|started| now.duration_since(started) >= SEEK_STATUS_DELAY);
        let visible_hold_expired = self
            .seek_status_visible_since
            .is_some_and(|shown| now.duration_since(shown) >= SEEK_STATUS_MIN_VISIBLE);
        if !active_after_delay && visible_hold_expired {
            self.seek_status_visible_since = None;
        }
        if active_after_delay {
            // Keep this fresh while the slow seek is visible so completion gets the
            // full minimum hold instead of expiring from the first visible frame.
            self.seek_status_visible_since = Some(now);
        }
        let visible = self.seek_status_visible_at(now);
        self.seek_status_visible = visible;
        self.schedule_seek_status_repaint(now);
        visible
    }

    /// eframe 経由でルーティングされた活動 (Space で pause/resume 等) を反映する。
    /// `push_native_event` を経由しない経路用の明示的な活動通知。
    /// `dirty = true` を立てて次フレームで `render_once` を強制実行し、
    /// `update_cursor_icon` を更新カーソルで上書きする (= 隠れていたら再表示)。
    fn mark_cursor_activity(&mut self) {
        self.cursor_last_activity = Some(Instant::now());
        self.cursor_hidden = false;
        self.dirty = true;
    }

    fn show_toast(&mut self, text: String, centered: bool, linger: Option<Duration>) {
        if text.trim().is_empty() {
            return;
        }
        let linger =
            linger.unwrap_or_else(|| Duration::from_millis(if centered { 2500 } else { 1800 }));
        self.toast = Some(NativeOverlayToast {
            text,
            started_at: Instant::now(),
            centered,
            linger,
        });
        self.dirty = true;
    }

    fn set_tile_overlay(&mut self, tile_overlay: Option<NativeOverlayTileOverlay>) {
        if self.tile_overlay.is_none() && tile_overlay.is_none() {
            return;
        }
        if tile_overlay.is_none() {
            self.tile_textures.clear();
        }
        self.tile_overlay = tile_overlay;
        self.dirty = true;
    }

    fn sync_hover_thumbnail_texture(&mut self) {
        let Some(thumbnail) = self.hover_thumbnail.as_ref() else {
            self.hover_texture = None;
            self.hover_texture_key = None;
            return;
        };
        let key = (
            thumbnail.width,
            thumbnail.height,
            thumbnail_rgba_key(thumbnail),
        );
        if self.hover_texture_key == Some(key) {
            return;
        }
        let size = [thumbnail.width as usize, thumbnail.height as usize];
        if size[0] == 0 || size[1] == 0 || thumbnail.rgba.len() != size[0] * size[1] * 4 {
            return;
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, thumbnail.rgba.as_ref());
        let texture = self.egui_ctx.load_texture(
            "native_seek_hover_thumbnail",
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.hover_texture = Some(texture);
        self.hover_texture_key = Some(key);
    }

    fn sync_tile_overlay_textures(&mut self) {
        let Some(tile_overlay) = self.tile_overlay.as_ref() else {
            self.tile_textures.clear();
            return;
        };
        self.tile_textures
            .retain(|idx, _| tile_overlay.tiles.get(*idx).is_some_and(Option::is_some));
        for (idx, slot) in tile_overlay.tiles.iter().enumerate() {
            let Some(tile) = slot.as_ref() else {
                self.tile_textures.remove(&idx);
                continue;
            };
            let key = tile.target_secs.to_bits() ^ ((tile.width as u64) << 32) ^ tile.height as u64;
            if self
                .tile_textures
                .get(&idx)
                .is_some_and(|(cached_key, _)| *cached_key == key)
            {
                continue;
            }
            let size = [tile.width as usize, tile.height as usize];
            if size[0] == 0 || size[1] == 0 || tile.rgba.len() != size[0] * size[1] * 4 {
                continue;
            }
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, tile.rgba.as_ref());
            let texture = self.egui_ctx.load_texture(
                format!("native_video_tile:{idx}"),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.tile_textures.insert(idx, (key, texture));
        }
    }

    fn sync_jump_entry_textures(&mut self) {
        self.jump_textures
            .retain(|idx, _| self.jump_entries.get(*idx).is_some());
        for (idx, entry) in self.jump_entries.iter().enumerate() {
            let Some(thumbnail) = entry.thumbnail.as_ref() else {
                continue;
            };
            let key = thumbnail_rgba_key(thumbnail)
                ^ ((thumbnail.width as u64) << 32)
                ^ thumbnail.height as u64;
            if self
                .jump_textures
                .get(&idx)
                .is_some_and(|(cached_key, _)| *cached_key == key)
            {
                continue;
            }
            let size = [thumbnail.width as usize, thumbnail.height as usize];
            if size[0] == 0 || size[1] == 0 || thumbnail.rgba.len() != size[0] * size[1] * 4 {
                continue;
            }
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied(size, thumbnail.rgba.as_ref());
            let texture = self.egui_ctx.load_texture(
                format!("native_video_jump:{idx}"),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.jump_textures.insert(idx, (key, texture));
        }
    }

    fn wants_periodic_tick(&self) -> bool {
        self.hud_visible()
            || self.jump_panel_visible()
            || self.top_bar_visible()
            || self.right_panel_visible()
            || self.vst3_panel_visible()
            || self.perf_visible
            || self.tile_overlay.is_some()
            || self.hover_preview_target_secs.is_some()
            || self.frame_step_hold.is_some()
            || self.bookmark_title_edit.is_some()
            || self.toast.is_some()
            || self.video_error.is_some()
            || !self.first_frame_presented
            || self.video_is_seeking
            || self.seek_status_visible_since.is_some()
            || self.video_limiter_visible_until.is_some()
            // カーソル auto-hide のカウントダウン中、および 3 秒経過後でもまだ
            // SetCursor(None) を 1 度も打てていない場合は tick を継続する。
            // - cursor_last_activity が Some && !cursor_hidden:
            //   - idle < 3 秒: 経過判定を進めるため tick
            //   - idle >= 3 秒: 1 回 render_once を走らせて SetCursor(None) を打つため tick
            //     (250 ms tick 間隔ぴったりに 3 秒境界で wants_periodic_tick が false に
            //     なって render が走らずカーソルが消えない、というバグを防ぐ)
            // - cursor_hidden = true: 既に隠した → 次の活動 / overlay 表示まで tick 不要。
            //   push_native_event 側で活動検出時に cursor_hidden を false に戻して tick を再開する。
            || (self.cursor_last_activity.is_some() && !self.cursor_hidden)
    }

    fn repaint_due(&self, now: Instant) -> bool {
        self.next_repaint_deadline
            .is_some_and(|deadline| now >= deadline)
    }

    fn has_pointer_pos(&self) -> bool {
        self.pointer_pos.is_some()
    }

    fn needs_synthetic_pointer_move(&self, x: i32, y: i32) -> bool {
        let pos = self.native_pos(x, y);
        self.pointer_pos
            .is_none_or(|prev| prev.distance_sq(pos) > 0.25)
    }

    fn hover_tooltip_repaint_needed(&self) -> bool {
        self.pointer_pos.is_some()
            && (self.hud_visible()
                || self.top_bar_visible()
                || self.right_panel_visible()
                || self.jump_panel_visible()
                || self.vst3_panel_visible()
                || self.video_speed_popup_open
                || self.hover_preview_target_secs.is_some())
    }

    fn needs_render(&self) -> bool {
        self.dirty || !self.pending_events.is_empty()
    }

    fn render_if_dirty(&mut self) -> Result<NativeOverlayInputOutcome, String> {
        let hover_tooltip_repaint_needed = self.hover_tooltip_repaint_needed();
        if !self.dirty && self.pending_events.is_empty() && !hover_tooltip_repaint_needed {
            // CP5: dirty フラグ無しでも HUD regions は最新の visibility に合わせて
            // 出しておく (= 例えば mouse hover で bar が消えても、その後の VST z-order
            // 変更で再 raise されたとき HUD HWND の region がきちんと「穴」になっている
            // 必要があるため)。dirty 無しの場合は render はしないが regions だけ更新。
            return Ok(NativeOverlayInputOutcome {
                routing: self.input_routing(),
                commands: Vec::new(),
                hud_regions: self.compute_hud_regions(),
            });
        }
        // egui tooltip は hover 後に `request_repaint_after(tooltip_delay)` で開く。
        // native overlay には eframe の repaint callback が無いため、hover UI 表示中は
        // periodic tick で egui pass も回して delay 到達を拾う。
        let commands = self.render_once()?;
        // overlay が wheel を WheelNavigate / TileColumnsDelta に変換したフレームでは、
        // 同じ raw wheel イベントを App へ二重転送しないよう routing に印を付ける。
        let consumed_wheel = commands.iter().any(|c| {
            matches!(
                c,
                NativeOverlayCommand::WheelNavigate { .. }
                    | NativeOverlayCommand::TileColumnsDelta { .. }
            )
        });
        let mut routing = self.input_routing();
        routing.consumed_wheel = consumed_wheel;
        Ok(NativeOverlayInputOutcome {
            routing,
            commands,
            hud_regions: self.compute_hud_regions(),
        })
    }

    /// CP5: 現在表示中の overlay UI 要素の物理ピクセル RECT を集める。
    /// `apply_hud_regions(&regions)` で `SetWindowRgn` に渡されて、HUD HWND の
    /// 物理形状が更新される。
    ///
    /// **含めるもの (= 表示される UI rect。クリック可能でない受動インジケータも含む)**:
    /// - 上 hover bar (`top_bar_visible()`): 画面上端 0..76pt 帯
    /// - 下 HUD (`hud_visible()`): 画面下端 (H-220)..H pt 帯
    /// - right panel (`right_panel_visible()`): 画面右端の panel rect
    /// - jump panel (`jump_panel_visible`): 画面下半分の jump rect
    /// - VST3 panel (`vst3_panel_visible()`): center modal panel
    /// - speed popup (`video_speed_popup_open`): 画面下端の popup
    /// - bookmark title editor (`bookmark_title_edit.is_some()`): center modal
    /// - normalize progress / scan UI (`normalize_state` の各 phase)
    /// - tile overlay (`tile_overlay.is_some()`): 全画面 tile grid
    /// - paused center indicator (paused かつ表示中)
    /// - seek hover thumbnail + pin/bookmark (`hover_thumbnail.is_some()`)
    /// - checkmark indicator (`video_checked`)
    ///
    /// **含めないもの**:
    /// - activation zone (= bar 非表示状態の hover 検出範囲) — VST のノブやメニューが
    ///   上下端に重なったとき入力を奪わないため (Codex 5 P1 #1)。hover 検出は CP6 の
    ///   presenter polling 経路で代替。
    ///
    /// **`capture_all` フラグ**: egui `pointer.any_down() && wants_pointer_input` が
    /// true のフレーム (= drag 中) は全画面 RECT に置換して drag 維持。
    ///
    /// CP5 段階では各 UI 要素の正確な egui `response.rect` を持っていないため、
    /// **概算 RECT** (= 既知の固定高さ帯) で実装する。CP7 で有効化したあとの実機検証で
    /// rect ずれが出たら egui レイアウト結果から rect を引いてくる方式に補修する。
    fn compute_hud_regions(&self) -> Vec<RECT> {
        let ppp = self.pixels_per_point.max(1.0);
        let width_px = self.width.max(1) as i32;
        let height_px = self.height.max(1) as i32;

        // capture_all: drag 中はクリックを HUD 経由で受け取りたいので region を画面全体に。
        let pointer_down = self.egui_ctx.input(|i| i.pointer.any_down());
        if pointer_down && self.wants_pointer_input {
            return vec![RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            }];
        }

        let mut regions: Vec<RECT> = Vec::new();
        let to_px = |pt: f32| -> i32 { (pt * ppp).round() as i32 };
        let width_points = (self.width as f32 / ppp).max(1.0);
        let height_points = (self.height as f32 / ppp).max(1.0);
        // Codex CP9 実機 P1 #3 反映: egui::Rect → physical RECT 変換 helper。
        // panel 概算値ではなく `overlay_draw::native_*_rect` を直接物理ピクセルに変換することで、
        // 実 UI rect と region が一致して境界振動を起こさない。
        let rect_to_px = |r: egui::Rect| -> RECT {
            let left = (r.min.x * ppp).round() as i32;
            let top = (r.min.y * ppp).round() as i32;
            let right = (r.max.x * ppp).round() as i32;
            let bottom = (r.max.y * ppp).round() as i32;
            RECT {
                left: left.max(0).min(width_px),
                top: top.max(0).min(height_px),
                right: right.max(0).min(width_px),
                bottom: bottom.max(0).min(height_px),
            }
        };

        // 描画側の visibility 判定をローカルで再現 (`mod.rs:3084` 周辺の `render_once` と整合)。
        // tile overlay 表示中は通常 HUD UI が非表示 (= tile grid モード) なので region も別系統。
        let tile_overlay_visible = self.tile_overlay.is_some();
        let top_bar_visible_flag = self.top_bar_visible;
        let right_panel_visible_flag = self.right_panel_visible();
        let jump_panel_visible_flag = self.jump_panel_visible;
        let side_panel_visible =
            !tile_overlay_visible && (jump_panel_visible_flag || right_panel_visible_flag);
        let panel_chrome_visible =
            !tile_overlay_visible && (top_bar_visible_flag || side_panel_visible);
        let normalize_scanning = matches!(
            self.normalize_state.ui_state,
            crate::video::normalize_types::NormalizeUiState::Scanning
        );
        let raw_seek_status_visible = self.seek_status_visible
            && !tile_overlay_visible
            && self.first_frame_presented
            && self.video_error.is_none();
        // paused_center_visible (= 中央 replay/play ボタンが見えている) も描画側と一致させる
        // (Codex CP5 P2 #2): paused 中の中央ボタンクリックが VST に抜けないように。
        let paused_center_visible =
            center_pause_controls_visible_for_state(CenterPauseControlsInputs {
                tile_overlay_visible,
                is_playing: self.video_is_playing,
                frame_step_active: self.video_frame_step_active,
                initial_pause_controls_pending: self.initial_pause_controls_pending,
                first_frame_presented: self.first_frame_presented,
                has_video_error: self.video_error.is_some(),
                normalize_scanning,
            });
        let seek_status_visible =
            center_seek_status_visible(raw_seek_status_visible, paused_center_visible);
        let status_visible = !tile_overlay_visible
            && (self.video_error.is_some() || !self.first_frame_presented || seek_status_visible);
        // **bottom_hud_visible** は描画側 (`render_once`) と完全一致させる。
        // CP5 旧版は `hud_visible()` 単独だったが、Codex CP5 P2 #1 で「top bar や
        // side panel 表示中も bottom HUD が描かれるのに region に下端帯がないと
        // クリックが奪われる」問題を指摘されたので panel_chrome_visible も含める。
        // paused center 表示中も下 HUD を常時 region に入れ、初回 click の hit-test
        // 前フレーム未登録問題で mute / seek が無反応になるのを防ぐ。
        let bottom_hud_visible =
            self.hud_visible() || panel_chrome_visible || paused_center_visible;

        // 上 hover bar (= 実描画 54pt = overlay_draw:1546 と一致)。
        //
        // ## region サイズの選択 (Codex 2026-05-12 P1 反映)
        //
        // **実描画 rect (54pt) だけ**を region に入れる。**活性化 zone (76pt) は region に
        // 入れない** (= polling / presenter wndproc 経由で pointer_pos が更新される経路に任せる)。
        //
        // ### なぜ実描画だけか
        //
        // `SetWindowRgn` で region に入れた領域は **DComp 透過でも HWND が入力を取る**
        // (= Codex 指摘の核心)。旧版で 180pt 入れていたのは「pointer が region 外に出ると
        // WM_MOUSEMOVE が HUD wndproc に届かないので bar 表示が振動する」懸念だったが、
        // 実際は **presenter HWND の wndproc も WM_MOUSEMOVE を `NativeEguiOverlay` に
        // push している** (presenter HWND は fullscreen 全画面で region 全域)。region 外でも
        // egui の pointer_pos は更新され続け、`top_bar_visible()` の hover 判定
        // (`pos.y <= 76`) は維持される。
        //
        // 旧版で region 内に「描画されない 126pt 帯」(= 54pt〜180pt) を入れていたのは、
        // この 126pt 帯 = VST の上部ヘッダ・タイトルバーが押せない原因だった (= ユーザー
        // 報告「VST 上端をドラッグして戻せない」「top bar 表示中に VST のヘッダクリックが
        // 効かない」)。
        if top_bar_visible_flag {
            regions.push(RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: to_px(54.0).min(height_px),
            });
        }

        // 下 HUD (seek bar + コントロール) 表示中。**実描画 46pt 帯** (= overlay_draw:3797
        // `fixed_pos(0, height-46)` + `set_min_size(W, 46)` と一致)。
        //
        // ## region サイズの選択 (Codex 2026-05-12 P1 反映)
        //
        // 旧版は 220pt = `hud_visible()` の活性化 zone と同じだったが、これは hover 検出用
        // しきい値であって描画 zone ではない。220pt 入れていた 174pt 余分帯 (= 46pt〜220pt) は
        // VST のフッタ・下半分ボタン・キー操作領域が押せない原因だった (= ユーザー報告
        // 「下半分の VST ボタンが押せない」の主因)。
        // 活性化判定は presenter wndproc 経由の pointer_pos で維持されるので region は不要。
        if bottom_hud_visible {
            let bottom_band_top = (height_px - to_px(46.0)).max(0);
            regions.push(RECT {
                left: 0,
                top: bottom_band_top,
                right: width_px,
                bottom: height_px,
            });
        }

        // Center status (error / preparing / slow seek): `draw_native_center_status`
        // と同じ box サイズを region に含め、HUD HWND の SetWindowRgn でクリップされないようにする。
        if status_visible {
            let has_body = self.video_error.is_some();
            let title = if has_body {
                "動画を再生できません".to_owned()
            } else if !self.first_frame_presented {
                crate::video::avio_progress::build_preparing_message(self.preparing_status)
            } else {
                "シーク中...".to_owned()
            };
            let available_w = (width_points - 48.0).max(120.0);
            let box_w = if has_body {
                width_points.clamp(360.0, 720.0).min(available_w)
            } else {
                let text_w = title.chars().count() as f32 * 18.0 + 72.0;
                text_w.clamp(180.0, 420.0).min(available_w)
            };
            let box_h = if has_body { 132.0 } else { 62.0 };
            regions.push(rect_to_px(egui::Rect::from_center_size(
                egui::pos2(width_points * 0.5, height_points * 0.5),
                egui::vec2(box_w, box_h),
            )));
        }

        // Right panel 表示中: `native_metadata_panel_rect` の実 rect を使う
        // (= 幅 430pt 上限、top=56pt から hover_bottom まで、Codex CP9 実機 P1 #3 反映)。
        if right_panel_visible_flag {
            regions.push(rect_to_px(self::overlay_draw::native_metadata_panel_rect(
                width_points,
                height_points,
            )));
        }

        // Jump panel 表示中: `native_jump_panel_rect` の実 rect を使う
        // (= 幅 320pt、top=56pt から hover_bottom まで、画面左端起点)。
        if jump_panel_visible_flag {
            regions.push(rect_to_px(self::overlay_draw::native_jump_panel_rect(
                height_points,
            )));
        }

        // 実機修正 (2026-05-12 P2): Perf overlay 表示中は perf rect を region に
        // 追加 (= ユーザー報告「Perlグラフが panel に重なる部分しか見えない」対応)。
        // perf overlay は `origin=(14, 14)` で width 300-460pt、height 158pt
        // (`overlay_draw.rs:12-26` 参照)。region 外だと SetWindowRgn で clip される。
        if self.perf_visible {
            let perf_w = width_points.min(460.0).max(300.0);
            regions.push(rect_to_px(egui::Rect::from_min_size(
                egui::pos2(14.0, 14.0),
                egui::vec2(perf_w, 158.0),
            )));
        }

        // Checkmark indicator: passive UI だが、HUD HWND は SetWindowRgn 外の DComp
        // 描画も OS 側で clip する。右パネル等の region が重なった時だけ見える
        // regression を避けるため、描画側と同じ rect を小さく追加する。
        if self.video_checked && !tile_overlay_visible {
            let top = if panel_chrome_visible { 68.0 } else { 28.0 };
            regions.push(rect_to_px(self::overlay_draw::native_checkmark_rect(
                width_points,
                top,
            )));
        }

        // Paused center indicator (replay / play ボタン + ラベル backdrop): 実描画 rect を使う。
        // Codex CP5 P2 #2 反映: paused 中の中央ボタンが VST と重なるケースで
        // クリックが奪われないよう region に追加。
        //
        // 旧版は 200×200pt 固定だったが、実 UI は両ボタンだけで横幅 258pt (= 2×112 + 34)、
        // ヒント背景は center_y+124pt まで広がるため、SetWindowRgn でクリップされて
        // 「ボタンが変に crop される」症状になっていた (= 2026-05-12 ユーザー報告)。
        // `draw_native_center_pause_controls` が実描画した個別 rect (replay/play/backdrop、
        // 各 +4pt margin) を `last_drawn_paused_center_rects` 経由で受け取り、それぞれを
        // region に push する (union で 1 矩形にせず個別で入れることで、ヒント横幅に
        // 引っ張られた不要な HUD 入力帯を作らず VST 入力干渉を最小化)。
        //
        // `paused_center_visible` ガードを残すことで、直前フレームの stale rect が
        // 不可視に切り替わった瞬間に region から外れる。初回フレーム fallback として、
        // visible だが rect 未記録のケース (= ダイアログ表示直後で先に region 計算が
        // 走るパス) では広めの保険 box を入れて次フレームの実描画 rect 更新を待つ。
        if paused_center_visible {
            if let Some(rects) = self.last_drawn_paused_center_rects.as_ref() {
                for rect in rects.iter() {
                    regions.push(rect_to_px(*rect));
                }
            } else {
                // 初回フレーム fallback: 横幅はヒント文字列 (約 40 字、推定 500pt 程度) +
                // backdrop expand を覆うために 640pt 確保。縦は radius=56, hint_y=center_y+108 を
                // 元に上 -64pt〜下 +140pt。次フレームの egui run で正確な個別 rect 群に
                // 置き換わるので 1 フレームだけ広めでも VST 干渉はごく短時間。
                let cx = width_px / 2;
                let cy = height_px / 2;
                let half_w = to_px(320.0);
                regions.push(RECT {
                    left: (cx - half_w).max(0),
                    top: (cy - to_px(64.0)).max(0),
                    right: (cx + half_w).min(width_px),
                    bottom: (cy + to_px(140.0)).min(height_px),
                });
            }
        }

        if self.toast.is_some() {
            if let Some(rect) = self.last_drawn_toast_rect {
                regions.push(rect_to_px(rect));
            } else {
                // 初回フレーム fallback: draw_native_toast が actual rect を記録する前に
                // region 計算だけ走る場合でも、toast が他 UI の region だけに切り抜かれて
                // 表示されないよう中央帯を確保する。次回描画後は実 rect に置き換わる。
                let toast_w = width_points.min(760.0).max(320.0);
                let toast_h = 92.0;
                regions.push(rect_to_px(egui::Rect::from_center_size(
                    egui::pos2(width_points * 0.5, height_points * 0.5),
                    egui::vec2(toast_w, toast_h),
                )));
            }
        }

        // VST3 panel: ドラッグ可能化 (= 2026-05-12 A) に伴い、`last_drawn_vst3_panel_rect`
        // (実描画後の actual rect) を優先で region に使う。`None` の場合は `native_vst3_panel_rect`
        // (デフォルト位置) に fallback。これで panel がデフォルト位置にあってもドラッグ後でも
        // region が描画位置と一致する。
        if let Some(panel) = self.vst3_panel.as_ref() {
            if panel.visible {
                let rect = self.last_drawn_vst3_panel_rect.unwrap_or_else(|| {
                    self::overlay_draw::native_vst3_panel_rect(width_points, height_points, panel)
                });
                regions.push(rect_to_px(rect));
            }
        }

        if self.video_speed_popup_open {
            let rect = self.last_drawn_speed_popup_rect.unwrap_or_else(|| {
                let popup_w = 356.0_f32.min((width_points - 16.0).max(180.0));
                let popup_h = 74.0;
                let popup_x = (width_points - popup_w - 8.0).max(8.0);
                let popup_y = (height_points - 46.0 - popup_h - 6.0).max(8.0);
                egui::Rect::from_min_size(
                    egui::pos2(popup_x, popup_y),
                    egui::vec2(popup_w, popup_h),
                )
            });
            let rect_px = rect_to_px(rect.expand(4.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        // Bookmark title editor: center modal。`draw_native_bookmark_title_editor` の
        // 実描画 rect (`last_drawn_bookmark_editor_rect`) をそのまま region にする。
        //
        // 旧版は画面中心固定の概算 500×100pt だったが、ダイアログは
        // `pos.y = (H - dialog_h) * 0.5` (dialog_h=142 はレイアウト用の過大見積もり) に
        // 配置され、実コンテンツ高さ (~115pt) との差でダイアログ中心が画面中心より
        // 上にずれる。概算 region は画面中心 + 高さ 100pt だったため、ダイアログ上端
        // (=「ブックマーク名」ラベル) が SetWindowRgn でクリップされていた
        // (= 2026-05-14 ユーザー報告「上が欠ける」)。
        if self.bookmark_title_edit.is_some() {
            let rect = self.last_drawn_bookmark_editor_rect.unwrap_or_else(|| {
                // 初回フレーム fallback: 実描画前は draw 側と同じ式で概算する。
                // dialog_h=142 は実コンテンツ高さの過大見積もりなので、この rect は
                // ダイアログ全体を内包する。
                let dialog_w = 360.0_f32.min((width_points - 32.0).max(260.0));
                let dialog_h = 142.0;
                egui::Rect::from_min_size(
                    egui::pos2(
                        (width_points - dialog_w) * 0.5,
                        (height_points - dialog_h) * 0.5,
                    ),
                    egui::vec2(dialog_w, dialog_h),
                )
            });
            let rect_px = rect_to_px(rect.expand(2.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        // Normalize progress / scan blocker: 全画面被覆 (= scan 中はモーダル cancel ボタン操作のため)。
        if matches!(
            self.normalize_state.ui_state,
            crate::video::normalize_types::NormalizeUiState::Scanning
        ) {
            regions.push(RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            });
        }

        // Tile overlay: 全画面 tile grid。
        if self.tile_overlay.is_some() {
            regions.push(RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            });
        }

        // Seek hover thumbnail: **直近 egui run で実描画した rect を直接使う** (Codex 助言
        // 「描画側の preview_rect をそのまま保存して使う」反映、2026-05-12 P1 #2)。
        //
        // ## なぜ ptr.x ベース再計算は NG か
        //
        // 旧版は `compute_hud_regions` 内で `(ptr.x - preview_image_w * 0.5).clamp(...)` で
        // 再計算していたが、実描画は `bar_rect.min.x + bar_rect.width() * frac` (=
        // hover_preview_target_secs ベース) で計算する。両者が乖離するケース:
        //   - cursor が seek bar を離れ thumbnail 上に移動 → `seek_resp.hovered()` 不成立
        //   - `hover_preview_target_secs` は cursor 離脱前の値で固定 (overlay_draw:4170-4171)
        //   - cursor をサムネ上で左右に動かす:
        //     - 描画 rect: target_secs ベースなので **固定**
        //     - region rect: ptr.x ベースなので **cursor 追従**
        //   - 結果: サムネ画像 (= 描画 rect) は固定だが region が動く → region 外に出た部分が
        //     SetWindowRgn で clip されて「枠だけ動いて見える」症状 (= ユーザー報告)
        //
        // `last_drawn_preview_rect` には draw が「実際にこのフレームで描いた rect」が入って
        // いる (= None なら描画なし)。これをそのまま region に変換する。
        if let Some(rect) = self.last_drawn_preview_rect {
            regions.push(rect_to_px(rect));
        }

        // egui tooltip は Order::Tooltip の floating Area として下 HUD / panel の外側に
        // 描かれる。HUD HWND の SetWindowRgn に実 rect を含めないと、DComp には描けても
        // HWND region 外で物理的に clip される。
        let mut tooltip_layers: Vec<egui::LayerId> = self.egui_ctx.memory(|mem| {
            mem.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer_id| layer_id.order == egui::Order::Tooltip)
                .collect()
        });
        tooltip_layers.sort_by_key(|layer_id| layer_id.id.value());
        for layer_id in tooltip_layers {
            if let Some(state) = egui::AreaState::load(&self.egui_ctx, layer_id.id) {
                let rect = state.rect();
                if rect.is_finite() && rect.is_positive() {
                    let rect_px = rect_to_px(rect);
                    if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                        regions.push(rect_px);
                    }
                }
            }
        }

        regions
    }

    fn input_routing(&self) -> NativeOverlayInputRouting {
        NativeOverlayInputRouting {
            wants_pointer_input: self.wants_pointer_input,
            wants_keyboard_input: self.wants_keyboard_input,
            text_input_active: self.text_input_active(),
            // consumed_wheel は commands を見て render_if_dirty 側で設定する。
            ..Default::default()
        }
    }

    fn text_input_active(&self) -> bool {
        self.bookmark_title_edit.is_some()
    }

    fn hud_visible(&self) -> bool {
        // 実機修正 (2026-05-12): 外部 drag (= VST window ドラッグ等) 中は hover bar / panel を
        // 表示しない。さもないと VST を画面下にドラッグしたとき seek bar が出て VST に入力が
        // 届かなくなる。
        if self.external_drag_in_progress {
            return false;
        }
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        self.pointer_pos
            .is_some_and(|pos| pos.y >= (overlay_height_points - 220.0).max(0.0))
    }

    fn top_bar_visible(&self) -> bool {
        // 実機修正 (2026-05-12): 外部 drag 中は top bar を表示しない (= VST を画面上にドラッグ
        // したとき top bar が出てウィンドウを戻せなくなる症状の対応)。
        if self.external_drag_in_progress {
            return false;
        }
        self.pointer_pos.is_some_and(|pos| {
            let y_max = if self.top_bar_visible { 76.0 } else { 36.0 };
            pos.y <= y_max
        })
    }

    fn vst3_panel_visible(&self) -> bool {
        self.vst3_panel.as_ref().is_some_and(|panel| panel.visible)
    }

    fn right_panel_visible(&self) -> bool {
        // 実機修正 (2026-05-12): 外部 drag 中は right panel を表示しない (= VST を画面右に
        // ドラッグしたとき panel が出て VST 入力が奪われる症状の対応)。
        if self.external_drag_in_progress {
            return false;
        }
        if self.vst3_panel_visible() {
            return false;
        }
        if self.video_metadata.is_none() {
            return false;
        }
        if self.video_speed_popup_open || self.hover_preview_target_secs.is_some() {
            return false;
        }
        let Some(pos) = self.pointer_pos else {
            return false;
        };
        let overlay_width_points = self.width as f32 / self.pixels_per_point;
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        let panel_w =
            native_metadata_panel_rect(overlay_width_points, overlay_height_points).width();
        let x_min = overlay_width_points - panel_w;
        native_panel_hover_rect(
            egui::pos2(x_min, 0.0),
            egui::vec2(overlay_width_points - x_min, overlay_height_points),
            overlay_height_points,
        )
        .contains(pos)
    }

    fn jump_panel_visible(&self) -> bool {
        if self.vst3_panel_visible() {
            return false;
        }
        if self.video_speed_popup_open || self.hover_preview_target_secs.is_some() {
            return false;
        }
        let Some(pos) = self.pointer_pos else {
            return false;
        };
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        let x_max = native_jump_panel_width();
        native_panel_hover_rect(
            egui::pos2(0.0, 0.0),
            egui::vec2(x_max, overlay_height_points),
            overlay_height_points,
        )
        .contains(pos)
    }

    fn pointer_over_scroll_panel(&self, pos: egui::Pos2) -> bool {
        let overlay_width_points = self.width as f32 / self.pixels_per_point;
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        (self.jump_panel_visible() && native_jump_panel_rect(overlay_height_points).contains(pos))
            || (self.right_panel_visible()
                && native_metadata_panel_rect(overlay_width_points, overlay_height_points)
                    .contains(pos))
    }

    fn configure(&self) {
        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.format,
                width: self.width,
                height: self.height,
                present_mode: self.present_mode,
                desired_maximum_frame_latency: 1,
                alpha_mode: self.alpha_mode,
                view_formats: vec![],
            },
        );
    }

    fn set_visual_attached(&mut self, attached: bool) -> Result<(), String> {
        if self.visual_attached == attached {
            return Ok(());
        }
        unsafe {
            if attached {
                // CP3 P1 #3 反映: `after_visual` が `Some(v)` なら presenter フォールバック経路
                // (= video visual の後ろに挟む)、`None` なら HUD HWND の DComp root に
                // 単独配置する。後者は HUD root に他の visual がないので `None` で OK。
                let result = match &self.after_visual {
                    Some(v) => self.root_visual.AddVisual(&self.visual, true, v),
                    None => {
                        self.root_visual
                            .AddVisual(&self.visual, true, None::<&IDCompositionVisual>)
                    }
                };
                result
                    .map_err(|e| format!("IDCompositionVisual::AddVisual egui overlay: {e:?}"))?;
            } else {
                self.root_visual.RemoveVisual(&self.visual).map_err(|e| {
                    format!("IDCompositionVisual::RemoveVisual egui overlay: {e:?}")
                })?;
            }
            self.dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit egui overlay visual: {e:?}"))?;
        }
        self.visual_attached = attached;
        log_event(
            "egui_overlay_visual",
            &[("attached", Value::from(self.visual_attached))],
        );
        Ok(())
    }

    fn render_once(&mut self) -> Result<Vec<NativeOverlayCommand>, String> {
        let render_t0 = Instant::now();
        // カーソル auto-hide のタイマを初回フレームで起動する。push_native_event 由来の
        // 入力がまだ届いていなくても、フルスクリーン入場後 CURSOR_HIDE_IDLE_SECS 経過で
        // 隠れるようにするため。
        if self.cursor_last_activity.is_none() {
            self.cursor_last_activity = Some(Instant::now());
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.started_at.elapsed() > toast.linger)
        {
            self.toast = None;
        }
        let seek_status_active = self.update_seek_status_for_render(render_t0);
        self.sync_hover_thumbnail_texture();
        self.sync_tile_overlay_textures();
        self.sync_jump_entry_textures();
        let ppp = self.pixels_per_point;
        let event_count = self.event_count;
        let pointer_pos = self.pointer_pos;
        // hover_preview_target_secs はシークバー Area (= bottom_hud_visible) 内でしか
        // 更新されない。pointer が hud 領域から外れた状態で Some が居座ると
        // jump_panel_visible() / right_panel_visible() を false に固定し続ける
        // (paused_center_visible により overlay_visible 経路の clear も走らない)。
        // hud 領域外なら preview UI 自体描画されないので、ここで先に倒す。
        if self.hover_preview_target_secs.is_some() && !self.hud_visible() {
            self.hover_preview_target_secs = None;
        }
        let overlay_width_points = self.width as f32 / ppp;
        let overlay_height_points = self.height as f32 / ppp;
        let position_secs = self.video_position_secs;
        let duration_secs = self.video_duration_secs;
        let is_playing = self.video_is_playing;
        let frame_step_active = self.video_frame_step_active;
        let volume = self.video_volume;
        let muted = self.video_muted;
        let limiter_ceiling_hit = self.limiter_indicator_visible_at(render_t0);
        let playback_speed = self.video_playback_speed;
        let checked = self.video_checked;
        let loop_enabled = self.video_loop_enabled;
        let loop_mode = self.video_loop_mode;
        let first_frame_presented = self.first_frame_presented;
        let video_error = self.video_error.clone();
        let preparing_status = self.preparing_status;
        let toast = self.toast.clone();
        let hover_thumbnail = self.hover_thumbnail.clone();
        let hover_texture_id = self.hover_texture.as_ref().map(|texture| texture.id());
        let hover_preview_pinned = self.hover_preview_pinned;
        let timeline_markers = self.timeline_markers.clone();
        let jump_entries = self.jump_entries.clone();
        let mut bookmark_title_edit = self.bookmark_title_edit.take();
        let video_metadata = self.video_metadata.clone();
        // 音量ノーマライズ overlay state (Copy 型なので clone 不要)
        let normalize_state_snap = self.normalize_state;
        let vst3_panel = self.vst3_panel.clone();
        let mut last_emitted_vst3_panel_pos = self.last_emitted_vst3_panel_pos;
        let tile_overlay = self.tile_overlay.clone();
        let tile_texture_ids: HashMap<usize, egui::TextureId> = self
            .tile_textures
            .iter()
            .map(|(idx, (_, texture))| (*idx, texture.id()))
            .collect();
        let jump_texture_ids: HashMap<usize, egui::TextureId> = self
            .jump_textures
            .iter()
            .map(|(idx, (_, texture))| (*idx, texture.id()))
            .collect();
        let perf_visible = self.perf_visible;
        let vst3_available = self.vst3_available;
        let vst3_panel_visible = vst3_panel.as_ref().is_some_and(|panel| panel.visible);
        let perf_latest = self.perf_latest;
        let perf_history: Vec<_> = self.perf_history.iter().copied().collect();
        let hud_visible = self.hud_visible();
        let jump_panel_visible = self.jump_panel_visible();
        let top_bar_visible = self.top_bar_visible();
        let right_panel_visible = self.right_panel_visible();
        let tile_overlay_visible = tile_overlay.is_some();
        let raw_seek_status_visible = seek_status_active
            && !tile_overlay_visible
            && first_frame_presented
            && video_error.is_none();
        let toast_visible = toast.is_some();
        let bookmark_title_edit_visible = bookmark_title_edit.is_some();
        let side_panel_visible =
            !tile_overlay_visible && (jump_panel_visible || right_panel_visible);
        let panel_chrome_visible = !tile_overlay_visible && (top_bar_visible || side_panel_visible);
        // Codex P1: Scanning 中は paused_center を出さない (スキャン用に pause している
        // のがバレないため + clickハンドラ TogglePlay/CloseFullscreen を抑制)。
        let normalize_scanning = matches!(
            normalize_state_snap.ui_state,
            crate::video::normalize_types::NormalizeUiState::Scanning
        );
        let paused_center_visible =
            center_pause_controls_visible_for_state(CenterPauseControlsInputs {
                tile_overlay_visible,
                is_playing,
                frame_step_active,
                initial_pause_controls_pending: self.initial_pause_controls_pending,
                first_frame_presented,
                has_video_error: video_error.is_some(),
                normalize_scanning,
            });
        let seek_status_visible =
            center_seek_status_visible(raw_seek_status_visible, paused_center_visible);
        let status_visible = video_error.is_some() || !first_frame_presented || seek_status_visible;
        let bottom_hud_visible = hud_visible || panel_chrome_visible || paused_center_visible;
        let perf_origin = egui::pos2(14.0, 14.0);
        // Codex 2周目 P1: normalize_scanning も overlay_visible / cursor_blocking_overlay_visible
        // に含める。さもないと HUD/Toast 等が出ていない状態で `if !overlay_visible { return; }`
        // 早期 return に入って progress UI まで到達しない。
        let overlay_visible = tile_overlay_visible
            || bottom_hud_visible
            || panel_chrome_visible
            || perf_visible
            || (!tile_overlay_visible && checked)
            || status_visible
            || toast_visible
            || bookmark_title_edit_visible
            || paused_center_visible
            || vst3_panel_visible
            || normalize_scanning;
        // カーソル auto-hide の判定用: チェックマークのような「受動表示」(ユーザーが
        // 操作する対象ではなく単なる状態インジケータ) は countdown をブロックしない。
        // 静止画側 `fs_ui_is_clean` がチェック状態を考慮しないのと挙動を揃える。
        // tile overlay は受動表示の側面が強いが、サムネがクリックで操作可能なので
        // カーソル可視を維持する側に含める。
        //
        // 実機修正 (2026-05-12, Codex 助言 #3): **HUD activation zone (上端 76pt /
        // 下端 220pt)** に cursor が入っていれば auto-hide を抑制する。バーが「ふっと
        // 出る瞬間にカーソルが一瞬消える」症状は、cursor が activation zone に入った
        // フレームと top_bar_visible が true になるフレームが 1 フレームずれて、その間に
        // `cursor_should_hide = true` が成立して `SetCursor(None)` が呼ばれることが原因。
        // activation zone 内では auto-hide を打ち切ることで予防する。
        let in_top_activation_zone = pointer_pos.map(|p| p.y <= 76.0).unwrap_or(false);
        let in_bottom_activation_zone = pointer_pos
            .map(|p| p.y >= overlay_height_points - 220.0)
            .unwrap_or(false);
        let cursor_blocking_overlay_visible = tile_overlay_visible
            || bottom_hud_visible
            || panel_chrome_visible
            || status_visible
            || toast_visible
            || bookmark_title_edit_visible
            || paused_center_visible
            || vst3_panel_visible
            || normalize_scanning
            || in_top_activation_zone
            || in_bottom_activation_zone;
        let pending_event_count = self.pending_events.len();
        let mut commands = std::mem::take(&mut self.pending_overlay_commands);
        let mut last_seek_target_secs = self.last_seek_target_secs;
        let mut last_thumbnail_request_secs = self.last_thumbnail_request_secs;
        let mut last_thumbnail_request_at = self.last_thumbnail_request_at;
        let mut hover_preview_target_secs = self.hover_preview_target_secs;
        let mut video_speed_popup_open = self.video_speed_popup_open;
        let mut frame_step_hold = self.frame_step_hold;
        // 実機修正 (2026-05-12 P1 #2): 実描画した preview_rect を記録して
        // `compute_hud_regions` に渡す (= region を draw rect と完全同期させる)。
        let mut last_drawn_preview_rect: Option<egui::Rect> = None;
        // 実機修正 (2026-05-12 A): VST3 設定パネルをドラッグ可能化 (`.movable(true)`)。
        // ドラッグ後の actual rect を記録して region に追従させる。
        let mut last_drawn_vst3_panel_rect: Option<egui::Rect> = None;
        // 実機修正 (2026-05-12 P2): paused center 制御 UI (replay/play ボタン + label backdrop)
        // の実描画 rect を記録して `compute_hud_regions` に渡す。200×200pt 固定 region では
        // ボタン外側 ~29pt + ヒント帯が SetWindowRgn でクリップされる症状の修正。
        let mut last_drawn_paused_center_rects: Option<[egui::Rect; 3]> = None;
        let mut last_drawn_toast_rect: Option<egui::Rect> = None;
        let mut last_drawn_speed_popup_rect: Option<egui::Rect> = None;
        let mut last_drawn_bookmark_editor_rect: Option<egui::Rect> = None;
        if !overlay_visible {
            self.set_visual_attached(false)?;
            last_seek_target_secs = None;
            last_thumbnail_request_secs = None;
            last_thumbnail_request_at = None;
            hover_preview_target_secs = None;
            frame_step_hold = None;
        }
        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            )),
            time: Some(self.started_at.elapsed().as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.pending_events),
            ..Default::default()
        };
        if let Some(viewport) = raw_input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            if !overlay_visible {
                return;
            }
            // タイルモード中は perf overlay を描かない。タイルグリッドは全画面を
            // 不透明黒で塗るので、grid が `Order::Background`・perf が `Order::Middle`
            // だと perf がグリッドの上に乗ってサムネイルとクリック (seek) を塞いでしまう。
            // 旧実装 (grid = Foreground) でも grid の不透明塗りで perf は隠れていたので、
            // タイルモードで perf を出さないのは元の見た目と一致する。
            if perf_visible && tile_overlay.is_none() {
                draw_native_perf_overlay(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &perf_history,
                    perf_latest,
                    perf_origin,
                );
            }
            if let Some(tile_overlay) = tile_overlay.as_ref() {
                draw_native_tile_overlay(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    tile_overlay,
                    &tile_texture_ids,
                    &mut commands,
                );
                draw_native_top_bar_tile(
                    ctx,
                    overlay_width_points,
                    video_metadata.as_ref(),
                    tile_overlay,
                    &mut commands,
                );
                // タイル表示中も境界トースト (= 「最後の項目です」「次のフォルダが見つかりません」
                // 等) は出す。元実装は早期 return で line 4342 の draw_native_toast に
                // 到達しなかったため、タイル末尾に達してもユーザーへの feedback がゼロだった。
                if let Some(toast) = toast.as_ref() {
                    last_drawn_toast_rect =
                        draw_native_toast(ctx, overlay_width_points, overlay_height_points, toast);
                }
                return;
            }
            if let Some(error) = video_error.as_deref() {
                draw_native_center_status(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    "動画を再生できません",
                    Some(error),
                    true,
                );
            } else if !first_frame_presented {
                // フェーズ + 累積バイト数で文言を切り替え (Codex P2 への補足対応):
                // 「メタデータ読込中... NN MB / YY MB」「ストリーム解析中...」など。
                // moov atom 末尾配置の遅い動画でフリーズと誤認されないようにする。
                let title = crate::video::avio_progress::build_preparing_message(preparing_status);
                draw_native_center_status(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &title,
                    None,
                    false,
                );
            } else if seek_status_visible {
                draw_native_center_status(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    "シーク中...",
                    None,
                    false,
                );
            }
            if panel_chrome_visible {
                draw_native_top_bar(
                    ctx,
                    overlay_width_points,
                    position_secs,
                    duration_secs,
                    video_metadata.as_ref(),
                    perf_visible,
                    vst3_available,
                    vst3_panel_visible,
                    &mut commands,
                );
            }
            if checked {
                draw_native_checkmark(
                    ctx,
                    overlay_width_points,
                    if panel_chrome_visible { 68.0 } else { 28.0 },
                );
            }
            if vst3_panel_visible && let Some(panel) = vst3_panel.as_ref() {
                last_drawn_vst3_panel_rect = draw_native_vst3_panel(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    panel,
                    &mut commands,
                    &mut last_emitted_vst3_panel_pos,
                );
            }
            if paused_center_visible {
                // 「中央パネル外クリックで再開」判定の除外対象として、
                // 同じフレームで実際に描画されている他パネル rect を集める。
                // ホバー判定領域 (top 76 / 下 220 / 左 320 / 右 430) ではなく
                // 描画中の rect (top 54 / 下 46 / jump width 320 / metadata
                // 計算幅 / VST3 計算 rect) を渡すことで、不可視パネルや
                // パネル端のマージンが誤ってデッドゾーン化するのを防ぐ。
                let mut excluded_rects: Vec<egui::Rect> = Vec::new();
                // bookmark 名編集ダイアログ / 速度ポップアップ / シーク hover
                // preview のいずれかが開いている間は、画面のどこをクリックしても
                // TogglePlay/CloseFullscreen を発行しない (= 全画面を除外する)。
                // これらの popup 群は seek HUD の closure 内で動的に layout が
                // 決まり外から rect 計算が困難なうえ、popup 操作中にクリックで
                // 再生再開/フルスクリーン解除が混入すると操作意図と食い違う。
                // popup 自体の dismiss/操作は popup 側ロジックが担うので、
                // 「popup 表示中は overlay 側の click hook を停止」が安全。
                let popup_active = bookmark_title_edit_visible
                    || video_speed_popup_open
                    || hover_preview_target_secs.is_some();
                if popup_active {
                    excluded_rects.push(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(overlay_width_points, overlay_height_points),
                    ));
                }
                if top_bar_visible {
                    excluded_rects.push(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(overlay_width_points, 54.0),
                    ));
                }
                if bottom_hud_visible {
                    excluded_rects.push(egui::Rect::from_min_max(
                        egui::pos2(0.0, (overlay_height_points - 46.0).max(0.0)),
                        egui::pos2(overlay_width_points, overlay_height_points),
                    ));
                }
                if jump_panel_visible {
                    excluded_rects.push(native_jump_panel_rect(overlay_height_points));
                }
                if right_panel_visible {
                    excluded_rects.push(native_metadata_panel_rect(
                        overlay_width_points,
                        overlay_height_points,
                    ));
                }
                if vst3_panel_visible && let Some(panel) = vst3_panel.as_ref() {
                    excluded_rects.push(last_drawn_vst3_panel_rect.unwrap_or_else(|| {
                        native_vst3_panel_rect(overlay_width_points, overlay_height_points, panel)
                    }));
                }
                last_drawn_paused_center_rects = draw_native_center_pause_controls(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &excluded_rects,
                    &mut commands,
                );
            }
            if let Some(toast) = toast.as_ref() {
                last_drawn_toast_rect =
                    draw_native_toast(ctx, overlay_width_points, overlay_height_points, toast);
            }
            if right_panel_visible && let Some(metadata) = video_metadata.as_ref() {
                draw_native_metadata_panel(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    metadata,
                    &mut commands,
                );
            }
            if jump_panel_visible {
                draw_native_jump_panel(
                    ctx,
                    overlay_height_points,
                    position_secs,
                    &jump_entries,
                    &jump_texture_ids,
                    &mut bookmark_title_edit,
                    &mut commands,
                );
            }
            if bookmark_title_edit.is_some() {
                last_drawn_bookmark_editor_rect = draw_native_bookmark_title_editor(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &mut bookmark_title_edit,
                    &mut commands,
                );
            }
            // Codex 4周目 P1: 旧コードは `if !bottom_hud_visible { return; }` で
            // 早期 return していたが、それだと bottom HUD 非表示時に下の
            // `draw_native_normalize_progress` まで届かない (= スキャン中に HUD が
            // フェードアウトすると進捗 UI も消える)。bottom HUD だけを条件分岐し、
            // progress UI 描画は必ず実行する。
            if bottom_hud_visible {
                egui::Area::new(egui::Id::new("native_video_seek_hud"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(0.0, (overlay_height_points - 46.0).max(0.0)))
                    .show(ctx, |ui| {
                        ui.set_min_size(egui::vec2(overlay_width_points, 46.0));
                        let hud_rect = ui.min_rect();
                        let painter = ui.painter().clone();
                        let painter = &painter;
                        painter.rect_filled(
                            hud_rect,
                            0.0,
                            egui::Color32::from_rgba_premultiplied(0, 0, 0, 176),
                        );

                        let side_pad = 10.0;
                        let btn_size = 28.0;
                        let gap = 8.0;
                        let center_y = hud_rect.center().y;
                        let text_center_y = center_y + 4.0;
                        let mut x = hud_rect.min.x + side_pad;

                        let replay_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let replay_resp = ui.interact(
                            replay_rect,
                            egui::Id::new("native_video_replay"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(painter, replay_rect, replay_resp.hovered(), false);
                        draw_overlay_replay_icon(painter, replay_rect.center(), btn_size * 0.36);
                        let replay_resp =
                            replay_resp.on_hover_text("最初から再生 (頭出し + 即再生) [W]");
                        if replay_resp.clicked() {
                            commands.push(NativeOverlayCommand::SeekToStartAndPlay);
                        }
                        x = replay_rect.max.x + gap;

                        let play_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let play_resp = ui.interact(
                            play_rect,
                            egui::Id::new("native_video_play"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(painter, play_rect, play_resp.hovered(), false);
                        if is_playing {
                            draw_overlay_pause_icon(painter, play_rect.center(), btn_size * 0.30);
                        } else {
                            draw_overlay_play_icon(painter, play_rect.center(), btn_size * 0.38);
                        }
                        let play_resp = play_resp.on_hover_text(if is_playing {
                            "一時停止 [Enter]"
                        } else {
                            "再生 [Enter]"
                        });
                        if play_resp.clicked() {
                            commands.push(NativeOverlayCommand::TogglePlay);
                        }
                        x = play_rect.max.x + gap;

                        let loop_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let loop_resp = ui.interact(
                            loop_rect,
                            egui::Id::new("native_video_loop"),
                            egui::Sense::click(),
                        );
                        // 4 段階ループモード: Off / Full / Chapter / Bookmark
                        // - Off: アイコン中央、active 背景なし
                        // - Full: アイコン中央、active 背景 (青)
                        // - Chapter: アイコン下半分縮小 + 上に「CH」テキスト
                        // - Bookmark: アイコン下半分縮小 + 上にブックマーク vector アイコン
                        // active 背景は loop_mode != Off (= 表示用) に基づき判定する。
                        // 「BM 設定 + BM 無し動画」では loop_enabled は true (Full と等価) でも
                        // ボタン表示は BM のまま (active 背景 + BM 装飾) になる。
                        use crate::settings::VideoLoopMode;
                        let mode_active = !matches!(loop_mode, VideoLoopMode::Off);
                        draw_overlay_button_bg(
                            painter,
                            loop_rect,
                            loop_resp.hovered(),
                            mode_active,
                        );
                        let icon_color = if mode_active {
                            egui::Color32::from_rgb(170, 230, 255)
                        } else {
                            egui::Color32::from_rgb(238, 238, 238)
                        };
                        match loop_mode {
                            VideoLoopMode::Off | VideoLoopMode::Full => {
                                draw_overlay_loop_icon(
                                    painter,
                                    loop_rect.center(),
                                    btn_size * 0.36,
                                    icon_color,
                                );
                            }
                            VideoLoopMode::Chapter => {
                                let r = btn_size * 0.36;
                                let c = loop_rect.center();
                                draw_overlay_loop_icon(
                                    painter,
                                    egui::pos2(c.x, c.y + r * 0.18),
                                    r * 0.65,
                                    icon_color,
                                );
                                painter.text(
                                    egui::pos2(c.x, c.y - r * 0.55),
                                    egui::Align2::CENTER_CENTER,
                                    "CH",
                                    egui::FontId::proportional(11.0),
                                    egui::Color32::from_rgb(115, 210, 255),
                                );
                            }
                            VideoLoopMode::Bookmark => {
                                let r = btn_size * 0.36;
                                let c = loop_rect.center();
                                draw_overlay_loop_icon(
                                    painter,
                                    egui::pos2(c.x, c.y + r * 0.18),
                                    r * 0.65,
                                    icon_color,
                                );
                                draw_overlay_bookmark_icon(
                                    painter,
                                    egui::pos2(c.x, c.y - r * 0.55),
                                    r * 0.32,
                                    egui::Color32::from_rgb(255, 220, 82),
                                );
                            }
                        }
                        let hover_text = match loop_mode {
                            VideoLoopMode::Off => "ループ再生 [L]",
                            VideoLoopMode::Full => "ループ: 全体 [L]",
                            VideoLoopMode::Chapter => "ループ: チャプター [L]",
                            VideoLoopMode::Bookmark => "ループ: ブックマーク [L]",
                        };
                        let loop_resp = loop_resp.on_hover_text(hover_text);
                        if loop_resp.clicked() {
                            commands.push(NativeOverlayCommand::ToggleLoop);
                        }
                        let _ = loop_enabled; // mode_active ベース描画なので未使用
                        x = loop_rect.max.x + gap;

                        let prev_frame_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let prev_down = draw_native_frame_step_button(
                            ui,
                            painter,
                            prev_frame_rect,
                            "native_video_prev_frame",
                            -1,
                            "前のフレーム [Ctrl+Shift+←]",
                            &mut frame_step_hold,
                            &mut commands,
                        );
                        x = prev_frame_rect.max.x + gap;

                        let screenshot_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let screenshot_resp = ui.interact(
                            screenshot_rect,
                            egui::Id::new("native_video_screenshot"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(
                            painter,
                            screenshot_rect,
                            screenshot_resp.hovered(),
                            false,
                        );
                        draw_overlay_camera_icon(painter, screenshot_rect);
                        let screenshot_resp =
                            screenshot_resp.on_hover_text("現在フレームをクリップボードにコピー");
                        if screenshot_resp.clicked() {
                            commands.push(NativeOverlayCommand::CopyFrameToClipboard);
                        }
                        x = screenshot_rect.max.x + gap;

                        let next_frame_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let next_down = draw_native_frame_step_button(
                            ui,
                            painter,
                            next_frame_rect,
                            "native_video_next_frame",
                            1,
                            "次のフレーム [Ctrl+Shift+→]",
                            &mut frame_step_hold,
                            &mut commands,
                        );
                        x = next_frame_rect.max.x + gap;
                        if !prev_down && !next_down {
                            frame_step_hold = None;
                        }

                        let time_w = 132.0;
                        let vol_label_w = 60.0;
                        let limiter_indicator_w = 14.0;
                        let vol_slider_w = 144.0;
                        let mute_w = btn_size;
                        let norm_w = btn_size; // 音量ノーマライズボタン (mute と同じサイズ)
                        let speed_w = btn_size * 1.55;
                        let right_controls_w = time_w
                            + gap
                            + speed_w
                            + gap
                            + mute_w
                            + gap
                            + norm_w
                            + gap
                            + vol_slider_w
                            + gap
                            + vol_label_w
                            + limiter_indicator_w;
                        let right_controls_x = hud_rect.max.x - side_pad - right_controls_w;
                        let bar_min_x = x;
                        let bar_max_x = (right_controls_x - gap).max(bar_min_x + 1.0);
                        let bar_rect = egui::Rect::from_min_max(
                            egui::pos2(bar_min_x, center_y - 4.0),
                            egui::pos2(bar_max_x, center_y + 4.0),
                        );
                        let hit_rect = egui::Rect::from_min_max(
                            egui::pos2(bar_min_x, hud_rect.min.y),
                            egui::pos2(bar_max_x, hud_rect.max.y),
                        );

                        let time_x = bar_max_x + gap;
                        let label = format!(
                            "{} / {}",
                            format_overlay_time(position_secs),
                            format_overlay_time(duration_secs)
                        );
                        painter.text(
                            egui::pos2(time_x, text_center_y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(14.0),
                            egui::Color32::from_rgb(238, 238, 238),
                        );

                        let speed_rect = egui::Rect::from_min_size(
                            egui::pos2(time_x + time_w + gap, center_y - btn_size * 0.5),
                            egui::vec2(speed_w, btn_size),
                        );
                        let mute_rect = egui::Rect::from_min_size(
                            egui::pos2(speed_rect.max.x + gap, center_y - btn_size * 0.5),
                            egui::vec2(mute_w, btn_size),
                        );
                        let norm_rect = egui::Rect::from_min_size(
                            egui::pos2(mute_rect.max.x + gap, center_y - btn_size * 0.5),
                            egui::vec2(norm_w, btn_size),
                        );
                        let vol_rect = egui::Rect::from_min_max(
                            egui::pos2(norm_rect.max.x + gap, center_y - 4.0),
                            egui::pos2(norm_rect.max.x + gap + vol_slider_w, center_y + 4.0),
                        );

                        painter.rect_filled(bar_rect, 2.0, egui::Color32::from_gray(74));
                        if duration_secs > 0.0 {
                            let progress = (position_secs / duration_secs).clamp(0.0, 1.0) as f32;
                            let filled = egui::Rect::from_min_max(
                                bar_rect.min,
                                egui::pos2(
                                    bar_rect.min.x + bar_rect.width() * progress,
                                    bar_rect.max.y,
                                ),
                            );
                            painter.rect_filled(
                                filled,
                                2.0,
                                egui::Color32::from_rgb(228, 228, 228),
                            );
                        }
                        if duration_secs > 0.0 {
                            for marker in &timeline_markers {
                                draw_timeline_marker(painter, bar_rect, duration_secs, *marker);
                            }
                        }

                        let seek_resp = ui.interact(
                            hit_rect,
                            egui::Id::new("native_video_seek_hit"),
                            egui::Sense::click_and_drag(),
                        );
                        if seek_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        if duration_secs > 0.0 {
                            if (seek_resp.clicked() || seek_resp.dragged())
                                && let Some(pos) = seek_resp.interact_pointer_pos()
                            {
                                let x = pos.x.clamp(bar_rect.min.x, bar_rect.max.x);
                                let frac =
                                    ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
                                let target_secs = duration_secs * frac as f64;
                                let should_emit = seek_resp.clicked()
                                    || last_seek_target_secs
                                        .map(|prev| (prev - target_secs).abs() >= 0.10)
                                        .unwrap_or(true);
                                if should_emit {
                                    last_seek_target_secs = Some(target_secs);
                                    commands.push(NativeOverlayCommand::Seek { target_secs });
                                }
                            }
                            if seek_resp.drag_stopped() {
                                last_seek_target_secs = None;
                            }
                        }
                        if duration_secs > 0.0 {
                            if seek_resp.hovered()
                                && let Some(pos) = pointer_pos
                            {
                                let x = pos.x.clamp(bar_rect.min.x, bar_rect.max.x);
                                let frac =
                                    ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
                                hover_preview_target_secs = Some(duration_secs * frac as f64);
                            }
                        } else {
                            hover_preview_target_secs = None;
                        }
                        if duration_secs > 0.0
                            && let Some(target) = hover_preview_target_secs
                        {
                            let frac = (target / duration_secs).clamp(0.0, 1.0) as f32;
                            let x = bar_rect.min.x + bar_rect.width() * frac;
                            let hover_preview_bookmarked = target_has_marker(
                                &timeline_markers,
                                target,
                                duration_secs,
                                |kind| kind == NativeOverlayTimelineMarkerKind::Bookmark,
                            );
                            let preview_image_w = (overlay_width_points * 0.30).clamp(300.0, 352.0);
                            let image_size =
                                egui::vec2(preview_image_w, preview_image_w * 9.0 / 16.0);
                            let action_bar_h = 38.0;
                            let preview_size =
                                egui::vec2(image_size.x, image_size.y + action_bar_h);
                            let preview_x = (x - preview_size.x * 0.5)
                                .clamp(8.0, overlay_width_points - preview_size.x - 8.0);
                            let preview_y = (hud_rect.min.y - preview_size.y - 14.0).max(8.0);
                            let preview_rect = egui::Rect::from_min_size(
                                egui::pos2(preview_x, preview_y),
                                preview_size,
                            );
                            let image_rect =
                                egui::Rect::from_min_size(preview_rect.min, image_size);
                            let action_rect = egui::Rect::from_min_max(
                                egui::pos2(preview_rect.min.x, image_rect.max.y),
                                preview_rect.max,
                            );
                            let preview_corridor_rect = egui::Rect::from_min_max(
                                egui::pos2(preview_rect.min.x - 8.0, preview_rect.max.y),
                                egui::pos2(preview_rect.max.x + 8.0, hud_rect.max.y),
                            );
                            let pointer_in_preview = pointer_pos.is_some_and(|pos| {
                                preview_rect.expand(8.0).contains(pos)
                                    || preview_corridor_rect.contains(pos)
                            });
                            if !seek_resp.hovered() && !pointer_in_preview {
                                hover_preview_target_secs = None;
                            } else {
                                // 実機修正 (2026-05-12 P1 #2): 実描画 rect を記録。
                                // `compute_hud_regions` が region 計算で読む。
                                last_drawn_preview_rect = Some(preview_rect);
                                let request_due = last_thumbnail_request_secs
                                    .map(|prev| (prev - target).abs() >= 0.25)
                                    .unwrap_or(true)
                                    || last_thumbnail_request_at
                                        .map(|last| last.elapsed() >= Duration::from_millis(250))
                                        .unwrap_or(true);
                                if request_due {
                                    last_thumbnail_request_secs = Some(target);
                                    last_thumbnail_request_at = Some(Instant::now());
                                    commands.push(NativeOverlayCommand::RequestSeekThumbnail {
                                        target_secs: target,
                                    });
                                }
                                painter.line_segment(
                                    [
                                        egui::pos2(x, hud_rect.min.y + 6.0),
                                        egui::pos2(x, hud_rect.max.y - 6.0),
                                    ],
                                    egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 88, 88)),
                                );

                                painter.rect_filled(
                                    preview_rect.expand(2.0),
                                    4.0,
                                    egui::Color32::from_rgba_premultiplied(0, 0, 0, 220),
                                );
                                painter.rect_filled(image_rect, 3.0, egui::Color32::from_gray(20));
                                painter.rect_filled(
                                    action_rect,
                                    0.0,
                                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 235),
                                );
                                painter.rect_stroke(
                                    preview_rect,
                                    3.0,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(150)),
                                    egui::StrokeKind::Inside,
                                );
                                let thumbnail_matches =
                                    hover_thumbnail.as_ref().is_some_and(|thumb| {
                                        (thumb.target_secs - target).abs()
                                            <= crate::video::thumbnail::SECONDS_PER_BUCKET * 2.0
                                    });
                                // スクラブ中はサムネ画像を消さずに直近の 1 枚を出し
                                // 続ける。以前は target に合うサムネが無い間「黒地 +
                                // loading」を出していたため、スクラブ中にサムネ画像と
                                // 黒地が交互に出てちらついていた。
                                if let (Some(texture_id), Some(thumb)) =
                                    (hover_texture_id, hover_thumbnail.as_ref())
                                {
                                    let fitted = fit_rect_in_rect(
                                        egui::vec2(thumb.width as f32, thumb.height as f32),
                                        image_rect,
                                    );
                                    painter.image(
                                        texture_id,
                                        fitted,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                                // 目標位置のサムネがまだ揃っていない間は、フルスクリーン
                                // の「シーク中...」表示と同じ見た目の box を中央に重ねる
                                // (サムネ未取得時は image_rect の gray 地の上に出る)。
                                if !thumbnail_matches {
                                    let font = egui::FontId::proportional(14.0);
                                    let galley = painter.layout_no_wrap(
                                        "シーク中".to_owned(),
                                        font,
                                        egui::Color32::from_rgb(238, 238, 238),
                                    );
                                    let box_rect = egui::Rect::from_center_size(
                                        image_rect.center(),
                                        galley.size() + egui::vec2(28.0, 16.0),
                                    );
                                    painter.rect_filled(
                                        box_rect,
                                        6.0,
                                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214),
                                    );
                                    painter.galley(
                                        box_rect.center() - galley.size() * 0.5,
                                        galley,
                                        egui::Color32::PLACEHOLDER,
                                    );
                                }

                                let action_size = 24.0;
                                let action_gap = 6.0;
                                let pin_rect = egui::Rect::from_min_size(
                                    action_rect.min + egui::vec2(6.0, 4.0),
                                    egui::vec2(action_size, action_size),
                                );
                                let bookmark_rect = egui::Rect::from_min_size(
                                    egui::pos2(pin_rect.max.x + action_gap, pin_rect.min.y),
                                    egui::vec2(action_size, action_size),
                                );
                                let pin_resp = ui.interact(
                                    pin_rect,
                                    egui::Id::new("native_video_hover_pin"),
                                    egui::Sense::click(),
                                );
                                draw_overlay_button_bg(
                                    painter,
                                    pin_rect,
                                    pin_resp.hovered(),
                                    hover_preview_pinned,
                                );
                                draw_overlay_pin_icon(
                                    painter,
                                    pin_rect.center(),
                                    action_size * 0.34,
                                    if hover_preview_pinned {
                                        egui::Color32::from_rgb(180, 255, 180)
                                    } else {
                                        egui::Color32::from_rgb(118, 214, 255)
                                    },
                                );
                                let pin_resp = pin_resp.on_hover_text(if hover_preview_pinned {
                                    "この位置でピン留めを上書き"
                                } else {
                                    "この位置をピン留め"
                                });
                                if pin_resp.clicked() {
                                    commands.push(NativeOverlayCommand::SetPinAt {
                                        target_secs: target,
                                    });
                                }
                                let bookmark_resp = ui.interact(
                                    bookmark_rect,
                                    egui::Id::new("native_video_hover_bookmark"),
                                    egui::Sense::click(),
                                );
                                draw_overlay_button_bg(
                                    painter,
                                    bookmark_rect,
                                    bookmark_resp.hovered(),
                                    hover_preview_bookmarked,
                                );
                                draw_overlay_bookmark_icon(
                                    painter,
                                    bookmark_rect.center(),
                                    action_size * 0.32,
                                    if hover_preview_bookmarked {
                                        egui::Color32::from_rgb(255, 245, 145)
                                    } else {
                                        egui::Color32::from_rgb(255, 220, 80)
                                    },
                                );
                                let bookmark_resp =
                                    bookmark_resp.on_hover_text(if hover_preview_bookmarked {
                                        "ブックマーク済み [B]"
                                    } else {
                                        "ブックマークを追加 [B]"
                                    });
                                if bookmark_resp.clicked() {
                                    commands.push(NativeOverlayCommand::AddBookmarkAt {
                                        target_secs: target,
                                    });
                                }

                                let time_label = format_overlay_time(target);
                                painter.text(
                                    egui::pos2(action_rect.max.x - 8.0, action_rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    time_label,
                                    egui::FontId::proportional(13.0),
                                    egui::Color32::from_rgb(245, 245, 245),
                                );
                            }
                        }

                        let mut speed_resp = ui.interact(
                            speed_rect,
                            egui::Id::new("native_video_speed"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(painter, speed_rect, speed_resp.hovered(), false);
                        painter.text(
                            egui::pos2(speed_rect.center().x, text_center_y),
                            egui::Align2::CENTER_CENTER,
                            crate::video::clock::format_playback_speed(playback_speed),
                            egui::FontId::proportional(12.0),
                            egui::Color32::from_rgb(238, 238, 238),
                        );
                        if speed_resp.secondary_clicked() || speed_resp.double_clicked() {
                            video_speed_popup_open = false;
                            commands.push(NativeOverlayCommand::SetPlaybackSpeed { speed: 1.0 });
                        } else if speed_resp.clicked() {
                            video_speed_popup_open = !video_speed_popup_open;
                        }
                        if !video_speed_popup_open {
                            speed_resp = speed_resp
                                .on_hover_text("再生速度 (右クリック / ダブルクリックで x1)");
                        }
                        if video_speed_popup_open {
                            use crate::video::clock::{
                                PLAYBACK_SPEED_CHOICES, format_playback_speed,
                            };

                            let popup_w = 356.0_f32.min((overlay_width_points - 16.0).max(180.0));
                            let speed_choice_size = egui::vec2(46.0, 24.0);
                            let speed_choice_text_y = 4.0;
                            let popup_h = speed_choice_size.y + 12.0;
                            let popup_x = (speed_rect.center().x - popup_w * 0.5)
                                .clamp(8.0, overlay_width_points - popup_w - 8.0);
                            let popup_y = (hud_rect.min.y - popup_h - 6.0).max(8.0);
                            let mut selected_speed = None;
                            let popup_inner =
                                egui::Area::new(egui::Id::new("native_video_speed_popup"))
                                    .order(egui::Order::Foreground)
                                    .fixed_pos(egui::pos2(popup_x, popup_y))
                                    .show(ctx, |ui| {
                                        egui::Frame::new()
                                            .fill(egui::Color32::from_rgba_unmultiplied(
                                                0, 0, 0, 225,
                                            ))
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                egui::Color32::from_gray(110),
                                            ))
                                            .corner_radius(egui::CornerRadius::same(4))
                                            .inner_margin(egui::Margin::same(6))
                                            .show(ui, |ui| {
                                                ui.set_min_width(popup_w - 12.0);
                                                ui.horizontal_wrapped(|ui| {
                                                    for speed in PLAYBACK_SPEED_CHOICES {
                                                        let selected =
                                                            (playback_speed - speed).abs() < 1.0e-6;
                                                        let label = format_playback_speed(speed);
                                                        let (button_rect, button_resp) = ui
                                                            .allocate_exact_size(
                                                                speed_choice_size,
                                                                egui::Sense::click(),
                                                            );
                                                        if button_resp.hovered() {
                                                            ui.ctx().set_cursor_icon(
                                                                egui::CursorIcon::PointingHand,
                                                            );
                                                        }
                                                        let visuals =
                                                            ui.style().interact_selectable(
                                                                &button_resp,
                                                                selected,
                                                            );
                                                        let painter = ui.painter();
                                                        painter.rect_filled(
                                                            button_rect,
                                                            3.0,
                                                            visuals.weak_bg_fill,
                                                        );
                                                        painter.rect_stroke(
                                                            button_rect,
                                                            3.0,
                                                            visuals.bg_stroke,
                                                            egui::StrokeKind::Inside,
                                                        );
                                                        painter.text(
                                                            button_rect.center()
                                                                + egui::vec2(
                                                                    0.0,
                                                                    speed_choice_text_y,
                                                                ),
                                                            egui::Align2::CENTER_CENTER,
                                                            label,
                                                            egui::TextStyle::Button
                                                                .resolve(ui.style()),
                                                            visuals.fg_stroke.color,
                                                        );
                                                        if button_resp.clicked() {
                                                            selected_speed = Some(speed);
                                                        }
                                                    }
                                                });
                                            });
                                    });
                            let popup_rect = popup_inner.response.rect;
                            last_drawn_speed_popup_rect = Some(popup_rect);
                            if ui.ctx().input(|i| i.pointer.any_click())
                                && !speed_resp.hovered()
                                && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
                            {
                                if !popup_rect.contains(pos) {
                                    video_speed_popup_open = false;
                                }
                            }
                            if let Some(speed) = selected_speed {
                                let speed = crate::video::clock::clamp_playback_speed(speed);
                                video_speed_popup_open = false;
                                commands.push(NativeOverlayCommand::SetPlaybackSpeed { speed });
                            }
                        }

                        let mute_resp = ui.interact(
                            mute_rect,
                            egui::Id::new("native_video_mute"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(painter, mute_rect, mute_resp.hovered(), muted);
                        draw_overlay_speaker_icon(
                            painter,
                            mute_rect.center(),
                            btn_size * 0.46,
                            muted,
                        );
                        let mute_resp = mute_resp.on_hover_text(if muted {
                            "ミュート解除 [M]"
                        } else {
                            "ミュート [M]"
                        });
                        if mute_resp.clicked() {
                            commands.push(NativeOverlayCommand::ToggleMute);
                        }

                        // ── 音量ノーマライズボタン (mute と vol_slider の間) ──
                        use crate::video::normalize_types::NormalizeUiState;
                        let norm_ui_state = self.normalize_state.ui_state;
                        let is_scanning = matches!(norm_ui_state, NormalizeUiState::Scanning);
                        let norm_active =
                            matches!(norm_ui_state, NormalizeUiState::OnApplied { .. });
                        let norm_unmeasured =
                            matches!(norm_ui_state, NormalizeUiState::OnUnmeasured);
                        let norm_resp = ui.interact(
                            norm_rect,
                            egui::Id::new("native_video_normalize"),
                            egui::Sense::CLICK | egui::Sense::HOVER,
                        );
                        let norm_hover_label = match norm_ui_state {
                            NormalizeUiState::Off => {
                                "音量ノーマライズ (-14 LUFS)。クリックで ON".to_string()
                            }
                            NormalizeUiState::OnApplied { gain_db } => format!(
                                "音量ノーマライズ ON ({gain_db:+.1}dB / -14 LUFS)。クリックで OFF"
                            ),
                            NormalizeUiState::OnUnmeasured => {
                                "音量ノーマライズが有効です。クリックして測定 / 右クリックで OFF"
                                    .to_string()
                            }
                            NormalizeUiState::Scanning => "ノーマライズ中…".to_string(),
                        };
                        draw_overlay_button_bg(
                            painter,
                            norm_rect,
                            norm_resp.hovered() && !is_scanning,
                            norm_active,
                        );
                        // ボタン色: Off=グレー / OnApplied=黄 / OnUnmeasured=オレンジ点滅 / Scanning=グレー
                        let norm_color = if is_scanning {
                            egui::Color32::from_gray(120)
                        } else if norm_active {
                            egui::Color32::from_rgb(255, 198, 62)
                        } else if norm_unmeasured {
                            // 半透明 blink (時間ベースで alpha 変動)
                            let t = ui.ctx().input(|i| i.time);
                            let blink = (((t * 2.0).sin() + 1.0) * 0.5) as f32;
                            let alpha = (180.0 + blink * 75.0) as u8;
                            egui::Color32::from_rgba_unmultiplied(255, 150, 60, alpha)
                        } else {
                            egui::Color32::from_gray(180)
                        };
                        // "Norm" ラベル
                        painter.text(
                            egui::pos2(norm_rect.center().x, text_center_y),
                            egui::Align2::CENTER_CENTER,
                            "Norm",
                            egui::FontId::proportional(11.0),
                            norm_color,
                        );
                        let norm_resp = norm_resp.on_hover_text(norm_hover_label);
                        if !is_scanning {
                            if norm_resp.clicked() {
                                commands.push(NativeOverlayCommand::ToggleNormalize);
                            } else if norm_resp.secondary_clicked() {
                                commands.push(NativeOverlayCommand::DisableNormalize);
                            }
                        }

                        painter.rect_filled(vol_rect, 2.0, egui::Color32::from_gray(74));
                        let volume = finite_video_volume(volume);
                        let volume_pos =
                            crate::settings::video_volume_linear_to_fader_pos(volume) as f32;
                        let zero_frac = crate::settings::video_volume_db_to_fader_pos(0.0) as f32;
                        let normal_fill_frac = volume_pos.min(zero_frac);
                        if normal_fill_frac > 0.0 {
                            let normal_fill = egui::Rect::from_min_max(
                                vol_rect.min,
                                egui::pos2(
                                    vol_rect.min.x + vol_rect.width() * normal_fill_frac,
                                    vol_rect.max.y,
                                ),
                            );
                            painter.rect_filled(
                                normal_fill,
                                2.0,
                                egui::Color32::from_rgb(220, 220, 220),
                            );
                        }
                        if volume_pos > zero_frac {
                            let boost_fill = egui::Rect::from_min_max(
                                egui::pos2(
                                    vol_rect.min.x + vol_rect.width() * zero_frac,
                                    vol_rect.min.y,
                                ),
                                egui::pos2(
                                    vol_rect.min.x + vol_rect.width() * volume_pos,
                                    vol_rect.max.y,
                                ),
                            );
                            painter.rect_filled(
                                boost_fill,
                                2.0,
                                egui::Color32::from_rgb(255, 198, 62),
                            );
                        }
                        for &db in &crate::settings::VIDEO_VOLUME_FADER_DB_MARKS {
                            let frac = crate::settings::video_volume_db_to_fader_pos(db) as f32;
                            let x = vol_rect.min.x + vol_rect.width() * frac;
                            let tick_h = if db == 0.0 {
                                8.0
                            } else if db > 0.0 {
                                6.0
                            } else {
                                4.0
                            };
                            let color = if db == 0.0 {
                                egui::Color32::from_gray(170)
                            } else if db > 0.0 {
                                egui::Color32::from_rgb(220, 170, 70)
                            } else {
                                egui::Color32::from_gray(118)
                            };
                            painter.line_segment(
                                [
                                    egui::pos2(x, vol_rect.center().y - tick_h * 0.5),
                                    egui::pos2(x, vol_rect.center().y + tick_h * 0.5),
                                ],
                                egui::Stroke::new(1.0, color),
                            );
                        }
                        let vol_resp = ui.interact(
                            vol_rect.expand2(egui::vec2(0.0, 10.0)),
                            egui::Id::new("native_video_volume"),
                            egui::Sense::click_and_drag(),
                        );
                        if vol_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        let vol_resp = vol_resp.on_hover_text(
                            "音量 (右クリック / ダブルクリックで 0dB) [Shift+↑ / Shift+↓]",
                        );
                        if vol_resp.secondary_clicked() || vol_resp.double_clicked() {
                            self.last_volume_target = Some(1.0);
                            commands.push(NativeOverlayCommand::SetVolume {
                                volume: 1.0,
                                persist: true,
                            });
                        } else if (vol_resp.clicked() || vol_resp.dragged())
                            && let Some(pos) = vol_resp.interact_pointer_pos()
                        {
                            let value = crate::settings::video_volume_fader_pos_to_linear(
                                ((pos.x - vol_rect.min.x) / vol_rect.width()).clamp(0.0, 1.0)
                                    as f64,
                            );
                            self.last_volume_target = Some(value);
                            commands.push(NativeOverlayCommand::SetVolume {
                                volume: value,
                                persist: vol_resp.clicked() && !vol_resp.dragged(),
                            });
                        }
                        if vol_resp.drag_stopped() {
                            let value = self.last_volume_target.take().unwrap_or(volume);
                            commands.push(NativeOverlayCommand::SetVolume {
                                volume: value,
                                persist: true,
                            });
                        }
                        let volume_label = format_video_volume_db_compact(volume);
                        let volume_label_color = if volume > 1.0 {
                            egui::Color32::from_rgb(255, 210, 80)
                        } else {
                            egui::Color32::from_rgb(238, 238, 238)
                        };
                        painter.text(
                            egui::pos2(vol_rect.max.x + gap + vol_label_w, text_center_y),
                            egui::Align2::RIGHT_CENTER,
                            volume_label,
                            egui::FontId::proportional(13.0),
                            volume_label_color,
                        );
                        let limiter_rect = egui::Rect::from_center_size(
                            egui::pos2(
                                vol_rect.max.x + gap + vol_label_w + limiter_indicator_w * 0.5,
                                center_y,
                            ),
                            egui::vec2(limiter_indicator_w, btn_size),
                        );
                        if limiter_ceiling_hit {
                            let limiter_resp = ui.interact(
                                limiter_rect,
                                egui::Id::new("native_video_limiter_indicator"),
                                egui::Sense::hover(),
                            );
                            painter.circle_filled(
                                limiter_rect.center(),
                                if limiter_resp.hovered() { 4.5 } else { 4.0 },
                                egui::Color32::from_rgb(255, 72, 72),
                            );
                            limiter_resp.on_hover_text("出力リミッターが作動しました");
                        }

                        if cfg!(debug_assertions) {
                            let pointer = pointer_pos
                                .map(|p| format!("{:.0},{:.0}", p.x, p.y))
                                .unwrap_or_else(|| "outside".to_string());
                            painter.text(
                                egui::pos2(hud_rect.max.x - 12.0, hud_rect.min.y + 10.0),
                                egui::Align2::RIGHT_TOP,
                                format!("native overlay events={event_count} pointer={pointer}"),
                                egui::FontId::proportional(10.0),
                                egui::Color32::from_rgb(150, 150, 150),
                            );
                        }
                    });
            } // ← `if bottom_hud_visible {` の閉じ (Codex 4周目 P1)
            // Codex 3周目 P2 反映: 音量ノーマライズ進捗パネルは **すべての overlay UI
            // 描画の最後** に置く。同じ Order::Foreground の Area は描画順 = z-order なので、
            // metadata panel / jump panel / bookmark editor / bottom HUD より後に描けば
            // 全画面 blocker がそれらより前面に出てクリックを完全にキャプチャできる。
            // Codex 4周目 P1: bottom_hud_visible == false でも必ず実行 (HUD フェードアウト中も
            // 進捗 UI は出続ける)。
            if matches!(
                normalize_state_snap.ui_state,
                crate::video::normalize_types::NormalizeUiState::Scanning
            ) {
                if let Some(progress) = normalize_state_snap.progress.as_ref() {
                    draw_native_normalize_progress(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        progress,
                        &mut commands,
                    );
                }
            }
        });
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|output| output.repaint_delay)
            .unwrap_or(Duration::MAX);
        self.next_repaint_deadline = if repaint_delay == Duration::MAX {
            None
        } else {
            Instant::now().checked_add(repaint_delay)
        };
        let now = Instant::now();
        self.schedule_seek_status_repaint(now);
        self.schedule_limiter_indicator_repaint(now);
        if hud_repaint_debug_enabled() && repaint_delay != Duration::MAX {
            crate::logger::log(format!(
                "[HUD-DEBUG] egui repaint_delay={:?} pending={} dirty_after_run={}",
                repaint_delay,
                self.pending_events.len(),
                self.dirty
            ));
        }
        // Query after `run`: egui updates these flags from the just-processed
        // frame, which lets this presenter decide whether to forward the same
        // native input batch to the legacy fullscreen shortcut path.
        self.wants_pointer_input = self.egui_ctx.wants_pointer_input();
        self.wants_keyboard_input = self.egui_ctx.wants_keyboard_input();
        self.last_seek_target_secs = last_seek_target_secs;
        self.last_thumbnail_request_secs = last_thumbnail_request_secs;
        self.last_thumbnail_request_at = last_thumbnail_request_at;
        self.hover_preview_target_secs = hover_preview_target_secs;
        self.last_drawn_preview_rect = last_drawn_preview_rect;
        self.last_drawn_vst3_panel_rect = last_drawn_vst3_panel_rect;
        self.last_emitted_vst3_panel_pos = last_emitted_vst3_panel_pos;
        self.last_drawn_paused_center_rects = last_drawn_paused_center_rects;
        self.last_drawn_toast_rect = last_drawn_toast_rect;
        self.last_drawn_speed_popup_rect = last_drawn_speed_popup_rect;
        self.last_drawn_bookmark_editor_rect = last_drawn_bookmark_editor_rect;
        self.video_speed_popup_open = video_speed_popup_open;
        self.frame_step_hold = frame_step_hold;
        self.bookmark_title_edit = bookmark_title_edit;
        self.top_bar_visible = top_bar_visible || side_panel_visible;
        self.right_panel_visible = right_panel_visible;
        self.jump_panel_visible = jump_panel_visible;
        self.update_ime_cursor_area(full_output.platform_output.ime);
        // パネル / HUD / トースト / paused_center などの「ユーザー操作対象 UI」が
        // 一切出ていない (= cursor_blocking_overlay_visible が false) で、ユーザー
        // 無操作が CURSOR_HIDE_IDLE_SECS 経過したらカーソルを隠す。
        // チェックマーク (`checked`) は単なる状態インジケータなので blocking には
        // 含めない (= 静止画側 `fs_ui_is_clean` の挙動と揃える)。
        // egui の cursor_icon を SetCursor(None) で上書きする。次回イベント到来時に
        // push_native_event 経由で cursor_last_activity が更新され、自然に復活する。
        //
        // 状態機械 (シンプル版):
        // - cursor_blocking_overlay_visible == true: 毎フレーム cursor_last_activity を
        //   Some(now) に bump して countdown を 0 に戻す (= 一時停止 → 再開で
        //   paused_center が消えた瞬間から 3 秒測り直す)。`wants_periodic_tick()` も
        //   毎フレーム true なので pause 中も 250ms ごとに render が走る (P3 トレードオフ)。
        // - cursor_blocking_overlay_visible == false: cursor_last_activity をそのまま
        //   維持して idle を計算。3 秒経過したらカーソル非表示。
        // - cursor_should_hide は idle のみで判定 (cursor_hidden 状態の sticky carry は
        //   不要 — push_native_event / mark_cursor_activity で適切にリセットされるため)。
        if cursor_blocking_overlay_visible {
            self.cursor_last_activity = Some(Instant::now());
            self.cursor_hidden = false;
        }
        let cursor_should_hide = !cursor_blocking_overlay_visible
            && self
                .cursor_last_activity
                .map(|t| t.elapsed().as_secs_f32() >= CURSOR_HIDE_IDLE_SECS)
                .unwrap_or(false);
        let resolved_cursor_icon = if cursor_should_hide {
            egui::CursorIcon::None
        } else {
            full_output.platform_output.cursor_icon
        };
        self.update_cursor_icon(resolved_cursor_icon);
        self.cursor_hidden = cursor_should_hide;

        let shape_count = full_output.shapes.len();
        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.width, self.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        let surface_texture = self
            .surface
            .get_current_texture()
            .map_err(|e| format!("egui overlay get_current_texture: {e:?}"))?;
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mIV native egui overlay encoder"),
            });
        let user_cmds = self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mIV native egui overlay render"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }
        let mut submissions = user_cmds;
        submissions.push(encoder.finish());
        self.queue.submit(submissions);
        surface_texture.present();
        if overlay_visible {
            self.set_visual_attached(true)?;
        }
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        self.dirty = self.frame_step_hold.is_some() || self.bookmark_title_edit.is_some();
        log_event(
            "egui_overlay_present",
            &[
                ("width", Value::from(self.width as i64)),
                ("height", Value::from(self.height as i64)),
                ("input_events", Value::from(pending_event_count as i64)),
                ("native_events", Value::from(event_count as i64)),
                ("pixels_per_point", Value::from(ppp)),
                ("shapes", Value::from(shape_count as i64)),
                ("paint_jobs", Value::from(paint_jobs.len() as i64)),
                ("wants_pointer", Value::from(self.wants_pointer_input)),
                ("wants_keyboard", Value::from(self.wants_keyboard_input)),
                ("hud_visible", Value::from(hud_visible)),
                ("perf_visible", Value::from(perf_visible)),
                ("visual_attached", Value::from(self.visual_attached)),
                (
                    "render_ms",
                    Value::from(render_t0.elapsed().as_secs_f64() * 1000.0),
                ),
            ],
        );
        Ok(commands)
    }

    fn update_ime_cursor_area(&self, ime: Option<egui::output::IMEOutput>) {
        let Some(ime) = ime else {
            return;
        };
        let ppp = self.pixels_per_point.max(1.0);
        let cursor = ime.cursor_rect;
        let x = (cursor.min.x * ppp).round() as i32;
        let y = (cursor.min.y * ppp).round() as i32;
        let width = (cursor.width().max(1.0) * ppp).round() as i32;
        let height = (cursor.height().max(1.0) * ppp).round() as i32;
        let rc_area = RECT {
            left: x,
            top: y,
            right: x + width.max(1),
            bottom: y + height.max(1),
        };
        let candidate_form = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: CFS_EXCLUDE,
            ptCurrentPos: POINT { x, y },
            rcArea: rc_area,
        };
        let composition_form = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT {
                x,
                y: rc_area.bottom,
            },
            rcArea: rc_area,
        };
        unsafe {
            // IME context は **focus を持つ HWND** (= presenter HWND) で取る必要がある。
            // HUD HWND は `WS_EX_NOACTIVATE` で focus を取らないので、HUD HWND で
            // `ImmGetContext` を呼んでも有効な context が返らず、IME が動かない
            // (Codex プラン P1 #2 反映)。
            let himc = ImmGetContext(self.focus_hwnd);
            if himc.0.is_null() {
                return;
            }
            let _ = ImmSetCompositionWindow(himc, &composition_form);
            let _ = ImmSetCandidateWindow(himc, &candidate_form);
            let _ = ImmReleaseContext(self.focus_hwnd, himc);
        }
    }

    fn update_cursor_icon(&self, cursor_icon: egui::CursorIcon) {
        use std::sync::atomic::Ordering;
        // `CursorIcon::None` は `SetCursor(None)` で完全非表示にする。IDC_ARROW へ
        // フォールバックすると idle 時にもポインタが見えてしまう。毎フレーム呼ばれる
        // ので、非表示状態は連続して `SetCursor(None)` が打たれて維持される。
        //
        // 実機修正 (2026-05-12 Codex P2 #6): HUD wndproc に「直前に隠した」情報を共有して、
        // WM_SETCURSOR が必要に応じて IDC_ARROW で復帰させる。
        if matches!(cursor_icon, egui::CursorIcon::None) {
            unsafe {
                SetCursor(None);
            }
            self.cursor_was_hidden_shared.store(true, Ordering::Release);
            return;
        }
        // 復帰側: cursor が見える icon を設定するとき、hidden flag を clear。
        self.cursor_was_hidden_shared
            .store(false, Ordering::Release);
        let cursor_id = match cursor_icon {
            egui::CursorIcon::PointingHand => IDC_HAND,
            egui::CursorIcon::Text | egui::CursorIcon::VerticalText => IDC_IBEAM,
            egui::CursorIcon::ResizeHorizontal => IDC_SIZEWE,
            egui::CursorIcon::ResizeVertical => IDC_SIZENS,
            egui::CursorIcon::Move
            | egui::CursorIcon::Grab
            | egui::CursorIcon::Grabbing
            | egui::CursorIcon::AllScroll => IDC_SIZEALL,
            egui::CursorIcon::NotAllowed | egui::CursorIcon::NoDrop => IDC_NO,
            egui::CursorIcon::Progress | egui::CursorIcon::Wait => IDC_WAIT,
            egui::CursorIcon::Default => IDC_ARROW,
            _ => IDC_ARROW,
        };
        if let Ok(cursor) = unsafe { LoadCursorW(None, cursor_id) } {
            unsafe {
                SetCursor(Some(cursor));
            }
        }
    }
}

fn choose_overlay_surface_format(
    formats: &[wgpu::TextureFormat],
) -> Result<wgpu::TextureFormat, String> {
    for preferred in [
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        if formats.contains(&preferred) {
            return Ok(preferred);
        }
    }
    formats
        .first()
        .copied()
        .ok_or_else(|| "wgpu DComp overlay surface has no formats".to_string())
}

fn pixels_per_point_for_hwnd(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let pixels_per_point = dpi as f32 / 96.0;
    if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    }
}

fn egui_modifiers(shift: bool, ctrl: bool, alt: bool) -> egui::Modifiers {
    egui::Modifiers {
        alt,
        ctrl,
        shift,
        mac_cmd: false,
        command: ctrl,
    }
}

fn egui_key_from_virtual_key(vk: u32) -> Option<egui::Key> {
    Some(match vk {
        0x08 => egui::Key::Backspace,
        0x09 => egui::Key::Tab,
        0x0D => egui::Key::Enter,
        0x1B => egui::Key::Escape,
        0x20 => egui::Key::Space,
        0x21 => egui::Key::PageUp,
        0x22 => egui::Key::PageDown,
        0x23 => egui::Key::End,
        0x24 => egui::Key::Home,
        0x25 => egui::Key::ArrowLeft,
        0x26 => egui::Key::ArrowUp,
        0x27 => egui::Key::ArrowRight,
        0x28 => egui::Key::ArrowDown,
        0x2D => egui::Key::Insert,
        0x2E => egui::Key::Delete,
        0x30 => egui::Key::Num0,
        0x31 => egui::Key::Num1,
        0x32 => egui::Key::Num2,
        0x33 => egui::Key::Num3,
        0x34 => egui::Key::Num4,
        0x35 => egui::Key::Num5,
        0x36 => egui::Key::Num6,
        0x37 => egui::Key::Num7,
        0x38 => egui::Key::Num8,
        0x39 => egui::Key::Num9,
        0x41 => egui::Key::A,
        0x42 => egui::Key::B,
        0x43 => egui::Key::C,
        0x44 => egui::Key::D,
        0x45 => egui::Key::E,
        0x46 => egui::Key::F,
        0x47 => egui::Key::G,
        0x48 => egui::Key::H,
        0x49 => egui::Key::I,
        0x4A => egui::Key::J,
        0x4B => egui::Key::K,
        0x4C => egui::Key::L,
        0x4D => egui::Key::M,
        0x4E => egui::Key::N,
        0x4F => egui::Key::O,
        0x50 => egui::Key::P,
        0x51 => egui::Key::Q,
        0x52 => egui::Key::R,
        0x53 => egui::Key::S,
        0x54 => egui::Key::T,
        0x55 => egui::Key::U,
        0x56 => egui::Key::V,
        0x57 => egui::Key::W,
        0x58 => egui::Key::X,
        0x59 => egui::Key::Y,
        0x5A => egui::Key::Z,
        0x70 => egui::Key::F1,
        0x71 => egui::Key::F2,
        0x72 => egui::Key::F3,
        0x73 => egui::Key::F4,
        0x74 => egui::Key::F5,
        0x75 => egui::Key::F6,
        0x76 => egui::Key::F7,
        0x77 => egui::Key::F8,
        0x78 => egui::Key::F9,
        0x79 => egui::Key::F10,
        0x7A => egui::Key::F11,
        0x7B => egui::Key::F12,
        _ => return None,
    })
}

impl NativeTestOverlay {
    fn new(
        factory: &IDXGIFactory2,
        d3d_device: &ID3D11Device,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        d3d_context1: &ID3D11DeviceContext1,
        dcomp_device: &IDCompositionDevice,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap_chain = unsafe {
            factory
                .CreateSwapChainForComposition(d3d_device, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition overlay: {e:?}"))?
        };
        let visual = unsafe {
            let visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual overlay: {e:?}"))?;
            visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent overlay: {e:?}"))?;
            visual
        };
        let mut this = Self {
            swap_chain,
            _visual: visual,
            backbuffer: None,
            render_target: None,
            width: width.max(1),
            height: height.max(1),
        };
        this.recreate_backbuffer(d3d_device1, d3d_context)?;
        this.draw_test_pattern(d3d_context, d3d_context1)?;
        log_event(
            "overlay_init",
            &[
                ("width", Value::from(this.width as i64)),
                ("height", Value::from(this.height as i64)),
                ("alpha_mode", Value::from("premultiplied")),
            ],
        );
        Ok(this)
    }

    fn resize(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        d3d_context1: &ID3D11DeviceContext1,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return Ok(());
        }
        self.render_target = None;
        self.backbuffer = None;
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width,
                    height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|e| format!("overlay IDXGISwapChain::ResizeBuffers: {e:?}"))?;
        }
        self.width = width;
        self.height = height;
        self.recreate_backbuffer(d3d_device1, d3d_context)?;
        self.draw_test_pattern(d3d_context, d3d_context1)?;
        Ok(())
    }

    fn recreate_backbuffer(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
    ) -> Result<(), String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("overlay IDXGISwapChain::GetBuffer: {e:?}"))?
        };
        let mut render_target = None;
        unsafe {
            d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut render_target))
                .map_err(|e| format!("overlay CreateRenderTargetView: {e:?}"))?;
        }
        let render_target: ID3D11RenderTargetView = render_target
            .ok_or_else(|| "overlay CreateRenderTargetView returned null".to_string())?;
        unsafe {
            d3d_context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.0, 0.0]);
        }
        self.backbuffer = Some(backbuffer);
        self.render_target = Some(render_target);
        Ok(())
    }

    fn draw_test_pattern(
        &mut self,
        d3d_context: &ID3D11DeviceContext,
        d3d_context1: &ID3D11DeviceContext1,
    ) -> Result<(), String> {
        let render_target = self
            .render_target
            .as_ref()
            .ok_or_else(|| "overlay render target is not initialized".to_string())?;
        unsafe {
            d3d_context.ClearRenderTargetView(render_target, &[0.0, 0.0, 0.0, 0.0]);
            let view: ID3D11View = render_target
                .cast()
                .map_err(|e| format!("cast overlay RTV to view: {e:?}"))?;
            let bar = RECT {
                left: 32,
                top: 32,
                right: (self.width as i32).min(360).max(33),
                bottom: (self.height as i32).min(92).max(33),
            };
            let badge = RECT {
                left: 44,
                top: 44,
                right: (self.width as i32).min(84).max(45),
                bottom: (self.height as i32).min(80).max(45),
            };
            // Premultiplied colors: RGB components are intentionally <= alpha.
            d3d_context1.ClearView(&view, &[0.0, 0.09, 0.04, 0.34], Some(&[bar]));
            d3d_context1.ClearView(&view, &[0.0, 0.24, 0.10, 0.52], Some(&[badge]));
            self.swap_chain
                .Present(1, Default::default())
                .ok()
                .map_err(|e| format!("overlay IDXGISwapChain::Present: {e:?}"))?;
        }
        log_event(
            "overlay_present",
            &[
                ("width", Value::from(self.width as i64)),
                ("height", Value::from(self.height as i64)),
            ],
        );
        Ok(())
    }
}

impl Drop for NativeVideoPresenter {
    fn drop(&mut self) {
        if !self.waitable.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.waitable);
            }
        }
    }
}

fn log_event(kind: &str, fields: &[(&str, Value)]) {
    crate::perf::event("native_presenter", kind, None, 0, fields);
}

/// 動画 visual の DirectComposition transform 行列を SAR 込みで計算する。
///
/// `surface_w/h` は decoded frame の raw pixel サイズ (= swap chain backbuffer)。
/// `win_w/h` はウィンドウサイズ。`sar_num/sar_den` は sample aspect ratio
/// (= pixel aspect ratio)、1/1 で従来の isotropic 表示。`compact` は VST3 panel 表示時の
/// 1/4 領域モード。
///
/// 戻り値は `(M11, M22, M31, M32)`。`M12 = M21 = 0` (= 回転 / shear なし)。
/// SAR != 1:1 の場合は M11 != M22 の anisotropic scale になる
/// (= 横方向だけ伸ばして表示比を補正する、SAR>1 の anamorphic 動画)。
fn compute_video_visual_transform(
    surface_w: u32,
    surface_h: u32,
    win_w: u32,
    win_h: u32,
    sar_num: u32,
    sar_den: u32,
    compact: bool,
) -> (f32, f32, f32, f32) {
    let surface_w = surface_w.max(1) as f32;
    let surface_h = surface_h.max(1) as f32;
    let win_w = win_w.max(1) as f32;
    let win_h = win_h.max(1) as f32;
    let sar = (sar_num.max(1) as f32) / (sar_den.max(1) as f32);
    // 表示寸法 = raw pixel × SAR (横だけ)。SAR>1 で widen、SAR<1 で narrow。
    let display_w = surface_w * sar;
    let display_h = surface_h;
    let (target_x, target_y, target_w, target_h) = if compact {
        (win_w * 0.5, 0.0, win_w * 0.5, win_h * 0.5)
    } else {
        (0.0, 0.0, win_w, win_h)
    };
    // `display_w × display_h` を `target_w × target_h` に letterbox fit する scale。
    let scale = (target_w / display_w).min(target_h / display_h);
    // M11 は raw surface 幅から最終表示幅への係数なので、scale × sar が掛かる。
    // M22 は高さなので scale だけ。SAR=1:1 なら M11 == M22 で従来挙動と同一。
    let m11 = scale * sar;
    let m22 = scale;
    let offset_x = target_x + (target_w - display_w * scale) * 0.5;
    let offset_y = target_y + (target_h - display_h * scale) * 0.5;
    (m11, m22, offset_x, offset_y)
}

fn copy_cpu_rgba_to_swapchain_bgra(
    src_rgba: &[u8],
    dst_bgra: &mut Vec<u8>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| format!("CPU frame size overflow: {width}x{height}"))?;
    if src_rgba.len() < expected_len {
        return Err(format!(
            "CPU frame buffer is too small: got {} bytes, expected {expected_len}",
            src_rgba.len()
        ));
    }

    dst_bgra.resize(expected_len, 0);
    for (src, dst) in src_rgba[..expected_len]
        .chunks_exact(4)
        .zip(dst_bgra.chunks_exact_mut(4))
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    Ok(())
}

fn sample_cpu_rgba_pixel(
    src_rgba: &[u8],
    width: u32,
    height: u32,
    format: i32,
) -> Result<NativePixelSample, String> {
    if width == 0 || height == 0 {
        return Err("CPU pixel probe: empty frame".to_string());
    }
    let x = width / 2;
    let y = height / 2;
    let offset = (y as usize)
        .checked_mul(width as usize)
        .and_then(|row| row.checked_add(x as usize))
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| format!("CPU pixel probe size overflow: {width}x{height}"))?;
    if src_rgba.len() < offset + 4 {
        return Err(format!(
            "CPU pixel probe buffer is too small: got {} bytes, need {}",
            src_rgba.len(),
            offset + 4
        ));
    }
    Ok(NativePixelSample {
        x,
        y,
        width,
        height,
        format,
        b: src_rgba[offset + 2],
        g: src_rgba[offset + 1],
        r: src_rgba[offset],
        a: src_rgba[offset + 3],
    })
}

fn compare_pixel_probe(
    path: &str,
    expected: NativePixelSample,
    actual: NativePixelSample,
) -> Result<(), String> {
    const TOLERANCE: u8 = 0;
    let mismatch = expected.width != actual.width
        || expected.height != actual.height
        || expected.format != actual.format
        || channel_delta(expected.b, actual.b) > TOLERANCE
        || channel_delta(expected.g, actual.g) > TOLERANCE
        || channel_delta(expected.r, actual.r) > TOLERANCE
        || channel_delta(expected.a, actual.a) > TOLERANCE;
    if !mismatch {
        log_event(
            "pixel_probe_match",
            &[
                ("path", Value::from(path)),
                ("x", Value::from(actual.x as i64)),
                ("y", Value::from(actual.y as i64)),
                ("b", Value::from(actual.b as i64)),
                ("g", Value::from(actual.g as i64)),
                ("r", Value::from(actual.r as i64)),
                ("a", Value::from(actual.a as i64)),
            ],
        );
        return Ok(());
    }

    log_event(
        "pixel_probe_mismatch",
        &[
            ("path", Value::from(path)),
            ("expected_b", Value::from(expected.b as i64)),
            ("expected_g", Value::from(expected.g as i64)),
            ("expected_r", Value::from(expected.r as i64)),
            ("expected_a", Value::from(expected.a as i64)),
            ("actual_b", Value::from(actual.b as i64)),
            ("actual_g", Value::from(actual.g as i64)),
            ("actual_r", Value::from(actual.r as i64)),
            ("actual_a", Value::from(actual.a as i64)),
        ],
    );
    Err(format!(
        "native presenter pixel probe mismatch on {path}: expected BGRA=({},{},{},{}) got BGRA=({},{},{},{})",
        expected.b, expected.g, expected.r, expected.a, actual.b, actual.g, actual.r, actual.a
    ))
}

fn channel_delta(a: u8, b: u8) -> u8 {
    a.abs_diff(b)
}

#[cfg(test)]
mod tests {
    use super::{
        NativeOverlayInputRouting, NativePixelSample, compare_pixel_probe,
        compute_video_visual_transform, copy_cpu_rgba_to_swapchain_bgra, metadata_clean_text,
        native_video_fullscreen_shortcut_key, sample_cpu_rgba_pixel,
    };
    use crate::video::native_window::{
        NativeVideoKeyEvent, NativeVideoMouseWheelEvent, NativeVideoWindowEvent,
    };

    fn key(virtual_key: u32) -> NativeVideoKeyEvent {
        NativeVideoKeyEvent {
            virtual_key,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        }
    }

    fn wheel(ctrl: bool) -> NativeVideoWindowEvent {
        NativeVideoWindowEvent::MouseWheel(NativeVideoMouseWheelEvent {
            delta: 120,
            x: 100,
            y: 100,
            shift: false,
            ctrl,
        })
    }

    #[test]
    fn native_overlay_does_not_double_forward_consumed_wheel() {
        // overlay が wheel を WheelNavigate / TileColumnsDelta コマンドへ変換済みなら、
        // wants_pointer_input が false でも raw wheel を App へ転送しない
        // (= overlay コマンドと App 側 wheel ハンドラの二重適用を防ぐ)。
        let consumed = NativeOverlayInputRouting {
            wants_pointer_input: false,
            consumed_wheel: true,
            ..Default::default()
        };
        assert!(!consumed.should_forward_to_ui(&wheel(true)));
        assert!(!consumed.should_forward_to_ui(&wheel(false)));

        // 未消費 (= overlay 無効のフォールバック経路など) なら従来どおり
        // wants_pointer_input 次第で転送する。
        let fallback = NativeOverlayInputRouting {
            wants_pointer_input: false,
            consumed_wheel: false,
            ..Default::default()
        };
        assert!(fallback.should_forward_to_ui(&wheel(true)));

        // overlay UI 上 (wants_pointer_input=true) なら未消費でも転送しない。
        let over_ui = NativeOverlayInputRouting {
            wants_pointer_input: true,
            consumed_wheel: false,
            ..Default::default()
        };
        assert!(!over_ui.should_forward_to_ui(&wheel(true)));
    }

    #[test]
    fn native_overlay_routes_shortcuts_even_when_button_has_focus() {
        let routing = NativeOverlayInputRouting {
            wants_keyboard_input: true,
            text_input_active: false,
            ..Default::default()
        };

        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x20))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x0D))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x41))));
        // マウス進む/戻る (VK_BROWSER_BACK/FORWARD) も overlay が keyboard を欲しがる
        // 状態 (text input 以外) でも fullscreen ショートカットとして App 側へ流す (Codex P2)。
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA6))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA7))));
    }

    #[test]
    fn native_overlay_keeps_shortcuts_while_text_input_is_active() {
        let routing = NativeOverlayInputRouting {
            wants_keyboard_input: true,
            text_input_active: true,
            ..Default::default()
        };

        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x20))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x0D))));
        // text_input_active 中はマウス進む/戻るも UI 側へ流さない (= text 編集の邪魔をしない)。
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA6))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA7))));
    }

    #[test]
    fn native_video_fullscreen_shortcut_key_ignores_alt_combos() {
        let mut event = key(0x20);
        assert!(native_video_fullscreen_shortcut_key(&event));

        event.alt = true;
        assert!(!native_video_fullscreen_shortcut_key(&event));
    }

    #[test]
    fn seek_status_waits_for_delay_and_holds_after_completion() {
        let now = std::time::Instant::now();
        assert!(!super::seek_status_visible_for_times(
            true,
            Some(now),
            None,
            now + super::SEEK_STATUS_DELAY / 2
        ));
        assert!(super::seek_status_visible_for_times(
            true,
            Some(now),
            None,
            now + super::SEEK_STATUS_DELAY
        ));
        assert!(super::seek_status_visible_for_times(
            false,
            None,
            Some(now),
            now + super::SEEK_STATUS_MIN_VISIBLE / 2
        ));
        assert!(!super::seek_status_visible_for_times(
            false,
            None,
            Some(now),
            now + super::SEEK_STATUS_MIN_VISIBLE
        ));
    }

    #[test]
    fn seek_status_delay_resets_for_new_seek_generation() {
        let now = std::time::Instant::now();
        let second_seek_started = now + std::time::Duration::from_millis(100);
        assert!(!super::seek_status_visible_for_times(
            true,
            Some(second_seek_started),
            None,
            now + super::SEEK_STATUS_DELAY
        ));
    }

    #[test]
    fn center_pause_controls_visibility_requires_initial_paused_ready_video() {
        let base = super::CenterPauseControlsInputs {
            tile_overlay_visible: false,
            is_playing: false,
            frame_step_active: false,
            initial_pause_controls_pending: true,
            first_frame_presented: true,
            has_video_error: false,
            normalize_scanning: false,
        };
        assert!(super::center_pause_controls_visible_for_state(base));

        let cases = [
            super::CenterPauseControlsInputs {
                tile_overlay_visible: true,
                ..base
            },
            super::CenterPauseControlsInputs {
                is_playing: true,
                ..base
            },
            super::CenterPauseControlsInputs {
                frame_step_active: true,
                ..base
            },
            super::CenterPauseControlsInputs {
                initial_pause_controls_pending: false,
                ..base
            },
            super::CenterPauseControlsInputs {
                first_frame_presented: false,
                ..base
            },
            super::CenterPauseControlsInputs {
                has_video_error: true,
                ..base
            },
            super::CenterPauseControlsInputs {
                normalize_scanning: true,
                ..base
            },
        ];
        for inputs in cases {
            assert!(!super::center_pause_controls_visible_for_state(inputs));
        }
    }

    #[test]
    fn center_seek_status_hides_behind_pause_controls() {
        assert!(super::center_seek_status_visible(true, false));
        assert!(!super::center_seek_status_visible(true, true));
        assert!(!super::center_seek_status_visible(false, false));
    }

    #[test]
    fn native_checkmark_rect_matches_top_right_indicator_position() {
        let rect = super::overlay_draw::native_checkmark_rect(1920.0, 28.0);

        assert!((rect.center().x - 1890.0).abs() < f32::EPSILON);
        assert!((rect.center().y - 46.0).abs() < f32::EPSILON);
        assert!((rect.width() - 39.6).abs() < 0.001);
        assert!((rect.height() - 39.6).abs() < 0.001);
    }

    /// 1:1 SAR では従来の isotropic transform と一致 (regression-safe を保証)。
    #[test]
    fn compute_video_visual_transform_sar_1_1_is_isotropic() {
        let (m11, m22, ox, oy) =
            compute_video_visual_transform(1920, 1080, 3840, 2160, 1, 1, false);
        assert!(
            (m11 - m22).abs() < 1e-6,
            "M11 ({m11}) should equal M22 ({m22})"
        );
        assert!((m11 - 2.0).abs() < 1e-6, "1920->3840 should be 2x");
        assert!(ox.abs() < 1e-3, "centered horizontally");
        assert!(oy.abs() < 1e-3, "centered vertically");
    }

    /// SAR 97/80 (= 1.2125、本件動画) で 720x480 を 1920x1080 に letterbox fit。
    /// 表示寸法 = 720*1.2125 × 480 = 873x480、aspect 1.819:1 なので window 16:9 に
    /// 横いっぱい (1920 幅) 入るはず。M11/M22 比は SAR と一致する。
    #[test]
    fn compute_video_visual_transform_anamorphic_97_80() {
        let (m11, m22, ox, oy) =
            compute_video_visual_transform(720, 480, 1920, 1080, 97, 80, false);
        let ratio = m11 / m22;
        assert!(
            (ratio - 1.2125).abs() < 1e-4,
            "M11/M22 should equal SAR 1.2125 (got {ratio})"
        );
        // display_w = 720 * 1.2125 = 873, display_h = 480, win 1920x1080
        // scale = min(1920/873, 1080/480) = min(2.199, 2.25) = 2.199
        // → 横いっぱい、縦に余白
        let expected_scale = 1920.0_f32 / (720.0 * 97.0 / 80.0);
        assert!((m22 - expected_scale).abs() < 1e-3);
        assert!(ox.abs() < 0.5, "should fit horizontally edge-to-edge");
        assert!(oy > 0.0, "should letterbox vertically");
    }

    /// SAR 0/0 (未指定)・0/1・1/0 はすべて 1:1 として扱う (= max(1) で正規化)。
    #[test]
    fn compute_video_visual_transform_zero_sar_normalizes_to_one() {
        let baseline = compute_video_visual_transform(720, 480, 1920, 1080, 1, 1, false);
        assert_eq!(
            compute_video_visual_transform(720, 480, 1920, 1080, 0, 0, false),
            baseline,
            "0/0 must equal 1/1"
        );
        assert_eq!(
            compute_video_visual_transform(720, 480, 1920, 1080, 0, 1, false),
            baseline,
            "0/1 must equal 1/1"
        );
        assert_eq!(
            compute_video_visual_transform(720, 480, 1920, 1080, 1, 0, false),
            baseline,
            "1/0 must equal 1/1"
        );
    }

    /// SAR < 1 (縦アナモフィック、稀) でも letterbox が成立する。
    #[test]
    fn compute_video_visual_transform_vertical_anamorphic() {
        let (m11, m22, _ox, _oy) =
            compute_video_visual_transform(960, 540, 1920, 1080, 1, 2, false);
        // SAR=0.5 → display_w=480, display_h=540, ratio M11/M22 = 0.5
        let ratio = m11 / m22;
        assert!((ratio - 0.5).abs() < 1e-4);
    }

    /// Compact mode (VST3 panel 表示時の 1/4 領域) でも SAR 補正が正しく適用される。
    #[test]
    fn compute_video_visual_transform_compact_mode_respects_sar() {
        let (m11_normal, m22_normal, _, _) =
            compute_video_visual_transform(720, 480, 1920, 1080, 97, 80, false);
        let (m11_compact, m22_compact, ox, oy) =
            compute_video_visual_transform(720, 480, 1920, 1080, 97, 80, true);
        // compact = 1/4 領域なので scale も半分。M11/M22 比は SAR で同じ。
        let ratio_normal = m11_normal / m22_normal;
        let ratio_compact = m11_compact / m22_compact;
        assert!((ratio_normal - ratio_compact).abs() < 1e-5);
        assert!(m11_compact < m11_normal, "compact should fit smaller area");
        // compact 領域は右上 (target_x = win_w/2 = 960、target_y = 0)
        assert!(ox >= 960.0, "compact target starts at right half");
        assert!(oy >= 0.0);
    }

    /// 不正な (極めて小さい / 0) ウィンドウやサーフェイスでも panic しない。
    #[test]
    fn compute_video_visual_transform_handles_zero_dims() {
        // surface 0/0 → max(1) 正規化、計算が NaN/inf にならず数値で返ること
        let (m11, m22, ox, oy) = compute_video_visual_transform(0, 0, 1920, 1080, 1, 1, false);
        assert!(m11.is_finite() && m22.is_finite() && ox.is_finite() && oy.is_finite());
        let (m11, m22, ox, oy) = compute_video_visual_transform(720, 480, 0, 0, 1, 1, false);
        assert!(m11.is_finite() && m22.is_finite() && ox.is_finite() && oy.is_finite());
    }

    #[test]
    fn copy_cpu_rgba_to_swapchain_bgra_swaps_red_and_blue() {
        let src = [
            0x10, 0x20, 0x30, 0x40, //
            0xaa, 0xbb, 0xcc, 0xdd,
        ];
        let mut dst = Vec::new();

        copy_cpu_rgba_to_swapchain_bgra(&src, &mut dst, 2, 1).unwrap();

        assert_eq!(
            dst,
            [
                0x30, 0x20, 0x10, 0x40, //
                0xcc, 0xbb, 0xaa, 0xdd,
            ]
        );
    }

    #[test]
    fn copy_cpu_rgba_to_swapchain_bgra_rejects_short_input() {
        let mut dst = Vec::new();

        let err = copy_cpu_rgba_to_swapchain_bgra(&[0, 1, 2], &mut dst, 1, 1).unwrap_err();

        assert!(err.contains("too small"));
    }

    #[test]
    fn sample_cpu_rgba_pixel_returns_bgra_at_center() {
        let src = [
            1, 2, 3, 4, //
            5, 6, 7, 8, //
            9, 10, 11, 12, //
            13, 14, 15, 16,
        ];

        let sample = sample_cpu_rgba_pixel(&src, 2, 2, 87).unwrap();

        assert_eq!(sample.x, 1);
        assert_eq!(sample.y, 1);
        assert_eq!((sample.b, sample.g, sample.r, sample.a), (15, 14, 13, 16));
    }

    #[test]
    fn compare_pixel_probe_detects_channel_mismatch() {
        let expected = NativePixelSample {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            format: 87,
            b: 10,
            g: 20,
            r: 30,
            a: 255,
        };
        let actual = NativePixelSample {
            r: 10,
            b: 30,
            ..expected
        };

        let err = compare_pixel_probe("cpu_upload", expected, actual).unwrap_err();

        assert!(err.contains("mismatch"));
    }

    #[test]
    fn metadata_clean_text_preserves_description_line_breaks() {
        let text = " line one  with   spaces\\n\\nline two\r\n  line three  ";

        assert_eq!(
            metadata_clean_text(text),
            "line one with spaces\n\nline two\nline three"
        );
    }
}
