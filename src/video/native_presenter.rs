use std::collections::{HashMap, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11Device1, ID3D11Device5,
    ID3D11DeviceContext, ID3D11DeviceContext1, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11RenderTargetView, ID3D11Resource, ID3D11Texture2D, ID3D11View,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
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
use windows::core::Interface;
use windows_numerics::Matrix3x2;

use crate::video::decoder::{VideoFrame, VideoFrameData};

const SHARED_TEXTURE_CACHE_CAPACITY: usize = 64;

pub struct NativePresenterConfig {
    pub hwnd: HWND,
    pub width: u32,
    pub height: u32,
    pub test_overlay: bool,
    pub egui_overlay: bool,
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
    fence_cache: Option<(u64, isize, ID3D11Fence)>,
    shared_texture_cache: Vec<(u64, ID3D11Texture2D)>,
    cpu_upload_scratch: Vec<u8>,
    pixel_probe_enabled: bool,
    pixel_probe_strict: bool,
    last_pixel_probe: Option<Instant>,
    video_compact: bool,
    width: u32,
    height: u32,
    surface_width: u32,
    surface_height: u32,
}

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
    after_visual: IDCompositionVisual,
    _instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    present_mode: wgpu::PresentMode,
    alpha_mode: wgpu::CompositeAlphaMode,
    renderer: egui_wgpu::Renderer,
    egui_ctx: egui::Context,
    started_at: Instant,
    pending_events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    pointer_pos: Option<egui::Pos2>,
    event_count: u64,
    dirty: bool,
    wants_pointer_input: bool,
    wants_keyboard_input: bool,
    video_position_secs: f64,
    video_duration_secs: f64,
    video_is_playing: bool,
    video_volume: f64,
    video_muted: bool,
    video_playback_speed: f64,
    video_speed_popup_open: bool,
    video_loop_enabled: bool,
    video_checked: bool,
    vst3_available: bool,
    vst3_panel: Option<NativeOverlayVst3Panel>,
    first_frame_presented: bool,
    video_error: Option<String>,
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
    hover_thumbnail: Option<NativeOverlayThumbnail>,
    hover_texture: Option<egui::TextureHandle>,
    hover_texture_key: Option<(u32, u32, u64)>,
    timeline_markers: Vec<NativeOverlayTimelineMarker>,
    jump_entries: Vec<NativeOverlayJumpEntry>,
    video_metadata: Option<NativeOverlayMetadata>,
    tile_overlay: Option<NativeOverlayTileOverlay>,
    tile_textures: HashMap<usize, (u64, egui::TextureHandle)>,
    jump_textures: HashMap<usize, (u64, egui::TextureHandle)>,
    top_bar_visible: bool,
    right_panel_visible: bool,
    jump_panel_visible: bool,
    pending_overlay_commands: Vec<NativeOverlayCommand>,
    last_volume_target: Option<f64>,
    visual_attached: bool,
    pixels_per_point: f32,
    width: u32,
    height: u32,
}

#[derive(Clone, Debug)]
struct NativeOverlayToast {
    text: String,
    started_at: Instant,
    centered: bool,
}

pub struct NativePresentOutcome {
    pub path: &'static str,
    pub shared_handle: u64,
    pub shared_cache_hit: bool,
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
    pub source_delta_ms: f32,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayVst3Panel {
    pub visible: bool,
    pub video_compact: bool,
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
}

impl NativeOverlayTileOverlay {
    pub fn preparing() -> Self {
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
}

pub struct NativeOverlayInputOutcome {
    pub routing: NativeOverlayInputRouting,
    pub commands: Vec<NativeOverlayCommand>,
}

impl NativeOverlayInputOutcome {
    fn empty() -> Self {
        Self {
            routing: NativeOverlayInputRouting::default(),
            commands: Vec::new(),
        }
    }
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
    ToggleLoop,
    AddBookmarkAt {
        target_secs: f64,
    },
    TogglePinAt {
        target_secs: f64,
    },
    DeleteBookmark {
        id: i64,
    },
}

impl NativeOverlayInputRouting {
    pub fn should_forward_to_ui(
        self,
        event: crate::video::native_window::NativeVideoWindowEvent,
    ) -> bool {
        use crate::video::native_window::NativeVideoWindowEvent as NativeEvent;

        match event {
            NativeEvent::KeyDown(_) | NativeEvent::KeyUp(_) => !self.wants_keyboard_input,
            NativeEvent::MouseMove(_)
            | NativeEvent::MouseButton(_)
            | NativeEvent::MouseWheel(_)
            | NativeEvent::MouseLeave => !self.wants_pointer_input,
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
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(data) = std::fs::read(r"C:\Windows\Fonts\seguiemj.ttf") {
        fonts.font_data.insert(
            "emoji".to_owned(),
            Arc::new(egui::FontData::from_owned(data)),
        );
        // Prefer the emoji font for symbol/emoji codepoints in the standalone
        // native overlay; Japanese text still falls through to the UI font.
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "emoji".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "emoji".to_owned());
    }
    for path in [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ] {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        fonts.font_data.insert(
            "japanese".to_owned(),
            Arc::new(egui::FontData::from_owned(data)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let family_fonts = fonts.families.entry(family).or_default();
            let insert_at = if family_fonts.iter().any(|name| name == "emoji") {
                1
            } else {
                0
            };
            family_fonts.insert(insert_at, "japanese".to_owned());
        }
        break;
    }
    ctx.set_fonts(fonts);
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
            let mut egui_overlay = None;
            if config.egui_overlay {
                let overlay_visual = dcomp_device
                    .CreateVisual()
                    .map_err(|e| format!("CreateVisual egui overlay: {e:?}"))?;
                match NativeEguiOverlay::new(
                    overlay_visual,
                    &dcomp_device,
                    &root_visual,
                    &video_visual,
                    config.hwnd,
                    config.width,
                    config.height,
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
                fence_cache: None,
                shared_texture_cache: Vec::new(),
                cpu_upload_scratch: Vec::new(),
                pixel_probe_enabled: std::env::var_os("MIV_NATIVE_VIDEO_PIXEL_PROBE").is_some(),
                pixel_probe_strict: std::env::var_os("MIV_NATIVE_VIDEO_PIXEL_PROBE_STRICT")
                    .is_some(),
                last_pixel_probe: None,
                video_compact: false,
                width: config.width,
                height: config.height,
                surface_width: config.width,
                surface_height: config.height,
            };
            this.recreate_backbuffer(true)?;
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

    pub fn present(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
    ) -> Result<NativePresentOutcome, String> {
        let wait_t0 = Instant::now();
        let wait_result = unsafe { WaitForSingleObject(self.waitable, 100) };
        let wait_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        let present_waitable_ms = wait_ms;
        let timed_out = wait_result == WAIT_TIMEOUT;

        let copy_t0 = Instant::now();
        let mut fence_wait_ms = 0.0;
        let mut open_shared_ms = 0.0;
        let mut keyed_mutex_ms = 0.0;
        let mut keyed_mutex_cast_ms = 0.0;
        let mut keyed_mutex_acquire_ms = 0.0;
        let copy_call_ms;
        let mut shared_handle = 0;
        let mut shared_cache_hit = false;
        let path = match &frame.data {
            VideoFrameData::Cpu(bytes) => {
                self.ensure_video_surface_size(frame.width, frame.height)?;
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
                let backbuffer = self
                    .backbuffer
                    .as_ref()
                    .ok_or_else(|| "native presenter backbuffer is not initialized".to_string())?;
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
                    copy_call_ms = copy_call_t0.elapsed().as_secs_f64() * 1000.0;
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
                "cpu_upload"
            }
            VideoFrameData::Gpu(gpu_frame) => {
                if gpu_frame.ten_bit {
                    return Err("10-bit D3D11 frame is not supported by native presenter".into());
                }
                if gpu_frame.width != frame.width || gpu_frame.height != frame.height {
                    return Err("D3D11 frame metadata size mismatch".into());
                }
                shared_handle = gpu_frame.shared_handle.0 as usize as u64;
                self.ensure_video_surface_size(gpu_frame.width, gpu_frame.height)?;
                let probe_this_frame = self.pixel_probe_due();
                let fence = self.open_fence(gpu_frame.fence_gen, gpu_frame.fence_shared_handle)?;
                let fence_t0 = Instant::now();
                unsafe {
                    self.d3d_context4
                        .Wait(&fence, gpu_frame.fence_value)
                        .map_err(|e| format!("D3D11 fence wait: {e:?}"))?;
                }
                fence_wait_ms = fence_t0.elapsed().as_secs_f64() * 1000.0;
                let open_shared_t0 = Instant::now();
                let (src, cache_hit) = self.open_shared_texture(gpu_frame.shared_handle)?;
                shared_cache_hit = cache_hit;
                open_shared_ms = open_shared_t0.elapsed().as_secs_f64() * 1000.0;
                let keyed_mutex_t0 = Instant::now();
                let keyed_mutex = self.acquire_source_keyed_mutex(
                    &src,
                    gpu_frame.shared_output_released_to_reader.clone(),
                )?;
                keyed_mutex_ms = keyed_mutex_t0.elapsed().as_secs_f64() * 1000.0;
                keyed_mutex_cast_ms = keyed_mutex.cast_ms;
                keyed_mutex_acquire_ms = keyed_mutex.acquire_ms;
                let _keyed_mutex_guard = keyed_mutex.guard;
                let src_probe = if probe_this_frame {
                    Some(self.sample_texture_pixel(&src, "source")?)
                } else {
                    None
                };
                unsafe {
                    let backbuffer = self
                        .backbuffer
                        .as_ref()
                        .ok_or_else(|| {
                            "native presenter backbuffer is not initialized".to_string()
                        })?
                        .clone();
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
                    copy_call_ms = copy_call_t0.elapsed().as_secs_f64() * 1000.0;
                    if probe_this_frame {
                        let backbuffer_probe =
                            self.sample_texture_pixel(&backbuffer, "backbuffer")?;
                        self.log_pixel_probe(
                            "d3d11_shared",
                            gpu_frame.fence_gen,
                            gpu_frame.fence_value,
                            fence_wait_ms,
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
                "d3d11_shared"
            }
        };
        let copy_ms = copy_t0.elapsed().as_secs_f64() * 1000.0;
        let present_t0 = Instant::now();
        let hr = unsafe { self.swap_chain.Present(sync_interval, Default::default()) };
        if hr.is_err() {
            return Err(format!("IDXGISwapChain::Present: {hr:?}"));
        }
        let present_call_ms = present_t0.elapsed().as_secs_f64() * 1000.0;
        let present_ms = present_call_ms;
        Ok(NativePresentOutcome {
            path,
            shared_handle,
            shared_cache_hit,
            wait_ms,
            wait_timed_out: timed_out,
            fence_wait_ms,
            open_shared_ms,
            keyed_mutex_ms,
            keyed_mutex_cast_ms,
            keyed_mutex_acquire_ms,
            copy_call_ms,
            copy_ms,
            present_waitable_ms,
            present_call_ms,
            present_ms,
        })
    }

    pub fn handle_window_events(
        &mut self,
        events: &[crate::video::native_window::NativeVideoWindowEvent],
    ) -> Result<NativeOverlayInputOutcome, String> {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.push_native_events(events);
            return overlay.render_if_dirty();
        }
        Ok(NativeOverlayInputOutcome::empty())
    }

    pub fn update_overlay_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        playback_speed: f64,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.update_video_state(
                position_secs,
                duration_secs,
                is_playing,
                volume,
                muted,
                playback_speed,
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
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_playback_status(first_frame_presented, error);
        }
    }

    pub fn show_overlay_toast(&mut self, text: String, centered: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.show_toast(text, centered);
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
        playback_speed: f64,
    ) -> Result<(), String> {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            let force_tick_render = overlay.wants_periodic_tick();
            overlay.update_video_state(
                position_secs,
                duration_secs,
                is_playing,
                volume,
                muted,
                playback_speed,
            );
            if force_tick_render {
                overlay.dirty = true;
            }
            overlay.render_if_dirty()?;
        }
        Ok(())
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

    fn open_shared_texture(
        &mut self,
        shared_handle: HANDLE,
    ) -> Result<(ID3D11Texture2D, bool), String> {
        let handle_key = shared_handle.0 as usize as u64;
        if let Some(pos) = self
            .shared_texture_cache
            .iter()
            .position(|(cached_key, _)| *cached_key == handle_key)
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
            .insert(0, (handle_key, texture.clone()));
        if self.shared_texture_cache.len() > SHARED_TEXTURE_CACHE_CAPACITY {
            self.shared_texture_cache.pop();
        }
        Ok((texture, false))
    }

    fn ensure_video_surface_size(&mut self, width: u32, height: u32) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        if self.surface_width == width && self.surface_height == height {
            return Ok(());
        }

        self.backbuffer = None;
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width,
                    height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
                )
                .map_err(|e| format!("IDXGISwapChain::ResizeBuffers video surface: {e:?}"))?;
        }
        self.surface_width = width;
        self.surface_height = height;
        self.recreate_backbuffer(false)?;
        self.update_video_visual_transform()?;
        log_event(
            "surface_resize",
            &[
                ("width", Value::from(self.width as i64)),
                ("height", Value::from(self.height as i64)),
                ("surface_width", Value::from(width as i64)),
                ("surface_height", Value::from(height as i64)),
            ],
        );
        Ok(())
    }

    fn update_video_visual_transform(&self) -> Result<(), String> {
        let surface_width = self.surface_width.max(1) as f32;
        let surface_height = self.surface_height.max(1) as f32;
        let width = self.width.max(1) as f32;
        let height = self.height.max(1) as f32;
        let (target_x, target_y, target_w, target_h) = if self.video_compact {
            (width * 0.5, 0.0, width * 0.5, height * 0.5)
        } else {
            (0.0, 0.0, width, height)
        };
        let scale = (target_w / surface_width).min(target_h / surface_height);
        let offset_x = target_x + (target_w - surface_width * scale) * 0.5;
        let offset_y = target_y + (target_h - surface_height * scale) * 0.5;
        let transform = Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
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
        after_visual: &IDCompositionVisual,
        hwnd: HWND,
        width: u32,
        height: u32,
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
        let pixels_per_point = pixels_per_point_for_hwnd(hwnd);
        let this = Self {
            surface,
            visual,
            dcomp_device: dcomp_device.clone(),
            root_visual: root_visual.clone(),
            after_visual: after_visual.clone(),
            _instance: instance,
            adapter,
            device,
            queue,
            format,
            present_mode,
            alpha_mode,
            renderer,
            egui_ctx,
            started_at: Instant::now(),
            pending_events: Vec::new(),
            modifiers: egui::Modifiers::default(),
            pointer_pos: None,
            event_count: 0,
            dirty: true,
            wants_pointer_input: false,
            wants_keyboard_input: false,
            video_position_secs: 0.0,
            video_duration_secs: 0.0,
            video_is_playing: false,
            video_volume: 1.0,
            video_muted: false,
            video_playback_speed: 1.0,
            video_speed_popup_open: false,
            video_loop_enabled: false,
            video_checked: false,
            vst3_available: false,
            vst3_panel: None,
            first_frame_presented: false,
            video_error: None,
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
            hover_thumbnail: None,
            hover_texture: None,
            hover_texture_key: None,
            timeline_markers: Vec::new(),
            jump_entries: Vec::new(),
            video_metadata: None,
            tile_overlay: None,
            tile_textures: HashMap::new(),
            jump_textures: HashMap::new(),
            top_bar_visible: false,
            right_panel_visible: false,
            jump_panel_visible: false,
            pending_overlay_commands: Vec::new(),
            last_volume_target: None,
            visual_attached: false,
            pixels_per_point,
            width: width.max(1),
            height: height.max(1),
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

    fn push_native_events(
        &mut self,
        events: &[crate::video::native_window::NativeVideoWindowEvent],
    ) {
        for event in events {
            self.push_native_event(*event);
        }
    }

    fn push_native_event(&mut self, event: crate::video::native_window::NativeVideoWindowEvent) {
        use crate::video::native_window::{
            NativeVideoMouseButton, NativeVideoWindowEvent as NativeEvent,
        };

        self.event_count = self.event_count.saturating_add(1);
        match event {
            NativeEvent::KeyDown(key) | NativeEvent::KeyUp(key) => {
                let modifiers = egui_modifiers(key.shift, key.ctrl, key.alt);
                self.modifiers = modifiers;
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
        }
    }

    fn update_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        playback_speed: f64,
    ) {
        let position_secs = finite_nonnegative(position_secs);
        let duration_secs = finite_nonnegative(duration_secs);
        let volume = finite_unit(volume);
        let playback_speed = crate::video::clock::clamp_playback_speed(playback_speed);
        let duration_changed = (self.video_duration_secs - duration_secs).abs() > 0.001;
        let position_changed = (self.video_position_secs - position_secs).abs() >= 0.25;
        let playing_changed = self.video_is_playing != is_playing;
        let volume_changed = (self.video_volume - volume).abs() >= 0.005;
        let muted_changed = self.video_muted != muted;
        let speed_changed = (self.video_playback_speed - playback_speed).abs() >= 1.0e-6;
        if !is_playing {
            self.perf_pause_gap_pending = true;
        }
        self.video_position_secs = position_secs;
        self.video_duration_secs = duration_secs;
        self.video_is_playing = is_playing;
        self.video_volume = volume;
        self.video_muted = muted;
        self.video_playback_speed = playback_speed;
        if duration_changed
            || position_changed
            || playing_changed
            || volume_changed
            || muted_changed
            || speed_changed
        {
            self.dirty = true;
        }
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
            let expected_ms = native_perf_expected_frame_ms_from_samples([*prev, sample])
                .unwrap_or_else(|| {
                    if sample.source_delta_ms.is_finite() && sample.source_delta_ms > 0.5 {
                        sample.source_delta_ms
                    } else if prev.interval_ms.is_finite() && prev.interval_ms > 0.5 {
                        prev.interval_ms
                    } else {
                        16.67
                    }
                });
            sample.arrival =
                prev.arrival + Duration::from_secs_f32((expected_ms / 1000.0).clamp(0.001, 0.25));
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

    fn set_loop_enabled(&mut self, enabled: bool) {
        if self.video_loop_enabled == enabled {
            return;
        }
        self.video_loop_enabled = enabled;
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
        }
        self.dirty = true;
    }

    fn set_vst3_panel(&mut self, panel: Option<NativeOverlayVst3Panel>) {
        if self.vst3_panel == panel {
            return;
        }
        self.vst3_panel = panel;
        self.dirty = true;
    }

    fn set_playback_status(&mut self, first_frame_presented: bool, error: Option<String>) {
        if self.first_frame_presented == first_frame_presented && self.video_error == error {
            return;
        }
        self.first_frame_presented = first_frame_presented;
        self.video_error = error;
        self.dirty = true;
    }

    fn show_toast(&mut self, text: String, centered: bool) {
        if text.trim().is_empty() {
            return;
        }
        self.toast = Some(NativeOverlayToast {
            text,
            started_at: Instant::now(),
            centered,
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
            || self.vst3_panel.as_ref().is_some_and(|panel| panel.visible)
            || self.perf_visible
            || self.tile_overlay.is_some()
            || self.hover_preview_target_secs.is_some()
            || self.toast.is_some()
            || self.video_error.is_some()
            || !self.first_frame_presented
    }

    fn needs_render(&self) -> bool {
        self.dirty || !self.pending_events.is_empty()
    }

    fn render_if_dirty(&mut self) -> Result<NativeOverlayInputOutcome, String> {
        if !self.dirty && self.pending_events.is_empty() {
            return Ok(NativeOverlayInputOutcome {
                routing: self.input_routing(),
                commands: Vec::new(),
            });
        }
        let commands = self.render_once()?;
        Ok(NativeOverlayInputOutcome {
            routing: self.input_routing(),
            commands,
        })
    }

    fn input_routing(&self) -> NativeOverlayInputRouting {
        NativeOverlayInputRouting {
            wants_pointer_input: self.wants_pointer_input,
            wants_keyboard_input: self.wants_keyboard_input,
        }
    }

    fn hud_visible(&self) -> bool {
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        self.pointer_pos
            .is_some_and(|pos| pos.y >= (overlay_height_points - 220.0).max(0.0))
    }

    fn top_bar_visible(&self) -> bool {
        self.pointer_pos.is_some_and(|pos| {
            let y_max = if self.top_bar_visible { 76.0 } else { 36.0 };
            pos.y <= y_max
        })
    }

    fn right_panel_visible(&self) -> bool {
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
                self.root_visual
                    .AddVisual(&self.visual, true, &self.after_visual)
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
        if self.toast.as_ref().is_some_and(|toast| {
            toast.started_at.elapsed()
                > Duration::from_millis(if toast.centered { 2500 } else { 1800 })
        }) {
            self.toast = None;
        }
        self.sync_hover_thumbnail_texture();
        self.sync_tile_overlay_textures();
        self.sync_jump_entry_textures();
        let ppp = self.pixels_per_point;
        let event_count = self.event_count;
        let pointer_pos = self.pointer_pos;
        let overlay_width_points = self.width as f32 / ppp;
        let overlay_height_points = self.height as f32 / ppp;
        let position_secs = self.video_position_secs;
        let duration_secs = self.video_duration_secs;
        let is_playing = self.video_is_playing;
        let volume = self.video_volume;
        let muted = self.video_muted;
        let playback_speed = self.video_playback_speed;
        let checked = self.video_checked;
        let loop_enabled = self.video_loop_enabled;
        let first_frame_presented = self.first_frame_presented;
        let video_error = self.video_error.clone();
        let toast = self.toast.clone();
        let hover_thumbnail = self.hover_thumbnail.clone();
        let hover_texture_id = self.hover_texture.as_ref().map(|texture| texture.id());
        let hover_preview_pinned = self.hover_preview_pinned;
        let timeline_markers = self.timeline_markers.clone();
        let jump_entries = self.jump_entries.clone();
        let video_metadata = self.video_metadata.clone();
        let vst3_panel = self.vst3_panel.clone();
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
        let status_visible = video_error.is_some() || !first_frame_presented;
        let toast_visible = toast.is_some();
        let side_panel_visible =
            !tile_overlay_visible && (jump_panel_visible || right_panel_visible);
        let panel_chrome_visible = !tile_overlay_visible && (top_bar_visible || side_panel_visible);
        let bottom_hud_visible = hud_visible || panel_chrome_visible;
        let paused_center_visible =
            !tile_overlay_visible && !is_playing && first_frame_presented && video_error.is_none();
        let perf_origin = egui::pos2(14.0, 14.0);
        let overlay_visible = tile_overlay_visible
            || bottom_hud_visible
            || panel_chrome_visible
            || perf_visible
            || (!tile_overlay_visible && checked)
            || status_visible
            || toast_visible
            || paused_center_visible
            || vst3_panel_visible;
        let pending_event_count = self.pending_events.len();
        let mut commands = std::mem::take(&mut self.pending_overlay_commands);
        let mut last_seek_target_secs = self.last_seek_target_secs;
        let mut last_thumbnail_request_secs = self.last_thumbnail_request_secs;
        let mut last_thumbnail_request_at = self.last_thumbnail_request_at;
        let mut hover_preview_target_secs = self.hover_preview_target_secs;
        let mut video_speed_popup_open = self.video_speed_popup_open;
        if !overlay_visible {
            self.set_visual_attached(false)?;
            last_seek_target_secs = None;
            last_thumbnail_request_secs = None;
            last_thumbnail_request_at = None;
            hover_preview_target_secs = None;
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
            if perf_visible {
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
                draw_native_center_status(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    "動画を準備中...",
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
                draw_native_vst3_panel(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    panel,
                    &mut commands,
                );
            }
            if paused_center_visible {
                draw_native_center_pause_controls(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &mut commands,
                );
            }
            if let Some(toast) = toast.as_ref() {
                draw_native_toast(ctx, overlay_width_points, overlay_height_points, toast);
            }
            if right_panel_visible && let Some(metadata) = video_metadata.as_ref() {
                draw_native_metadata_panel(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    metadata,
                );
            }
            if jump_panel_visible {
                draw_native_jump_panel(
                    ctx,
                    overlay_height_points,
                    position_secs,
                    &jump_entries,
                    &jump_texture_ids,
                    &mut commands,
                );
            }
            if !bottom_hud_visible {
                return;
            }
            egui::Area::new(egui::Id::new("native_video_seek_hud"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(0.0, (overlay_height_points - 46.0).max(0.0)))
                .show(ctx, |ui| {
                    ui.set_min_size(egui::vec2(overlay_width_points, 46.0));
                    let hud_rect = ui.min_rect();
                    let painter = ui.painter();
                    painter.rect_filled(
                        hud_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 176),
                    );

                    let side_pad = 10.0;
                    let btn_size = 28.0;
                    let gap = 8.0;
                    let center_y = hud_rect.center().y;
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
                    draw_overlay_button_bg(painter, loop_rect, loop_resp.hovered(), loop_enabled);
                    draw_overlay_loop_icon(
                        painter,
                        loop_rect.center(),
                        btn_size * 0.36,
                        if loop_enabled {
                            egui::Color32::from_rgb(170, 230, 255)
                        } else {
                            egui::Color32::from_rgb(238, 238, 238)
                        },
                    );
                    let loop_resp = loop_resp.on_hover_text(if loop_enabled {
                        "ループ再生を解除 [L]"
                    } else {
                        "ループ再生 [L]"
                    });
                    if loop_resp.clicked() {
                        commands.push(NativeOverlayCommand::ToggleLoop);
                    }
                    x = loop_rect.max.x + gap;

                    let time_w = 132.0;
                    let vol_pct_w = 40.0;
                    let vol_slider_w = 90.0;
                    let mute_w = btn_size;
                    let speed_w = btn_size * 1.55;
                    let right_pad =
                        side_pad + vol_pct_w + gap + vol_slider_w + gap + speed_w + gap + mute_w;
                    let bar_min_x = x + time_w + gap;
                    let bar_max_x = (hud_rect.max.x - right_pad).max(bar_min_x + 1.0);
                    let bar_rect = egui::Rect::from_min_max(
                        egui::pos2(bar_min_x, center_y - 4.0),
                        egui::pos2(bar_max_x, center_y + 4.0),
                    );
                    let hit_rect = egui::Rect::from_min_max(
                        egui::pos2(bar_min_x, hud_rect.min.y),
                        egui::pos2(bar_max_x, hud_rect.max.y),
                    );

                    let label = format!(
                        "{} / {}",
                        format_overlay_time(position_secs),
                        format_overlay_time(duration_secs)
                    );
                    painter.text(
                        egui::pos2(x, center_y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(14.0),
                        egui::Color32::from_rgb(238, 238, 238),
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
                        painter.rect_filled(filled, 2.0, egui::Color32::from_rgb(228, 228, 228));
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
                            let frac = ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
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
                            let frac = ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
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
                        let hover_preview_bookmarked =
                            target_has_marker(&timeline_markers, target, duration_secs, |kind| {
                                kind == NativeOverlayTimelineMarkerKind::Bookmark
                            });
                        let preview_image_w = (overlay_width_points * 0.30).clamp(300.0, 352.0);
                        let image_size = egui::vec2(preview_image_w, preview_image_w * 9.0 / 16.0);
                        let action_bar_h = 38.0;
                        let preview_size = egui::vec2(image_size.x, image_size.y + action_bar_h);
                        let preview_x = (x - preview_size.x * 0.5)
                            .clamp(8.0, overlay_width_points - preview_size.x - 8.0);
                        let preview_y = (hud_rect.min.y - preview_size.y - 14.0).max(8.0);
                        let preview_rect = egui::Rect::from_min_size(
                            egui::pos2(preview_x, preview_y),
                            preview_size,
                        );
                        let image_rect = egui::Rect::from_min_size(preview_rect.min, image_size);
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
                            let thumbnail_matches = hover_thumbnail.as_ref().is_some_and(|thumb| {
                                (thumb.target_secs - target).abs()
                                    <= crate::video::thumbnail::SECONDS_PER_BUCKET * 2.0
                            });
                            if thumbnail_matches {
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
                            } else {
                                painter.text(
                                    image_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "loading",
                                    egui::FontId::proportional(12.0),
                                    egui::Color32::from_gray(150),
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
                                "ピン留めを解除"
                            } else {
                                "サムネイルをピン留め"
                            });
                            if pin_resp.clicked() {
                                commands.push(NativeOverlayCommand::TogglePinAt {
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

                    let mute_rect = egui::Rect::from_min_size(
                        egui::pos2(bar_max_x + gap, center_y - btn_size * 0.5),
                        egui::vec2(btn_size, btn_size),
                    );
                    let mute_resp = ui.interact(
                        mute_rect,
                        egui::Id::new("native_video_mute"),
                        egui::Sense::click(),
                    );
                    draw_overlay_button_bg(painter, mute_rect, mute_resp.hovered(), muted);
                    draw_overlay_speaker_icon(painter, mute_rect.center(), btn_size * 0.46, muted);
                    let mute_resp = mute_resp.on_hover_text(if muted {
                        "ミュート解除 [M]"
                    } else {
                        "ミュート [M]"
                    });
                    if mute_resp.clicked() {
                        commands.push(NativeOverlayCommand::ToggleMute);
                    }

                    let speed_rect = egui::Rect::from_min_size(
                        egui::pos2(mute_rect.max.x + gap, center_y - btn_size * 0.5),
                        egui::vec2(speed_w, btn_size),
                    );
                    let speed_resp = ui.interact(
                        speed_rect,
                        egui::Id::new("native_video_speed"),
                        egui::Sense::click(),
                    );
                    draw_overlay_button_bg(painter, speed_rect, speed_resp.hovered(), false);
                    painter.text(
                        speed_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        crate::video::clock::format_playback_speed(playback_speed),
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_rgb(238, 238, 238),
                    );
                    let speed_resp = speed_resp.on_hover_text("再生速度");
                    if speed_resp.clicked() {
                        video_speed_popup_open = !video_speed_popup_open;
                    }
                    if video_speed_popup_open {
                        let popup_w = 356.0_f32.min((overlay_width_points - 16.0).max(180.0));
                        let popup_h = 74.0;
                        let popup_x = (speed_rect.center().x - popup_w * 0.5)
                            .clamp(8.0, overlay_width_points - popup_w - 8.0);
                        let popup_y = (hud_rect.min.y - popup_h - 6.0).max(8.0);
                        let mut selected_speed = None;
                        egui::Area::new(egui::Id::new("native_video_speed_popup"))
                            .order(egui::Order::Foreground)
                            .fixed_pos(egui::pos2(popup_x, popup_y))
                            .show(ctx, |ui| {
                                egui::Frame::new()
                                    .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 225))
                                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(110)))
                                    .corner_radius(egui::CornerRadius::same(4))
                                    .inner_margin(egui::Margin::same(6))
                                    .show(ui, |ui| {
                                        ui.set_min_width(popup_w - 12.0);
                                        ui.horizontal_wrapped(|ui| {
                                            for speed in crate::video::clock::PLAYBACK_SPEED_CHOICES
                                            {
                                                let selected =
                                                    (playback_speed - speed).abs() < 1.0e-6;
                                                let label =
                                                    crate::video::clock::format_playback_speed(
                                                        speed,
                                                    );
                                                let button = egui::Button::new(label)
                                                    .selected(selected)
                                                    .min_size(egui::vec2(46.0, 24.0));
                                                if ui.add(button).clicked() {
                                                    selected_speed = Some(speed);
                                                }
                                            }
                                        });
                                    });
                            });
                        if ui.ctx().input(|i| i.pointer.any_click())
                            && !speed_resp.hovered()
                            && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
                        {
                            let popup_rect = egui::Rect::from_min_size(
                                egui::pos2(popup_x, popup_y),
                                egui::vec2(popup_w, popup_h),
                            );
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

                    let vol_rect = egui::Rect::from_min_max(
                        egui::pos2(speed_rect.max.x + gap, center_y - 4.0),
                        egui::pos2(speed_rect.max.x + gap + vol_slider_w, center_y + 4.0),
                    );
                    painter.rect_filled(vol_rect, 2.0, egui::Color32::from_gray(74));
                    let vol_frac = finite_unit(volume) as f32;
                    let vol_fill = egui::Rect::from_min_max(
                        vol_rect.min,
                        egui::pos2(vol_rect.min.x + vol_rect.width() * vol_frac, vol_rect.max.y),
                    );
                    painter.rect_filled(vol_fill, 2.0, egui::Color32::from_rgb(220, 220, 220));
                    let vol_resp = ui.interact(
                        vol_rect.expand2(egui::vec2(0.0, 10.0)),
                        egui::Id::new("native_video_volume"),
                        egui::Sense::click_and_drag(),
                    );
                    if vol_resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    let vol_resp = vol_resp.on_hover_text("音量 [Shift+↑ / Shift+↓]");
                    if (vol_resp.clicked() || vol_resp.dragged())
                        && let Some(pos) = vol_resp.interact_pointer_pos()
                    {
                        let value =
                            ((pos.x - vol_rect.min.x) / vol_rect.width()).clamp(0.0, 1.0) as f64;
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
                    painter.text(
                        egui::pos2(vol_rect.max.x + gap, center_y),
                        egui::Align2::LEFT_CENTER,
                        format!("{:>3}%", (finite_unit(volume) * 100.0).round() as i32),
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_rgb(238, 238, 238),
                    );

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
        });
        // Query after `run`: egui updates these flags from the just-processed
        // frame, which lets this presenter decide whether to forward the same
        // native input batch to the legacy fullscreen shortcut path.
        self.wants_pointer_input = self.egui_ctx.wants_pointer_input();
        self.wants_keyboard_input = self.egui_ctx.wants_keyboard_input();
        self.last_seek_target_secs = last_seek_target_secs;
        self.last_thumbnail_request_secs = last_thumbnail_request_secs;
        self.last_thumbnail_request_at = last_thumbnail_request_at;
        self.hover_preview_target_secs = hover_preview_target_secs;
        self.video_speed_popup_open = video_speed_popup_open;
        self.top_bar_visible = top_bar_visible || side_panel_visible;
        self.right_panel_visible = right_panel_visible;
        self.jump_panel_visible = jump_panel_visible;

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
        self.dirty = false;
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
}

fn draw_native_perf_overlay(
    ctx: &egui::Context,
    overlay_width_points: f32,
    _overlay_height_points: f32,
    history: &[NativeOverlayPerfSample],
    latest: NativeOverlayPerfSnapshot,
    origin: egui::Pos2,
) {
    egui::Area::new(egui::Id::new("native_video_perf_overlay"))
        .order(egui::Order::Middle)
        .fixed_pos(origin)
        .show(ctx, |ui| {
            let width = overlay_width_points.min(460.0).max(300.0);
            let panel_rect =
                egui::Rect::from_min_size(ui.min_rect().min, egui::vec2(width, 158.0));
            ui.set_min_size(panel_rect.size());
            let painter = ui.painter().clone();
            painter.rect_filled(
                panel_rect,
                5.0,
                egui::Color32::from_rgba_unmultiplied(8, 10, 14, 218),
            );
            painter.rect_stroke(
                panel_rect,
                5.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 42)),
                egui::StrokeKind::Inside,
            );

            let graph = egui::Rect::from_min_max(
                panel_rect.min + egui::vec2(10.0, 48.0),
                panel_rect.max - egui::vec2(10.0, 34.0),
            );
            let title = format!(
                "native {:.1} fps  frames {}  GPU {} CPU {}",
                latest.actual_fps, latest.presented, latest.gpu, latest.cpu
            );
            painter.text(
                panel_rect.min + egui::vec2(10.0, 9.0),
                egui::Align2::LEFT_TOP,
                title,
                egui::FontId::monospace(12.0),
                egui::Color32::from_rgb(235, 238, 244),
            );
            let warn = if latest.late_drop > 0 || latest.wait_timeout > 0 {
                egui::Color32::from_rgb(255, 112, 112)
            } else {
                egui::Color32::from_rgb(154, 236, 178)
            };
            painter.text(
                panel_rect.min + egui::vec2(panel_rect.width() - 10.0, 9.0),
                egui::Align2::RIGHT_TOP,
                format!("drop {} timeout {}", latest.late_drop, latest.wait_timeout),
                egui::FontId::monospace(12.0),
                warn,
            );
            painter.text(
                panel_rect.min + egui::vec2(10.0, 25.0),
                egui::Align2::LEFT_TOP,
                format!(
                    "clock late {:.1}ms  total {:.1}ms  max dt {:.1}ms  t {:.0}s",
                    latest.max_late_ms,
                    latest.max_total_ms,
                    latest.max_interval_ms,
                    latest.elapsed_secs
                ),
                egui::FontId::monospace(10.0),
                egui::Color32::from_rgb(168, 176, 188),
            );

            painter.rect_filled(
                graph,
                2.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 16),
            );
            let expected_ms = native_perf_expected_frame_ms(history);
            let y_max_ms = (expected_ms * 2.0).clamp(8.0, 160.0);
            let y_for_ms = |ms: f32| {
                graph.max.y - (ms.clamp(0.0, y_max_ms) / y_max_ms) * graph.height()
            };
            let grid_lines = [
                (expected_ms * 0.5, format!("{:.1}", expected_ms * 0.5)),
                (expected_ms, format!("{:.1}", expected_ms)),
                (expected_ms * 2.0, format!("{:.0}", expected_ms * 2.0)),
            ];
            for (ms, label) in grid_lines {
                let y = y_for_ms(ms);
                painter.line_segment(
                    [egui::pos2(graph.min.x, y), egui::pos2(graph.max.x, y)],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 34),
                    ),
                );
                painter.text(
                    egui::pos2(graph.max.x - 2.0, y - 1.0),
                    egui::Align2::RIGHT_BOTTOM,
                    label,
                    egui::FontId::monospace(9.0),
                    egui::Color32::from_rgb(160, 166, 176),
                );
            }

            if let Some(last) = history.last() {
                let now = last.arrival;
                let px_per_sec = graph.width() / 6.0;
                let mut prev_interval = None;
                let mut prev_total = None;
                let mut last_draw_x = f32::INFINITY;
                let clipped = painter.with_clip_rect(graph);
                for (idx, sample) in history.iter().enumerate() {
                    let age = now.saturating_duration_since(sample.arrival).as_secs_f32();
                    if age > 6.0 {
                        continue;
                    }
                    let x = graph.max.x - age * px_per_sec;
                    if (last_draw_x - x).abs() < 0.75 && idx + 1 < history.len() {
                        continue;
                    }
                    last_draw_x = x;
                    let interval_y = y_for_ms(sample.interval_ms);
                    let total_y = y_for_ms(sample.total_ms);
                    let copy_y = y_for_ms(sample.copy_ms);
                    let interval_point = egui::pos2(x, interval_y);
                    let total_point = egui::pos2(x, total_y);
                    if let Some(prev) = prev_interval {
                        clipped.line_segment(
                            [prev, interval_point],
                            egui::Stroke::new(1.8, egui::Color32::from_rgb(111, 211, 255)),
                        );
                    }
                    if let Some(prev) = prev_total {
                        clipped.line_segment(
                            [prev, total_point],
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 194, 87)),
                        );
                    }
                    clipped.circle_filled(
                        egui::pos2(x, copy_y),
                        1.4,
                        egui::Color32::from_rgb(178, 236, 135),
                    );
                    if native_perf_sample_has_frame_gap(sample) {
                        clipped.line_segment(
                            [egui::pos2(x, graph.min.y), egui::pos2(x, graph.max.y)],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 95, 95)),
                        );
                    }
                    prev_interval = Some(interval_point);
                    prev_total = Some(total_point);
                }
            }

            let latest_sample = history.last().copied();
            let interval = latest_sample.map(|s| s.interval_ms).unwrap_or(0.0);
            let total = latest_sample.map(|s| s.total_ms).unwrap_or(0.0);
            let copy = latest_sample.map(|s| s.copy_ms).unwrap_or(0.0);
            let waitable = latest_sample.map(|s| s.present_waitable_ms).unwrap_or(0.0);
            let present = latest_sample.map(|s| s.present_call_ms).unwrap_or(0.0);
            let source = latest_sample.map(|s| s.source_delta_ms).unwrap_or(0.0);
            let footer = format!(
                "dt {:>4.1}  total {:>4.1}  copy {:>4.1}  wait {:>4.1}  present {:>4.1}  src {:>4.1}",
                interval, total, copy, waitable, present, source
            );
            painter.text(
                panel_rect.min + egui::vec2(10.0, 137.0),
                egui::Align2::LEFT_TOP,
                footer,
                egui::FontId::monospace(11.0),
                egui::Color32::from_rgb(212, 216, 224),
            );
        });
}

fn draw_native_jump_panel(
    ctx: &egui::Context,
    overlay_height_points: f32,
    position_secs: f64,
    entries: &[NativeOverlayJumpEntry],
    jump_texture_ids: &HashMap<usize, egui::TextureId>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let panel_rect = native_jump_panel_rect(overlay_height_points);

    egui::Area::new(egui::Id::new("native_video_jump_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(panel_rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(panel_rect.size());
            let rect = ui.min_rect();
            let painter = ui.painter().clone();
            painter.rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(14, 14, 18, 232),
            );
            painter.line_segment(
                [rect.right_top(), rect.right_bottom()],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 55),
                ),
            );
            let _ = ui.interact(
                rect,
                egui::Id::new("native_video_jump_panel_bg"),
                egui::Sense::click(),
            );
            painter.text(
                rect.min + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                "ジャンプ",
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(238, 238, 238),
            );

            let pin_rect = egui::Rect::from_min_size(
                rect.min + egui::vec2(rect.width() - 68.0, 6.0),
                egui::vec2(26.0, 24.0),
            );
            let pin_resp = ui.interact(
                pin_rect,
                egui::Id::new("native_jump_pin_here"),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, pin_rect, pin_resp.hovered(), false);
            draw_overlay_pin_icon(
                &painter,
                pin_rect.center(),
                7.0,
                egui::Color32::from_rgb(140, 245, 170),
            );
            let pin_resp = pin_resp.on_hover_text("現在位置をピン留め [P]");
            if pin_resp.clicked() {
                commands.push(NativeOverlayCommand::TogglePinAt {
                    target_secs: position_secs,
                });
            }

            let bm_rect = egui::Rect::from_min_size(
                rect.min + egui::vec2(rect.width() - 36.0, 6.0),
                egui::vec2(26.0, 24.0),
            );
            let bm_resp = ui.interact(
                bm_rect,
                egui::Id::new("native_jump_bookmark_here"),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, bm_rect, bm_resp.hovered(), false);
            draw_overlay_bookmark_icon(
                &painter,
                bm_rect.center(),
                7.0,
                egui::Color32::from_rgb(255, 220, 82),
            );
            let bm_resp = bm_resp.on_hover_text("現在位置をブックマーク [B]");
            if bm_resp.clicked() {
                commands.push(NativeOverlayCommand::AddBookmarkAt {
                    target_secs: position_secs,
                });
            }

            let content_rect = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 34.0), rect.max);
            let mut content_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .max_height(content_rect.height())
                .show(&mut content_ui, |ui| {
                    ui.add_space(6.0);
                    if entries.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.colored_label(
                                egui::Color32::from_gray(170),
                                "ピン・ブックマーク・チャプターはまだありません",
                            );
                        });
                        return;
                    }

                    for kind in [
                        NativeOverlayTimelineMarkerKind::Pin,
                        NativeOverlayTimelineMarkerKind::Bookmark,
                        NativeOverlayTimelineMarkerKind::Chapter,
                    ] {
                        let section_entries: Vec<_> = entries
                            .iter()
                            .enumerate()
                            .filter(|(_, entry)| entry.kind == kind)
                            .collect();
                        if section_entries.is_empty() {
                            continue;
                        }
                        let (label, color) = match kind {
                            NativeOverlayTimelineMarkerKind::Pin => {
                                ("ピン留め", egui::Color32::from_rgb(140, 245, 170))
                            }
                            NativeOverlayTimelineMarkerKind::Bookmark => {
                                ("ブックマーク", egui::Color32::from_rgb(255, 220, 82))
                            }
                            NativeOverlayTimelineMarkerKind::Chapter => {
                                ("チャプター", egui::Color32::from_rgb(115, 210, 255))
                            }
                        };
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.colored_label(color, egui::RichText::new(label).size(12.0));
                        });
                        ui.add_space(3.0);
                        for (idx, entry) in section_entries {
                            draw_native_jump_row(ui, idx, entry, jump_texture_ids, commands);
                        }
                        ui.add_space(8.0);
                    }
                });
        });
}

fn draw_native_jump_row(
    ui: &mut egui::Ui,
    idx: usize,
    entry: &NativeOverlayJumpEntry,
    jump_texture_ids: &HashMap<usize, egui::TextureId>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let row_h = 76.0;
    let row_w = (ui.available_width() - 12.0).max(260.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (row_rect, resp) =
            ui.allocate_exact_size(egui::vec2(row_w, row_h), egui::Sense::click());
        let painter = ui.painter().clone();
        if resp.hovered() {
            painter.rect_filled(
                row_rect,
                4.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 22),
            );
        }
        let thumb_rect =
            egui::Rect::from_min_size(row_rect.min + egui::vec2(6.0, 4.0), egui::vec2(120.0, 68.0));
        painter.rect_filled(thumb_rect, 3.0, egui::Color32::from_rgb(30, 30, 35));
        if let Some(texture_id) = jump_texture_ids.get(&idx) {
            painter.image(
                *texture_id,
                thumb_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.text(
                thumb_rect.center(),
                egui::Align2::CENTER_CENTER,
                "...",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(140),
            );
        }
        painter.rect_stroke(
            thumb_rect,
            3.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(72)),
            egui::StrokeKind::Inside,
        );

        let text_x = thumb_rect.max.x + 10.0;
        let (kind_label, kind_color) = match entry.kind {
            NativeOverlayTimelineMarkerKind::Pin => ("PIN", egui::Color32::from_rgb(140, 245, 170)),
            NativeOverlayTimelineMarkerKind::Bookmark => {
                ("BM", egui::Color32::from_rgb(255, 220, 82))
            }
            NativeOverlayTimelineMarkerKind::Chapter => {
                ("CH", egui::Color32::from_rgb(115, 210, 255))
            }
        };
        painter.text(
            egui::pos2(text_x, row_rect.min.y + 14.0),
            egui::Align2::LEFT_CENTER,
            kind_label,
            egui::FontId::monospace(11.0),
            kind_color,
        );
        painter.text(
            egui::pos2(text_x + 36.0, row_rect.min.y + 14.0),
            egui::Align2::LEFT_CENTER,
            format_overlay_time(entry.pts_secs),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(232, 232, 232),
        );
        let title = entry
            .title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("無題");
        painter.text(
            egui::pos2(text_x, row_rect.min.y + 38.0),
            egui::Align2::LEFT_TOP,
            truncate_overlay_text(title, 22),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(205, 205, 205),
        );

        let mut delete_clicked = false;
        if let Some(id) = entry.bookmark_id {
            let delete_rect = egui::Rect::from_min_size(
                egui::pos2(row_rect.max.x - 28.0, row_rect.min.y + 8.0),
                egui::vec2(22.0, 22.0),
            );
            let delete_resp = ui.interact(
                delete_rect,
                egui::Id::new(("native_jump_delete", id)),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, delete_rect, delete_resp.hovered(), false);
            painter.text(
                delete_rect.center(),
                egui::Align2::CENTER_CENTER,
                "X",
                egui::FontId::monospace(12.0),
                egui::Color32::from_rgb(240, 190, 190),
            );
            let delete_resp = delete_resp.on_hover_text("ブックマークを削除");
            if delete_resp.clicked() {
                delete_clicked = true;
                commands.push(NativeOverlayCommand::DeleteBookmark { id });
            }
        }

        if resp.clicked() && !delete_clicked {
            commands.push(NativeOverlayCommand::Seek {
                target_secs: entry.pts_secs,
            });
        }
    });
}

#[derive(Copy, Clone)]
enum NativeTopButtonGlyph {
    TileGrid,
    PerfGraph,
    Vst3,
    Close,
}

fn draw_native_top_button(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    x: &mut f32,
    y: f32,
    width: f32,
    height: f32,
    gap: f32,
    id: &'static str,
    glyph: NativeTopButtonGlyph,
    active: bool,
    tooltip: &str,
    command: NativeOverlayCommand,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let rect = egui::Rect::from_min_size(egui::pos2(*x, y), egui::vec2(width, height));
    let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::click());
    draw_overlay_button_bg(painter, rect, resp.hovered(), active);
    match glyph {
        NativeTopButtonGlyph::TileGrid => draw_overlay_tile_grid_icon(painter, rect),
        NativeTopButtonGlyph::PerfGraph => draw_overlay_perf_graph_icon(painter, rect),
        NativeTopButtonGlyph::Vst3 => draw_overlay_vst3_top_icon(painter, rect),
        NativeTopButtonGlyph::Close => draw_overlay_close_icon(painter, rect),
    }
    let resp = resp.on_hover_text(tooltip);
    if resp.clicked() {
        commands.push(command);
    }
    *x -= width + gap;
}

fn draw_overlay_tile_grid_icon(painter: &egui::Painter, rect: egui::Rect) {
    let cell = 7.0;
    let gap = 3.0;
    let total = cell * 2.0 + gap;
    let start = rect.center() - egui::vec2(total * 0.5, total * 0.5);
    for row in 0..2 {
        for col in 0..2 {
            let min = start + egui::vec2((cell + gap) * col as f32, (cell + gap) * row as f32);
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(cell, cell)),
                1.5,
                egui::Color32::from_rgb(238, 238, 238),
            );
        }
    }
}

fn draw_overlay_perf_graph_icon(painter: &egui::Painter, rect: egui::Rect) {
    let left = rect.min.x + 6.0;
    let right = rect.max.x - 5.0;
    let top = rect.min.y + 7.0;
    let bottom = rect.max.y - 6.0;
    painter.line_segment(
        [egui::pos2(left, bottom), egui::pos2(right, bottom)],
        egui::Stroke::new(1.0, egui::Color32::from_gray(140)),
    );
    let points = [
        egui::pos2(left, bottom - 3.0),
        egui::pos2(left + 5.0, bottom - 9.0),
        egui::pos2(left + 10.0, bottom - 5.0),
        egui::pos2(left + 15.0, top + 2.0),
        egui::pos2(right, bottom - 12.0),
    ];
    painter.add(egui::Shape::line(
        points.to_vec(),
        egui::Stroke::new(1.7, egui::Color32::from_rgb(170, 230, 255)),
    ));
}

fn draw_overlay_vst3_top_icon(painter: &egui::Painter, rect: egui::Rect) {
    let color = egui::Color32::from_rgb(238, 238, 238);
    let stroke = egui::Stroke::new(1.55, color);
    let base_y = rect.center().y + 5.0;
    let top_y = rect.center().y - 6.0;
    let left = rect.min.x + 5.5;
    let mid = rect.center().x;
    let right = rect.max.x - 5.5;

    painter.line_segment(
        [egui::pos2(left, top_y), egui::pos2(left + 3.4, base_y)],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(left + 3.4, base_y),
            egui::pos2(left + 6.8, top_y),
        ],
        stroke,
    );

    let sx0 = mid - 1.8;
    let sx1 = mid + 4.8;
    let sy0 = top_y + 0.8;
    let sym = rect.center().y - 0.4;
    let sy1 = base_y - 0.8;
    for [a, b] in [
        [egui::pos2(sx0, sy0), egui::pos2(sx1, sy0)],
        [egui::pos2(sx0, sy0), egui::pos2(sx0, sym)],
        [egui::pos2(sx0, sym), egui::pos2(sx1, sym)],
        [egui::pos2(sx1, sym), egui::pos2(sx1, sy1)],
        [egui::pos2(sx0, sy1), egui::pos2(sx1, sy1)],
    ] {
        painter.line_segment([a, b], stroke);
    }

    painter.line_segment(
        [egui::pos2(right - 6.0, top_y), egui::pos2(right, top_y)],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(right - 3.0, top_y),
            egui::pos2(right - 3.0, base_y),
        ],
        stroke,
    );
}

fn draw_overlay_close_icon(painter: &egui::Painter, rect: egui::Rect) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.26;
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(242, 242, 242));
    painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
    painter.line_segment([c + egui::vec2(r, -r), c + egui::vec2(-r, r)], stroke);
}

fn draw_overlay_vst3_gui_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    visible: bool,
    enabled: bool,
) {
    draw_overlay_button_bg(painter, rect, hovered, visible);
    let color = if !enabled {
        egui::Color32::from_gray(84)
    } else if visible {
        egui::Color32::from_rgb(245, 132, 28)
    } else {
        egui::Color32::from_gray(132)
    };
    let fill = if visible {
        egui::Color32::from_rgba_unmultiplied(245, 132, 28, 34)
    } else {
        egui::Color32::from_rgba_unmultiplied(132, 132, 132, 20)
    };
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(15.0, 12.0));
    painter.rect_filled(icon_rect, 2.0, fill);
    painter.rect_stroke(
        icon_rect,
        2.0,
        egui::Stroke::new(1.8, color),
        egui::StrokeKind::Inside,
    );
}

fn draw_native_checkmark(ctx: &egui::Context, overlay_width_points: f32, top: f32) {
    egui::Area::new(egui::Id::new("native_video_checkmark"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let radius = 18.0;
            let center = egui::pos2((overlay_width_points - 30.0).max(radius), top + radius);
            let rect = egui::Rect::from_center_size(center, egui::vec2(radius * 2.2, radius * 2.2));
            ui.allocate_rect(rect, egui::Sense::hover());
            let painter = ui.painter();
            painter.circle_filled(
                center,
                radius,
                egui::Color32::from_rgba_unmultiplied(22, 154, 84, 226),
            );
            painter.circle_stroke(
                center,
                radius,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90),
                ),
            );
            painter.line_segment(
                [
                    center + egui::vec2(-7.0, 0.5),
                    center + egui::vec2(-2.0, 6.0),
                ],
                egui::Stroke::new(3.0, egui::Color32::WHITE),
            );
            painter.line_segment(
                [
                    center + egui::vec2(-2.0, 6.0),
                    center + egui::vec2(8.0, -7.0),
                ],
                egui::Stroke::new(3.0, egui::Color32::WHITE),
            );
        });
}

fn draw_native_center_status(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    title: &str,
    body: Option<&str>,
    is_error: bool,
) {
    egui::Area::new(egui::Id::new("native_video_center_status"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter();
            let box_w = overlay_width_points.clamp(360.0, 720.0);
            let box_h = if body.is_some() { 132.0 } else { 76.0 };
            let rect = egui::Rect::from_center_size(full_rect.center(), egui::vec2(box_w, box_h));
            painter.rect_filled(
                rect,
                8.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214),
            );
            let title_color = if is_error {
                egui::Color32::from_rgb(255, 120, 120)
            } else {
                egui::Color32::from_rgb(238, 238, 238)
            };
            painter.text(
                egui::pos2(rect.center().x, rect.min.y + 26.0),
                egui::Align2::CENTER_CENTER,
                title,
                egui::FontId::proportional(22.0),
                title_color,
            );
            if let Some(body) = body {
                let body_rect = egui::Rect::from_min_max(
                    rect.min + egui::vec2(22.0, 52.0),
                    rect.max - egui::vec2(22.0, 14.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(egui::Layout::top_down(egui::Align::Center)),
                );
                child.add(
                    egui::Label::new(
                        egui::RichText::new(body)
                            .size(14.0)
                            .color(egui::Color32::from_gray(230)),
                    )
                    .wrap(),
                );
            }
        });
}

fn draw_native_center_pause_controls(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    egui::Area::new(egui::Id::new("native_video_center_pause_controls"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter().clone();
            let radius = 56.0;
            let gap = 34.0;
            let center_y = full_rect.center().y;
            let replay_center = egui::pos2(full_rect.center().x - radius - gap * 0.5, center_y);
            let play_center = egui::pos2(full_rect.center().x + radius + gap * 0.5, center_y);

            let replay_rect =
                egui::Rect::from_center_size(replay_center, egui::vec2(radius * 2.0, radius * 2.0));
            let play_rect =
                egui::Rect::from_center_size(play_center, egui::vec2(radius * 2.0, radius * 2.0));

            let replay_resp = ui
                .interact(
                    replay_rect,
                    egui::Id::new("native_center_replay"),
                    egui::Sense::click(),
                )
                .on_hover_text("最初から再生 [W]");
            let play_resp = ui
                .interact(
                    play_rect,
                    egui::Id::new("native_center_play"),
                    egui::Sense::click(),
                )
                .on_hover_text("続きから再生 [Enter]");

            for (rect, hovered) in [
                (replay_rect, replay_resp.hovered()),
                (play_rect, play_resp.hovered()),
            ] {
                painter.circle_filled(
                    rect.center(),
                    radius,
                    if hovered {
                        egui::Color32::from_rgba_unmultiplied(40, 40, 46, 238)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214)
                    },
                );
                painter.circle_stroke(
                    rect.center(),
                    radius,
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
                    ),
                );
            }
            draw_overlay_replay_icon(&painter, replay_center, 22.0);
            draw_overlay_play_icon(&painter, play_center, 24.0);
            painter.text(
                replay_center + egui::vec2(0.0, radius + 22.0),
                egui::Align2::CENTER_CENTER,
                "最初から",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
            painter.text(
                play_center + egui::vec2(0.0, radius + 22.0),
                egui::Align2::CENTER_CENTER,
                "続きから",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
            painter.text(
                egui::pos2(full_rect.center().x, center_y + radius + 52.0),
                egui::Align2::CENTER_CENTER,
                "Enter: 再生 / W: 頭出し / ←→: シーク / J,K: マーカー移動",
                egui::FontId::proportional(13.0),
                egui::Color32::from_gray(205),
            );

            if replay_resp.clicked() {
                commands.push(NativeOverlayCommand::SeekToStartAndPlay);
            }
            if play_resp.clicked() {
                commands.push(NativeOverlayCommand::TogglePlay);
            }
        });
}

fn draw_native_toast(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    toast: &NativeOverlayToast,
) {
    let elapsed = toast.started_at.elapsed().as_secs_f32();
    let duration = if toast.centered { 2.5 } else { 1.8 };
    let alpha = if elapsed > duration - 0.35 {
        ((duration - elapsed) / 0.35).clamp(0.0, 1.0)
    } else {
        1.0
    };
    if alpha <= 0.0 {
        return;
    }
    egui::Area::new(egui::Id::new("native_video_toast"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter();
            let font = egui::FontId::proportional(if toast.centered { 24.0 } else { 16.0 });
            let galley =
                painter.layout_no_wrap(toast.text.clone(), font.clone(), egui::Color32::WHITE);
            let padding = if toast.centered {
                egui::vec2(28.0, 18.0)
            } else {
                egui::vec2(16.0, 10.0)
            };
            let max_w = (overlay_width_points - 40.0).max(160.0);
            let size = egui::vec2(
                (galley.size().x + padding.x * 2.0).min(max_w),
                galley.size().y + padding.y * 2.0,
            );
            let rect = if toast.centered {
                egui::Rect::from_center_size(full_rect.center(), size)
            } else {
                egui::Rect::from_min_size(
                    egui::pos2(full_rect.max.x - size.x - 20.0, full_rect.min.y + 62.0),
                    size,
                )
            };
            painter.rect_filled(
                rect,
                8.0,
                egui::Color32::from_rgba_unmultiplied(24, 24, 28, (alpha * 224.0) as u8),
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &toast.text,
                font,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8),
            );
        });
}

fn draw_native_top_bar(
    ctx: &egui::Context,
    overlay_width_points: f32,
    position_secs: f64,
    duration_secs: f64,
    metadata: Option<&NativeOverlayMetadata>,
    perf_visible: bool,
    vst3_available: bool,
    vst3_panel_visible: bool,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    egui::Area::new(egui::Id::new("native_video_top_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(overlay_width_points, 54.0));
            ui.set_min_size(rect.size());
            let painter = ui.painter().clone();
            painter.rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 186),
            );
            let name = metadata
                .and_then(|m| {
                    m.title
                        .as_ref()
                        .filter(|title| !title.trim().is_empty())
                        .or(Some(&m.file_name))
                })
                .map(String::as_str)
                .unwrap_or("video");
            painter.text(
                egui::pos2(14.0, 20.0),
                egui::Align2::LEFT_CENTER,
                truncate_overlay_text(name, 88),
                egui::FontId::proportional(15.0),
                egui::Color32::from_rgb(240, 240, 240),
            );
            let sub = if let Some(m) = metadata {
                format!(
                    "{}x{}  {}  {}  {}",
                    m.width,
                    m.height,
                    format_fps(m.avg_fps),
                    m.video_codec,
                    format_overlay_time(position_secs)
                )
            } else {
                format!(
                    "{} / {}",
                    format_overlay_time(position_secs),
                    format_overlay_time(duration_secs)
                )
            };
            painter.text(
                egui::pos2(14.0, 39.0),
                egui::Align2::LEFT_CENTER,
                truncate_overlay_text(&sub, 120),
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(190, 190, 190),
            );

            let btn_size = 28.0;
            let gap = 8.0;
            let mut x = overlay_width_points - 12.0 - btn_size;
            let y = 13.0;

            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_top_close",
                NativeTopButtonGlyph::Close,
                false,
                "動画を終了",
                NativeOverlayCommand::CloseFullscreen,
                commands,
            );
            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_top_tile",
                NativeTopButtonGlyph::TileGrid,
                false,
                "サムネイル一覧 [S]",
                NativeOverlayCommand::ToggleTileMode,
                commands,
            );
            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_top_perf",
                NativeTopButtonGlyph::PerfGraph,
                perf_visible,
                "Perfグラフ [P]",
                NativeOverlayCommand::TogglePerfOverlay,
                commands,
            );
            if vst3_available {
                draw_native_top_button(
                    ui,
                    &painter,
                    &mut x,
                    y,
                    btn_size,
                    btn_size,
                    gap,
                    "native_top_vst3",
                    NativeTopButtonGlyph::Vst3,
                    vst3_panel_visible,
                    "VST3 パネル表示/非表示",
                    NativeOverlayCommand::ToggleVst3Gui,
                    commands,
                );
            }
        });
}

fn draw_native_vst3_panel(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    panel: &NativeOverlayVst3Panel,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let rect = native_vst3_panel_rect(overlay_width_points, overlay_height_points, panel);
    egui::Area::new(egui::Id::new("native_video_vst3_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            let frame = egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(14, 14, 18, 238))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 58),
                ))
                .inner_margin(egui::Margin::same(10));
            frame.show(ui, |ui| {
                ui.set_max_size(rect.size() - egui::vec2(20.0, 20.0));
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("VST3")
                            .strong()
                            .color(egui::Color32::from_rgb(242, 242, 242)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(&panel.state_text)
                            .small()
                            .color(egui::Color32::from_rgb(178, 188, 202)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(egui::RichText::new("X").monospace())
                            .on_hover_text("VST3 パネルを閉じる")
                            .clicked()
                        {
                            commands
                                .push(NativeOverlayCommand::SetVst3PanelVisible { visible: false });
                        }
                    });
                });

                if let Some(reason) = panel.disabled_reason.as_ref() {
                    ui.add_space(6.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(238, 184, 88),
                        "このセッションでは VST3 が一時停止しています",
                    );
                    ui.label(
                        egui::RichText::new(reason)
                            .small()
                            .color(egui::Color32::from_rgb(208, 208, 208)),
                    );
                    return;
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("動画").small());
                    let full_resp = ui.selectable_label(!panel.video_compact, "フル");
                    if full_resp
                        .on_hover_text("動画をフルスクリーン全体に表示します")
                        .clicked()
                    {
                        commands.push(NativeOverlayCommand::SetVst3VideoCompact { compact: false });
                    }
                    let compact_resp = ui.selectable_label(panel.video_compact, "右上 1/4");
                    if compact_resp
                        .on_hover_text("動画を右上 1/4 に縮小し、プラグイン GUI の領域を空けます")
                        .clicked()
                    {
                        commands.push(NativeOverlayCommand::SetVst3VideoCompact { compact: true });
                    }
                });

                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("native_vst3_panel_scroll")
                    .max_height(native_vst3_slot_list_height(panel))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if panel.slots.is_empty() {
                            ui.label(
                                egui::RichText::new("プラグイン未設定")
                                    .color(egui::Color32::from_rgb(190, 190, 190)),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "環境設定の VST3 ページでチェーンに追加してください。",
                                )
                                .small()
                                .color(egui::Color32::from_rgb(160, 160, 160)),
                            );
                        }
                        for slot in &panel.slots {
                            draw_native_vst3_slot_row(ui, slot, commands);
                        }
                    });

                ui.separator();
                ui.label(
                    egui::RichText::new("チェーンスロット")
                        .small()
                        .strong()
                        .color(egui::Color32::from_rgb(210, 210, 210)),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("読込").small());
                    for chain in &panel.chain_slots {
                        let response = ui
                            .add_enabled(
                                chain.name.is_some(),
                                egui::Button::new(chain.key_label.clone()).small(),
                            )
                            .on_hover_text(native_vst3_chain_slot_tooltip(chain));
                        if response.clicked() {
                            commands.push(NativeOverlayCommand::Vst3LoadChainSlot {
                                slot_idx: chain.idx,
                            });
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("保存").small());
                    for chain in &panel.chain_slots {
                        let response = ui
                            .add(egui::Button::new(chain.key_label.clone()).small())
                            .on_hover_text(native_vst3_chain_slot_tooltip(chain));
                        if response.clicked() {
                            commands.push(NativeOverlayCommand::Vst3SaveChainSlot {
                                slot_idx: chain.idx,
                            });
                        }
                    }
                });
            });
        });
}

fn draw_native_vst3_slot_row(
    ui: &mut egui::Ui,
    slot: &NativeOverlayVst3Slot,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    ui.horizontal(|ui| {
        let mut enabled = !slot.bypass;
        let label = format!("{}. {}", slot.idx + 1, slot.name);
        let checkbox = ui.add_enabled(
            !slot.placeholder,
            egui::Checkbox::new(&mut enabled, truncate_overlay_text(&label, 42)),
        );
        if checkbox.on_hover_text("ON/OFF を切り替えます").changed() {
            commands.push(NativeOverlayCommand::Vst3SetBypass {
                idx: slot.idx,
                path: slot.path.clone(),
                bypass: !enabled,
            });
        }
        if slot.placeholder {
            ui.label(
                egui::RichText::new("読込中")
                    .small()
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
        } else if slot.state == NativeOverlayVst3SlotState::Loading {
            ui.label(
                egui::RichText::new("loading")
                    .small()
                    .color(egui::Color32::from_rgb(170, 200, 255)),
            );
        } else if slot.state == NativeOverlayVst3SlotState::Error {
            ui.label(
                egui::RichText::new("error")
                    .small()
                    .color(egui::Color32::from_rgb(245, 120, 120)),
            );
        }
        if let Some(ms) = slot.latency_ms
            && ms > 0.0
            && !slot.bypass
        {
            ui.label(
                egui::RichText::new(format!("{ms:.1}ms"))
                    .small()
                    .color(egui::Color32::from_rgb(255, 206, 116)),
            );
        }
        if slot.auto_bypassed_for_latency && slot.bypass {
            ui.label(
                egui::RichText::new("auto-OFF")
                    .small()
                    .strong()
                    .color(egui::Color32::from_rgb(255, 150, 150)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(28.0, 22.0), egui::Sense::click());
            draw_overlay_vst3_gui_icon(
                ui.painter(),
                rect,
                response.hovered(),
                slot.gui_visible,
                !slot.placeholder,
            );
            if response
                .on_hover_text(if slot.gui_visible {
                    "プラグイン GUI を閉じる"
                } else {
                    "プラグイン GUI を表示"
                })
                .clicked()
                && !slot.placeholder
            {
                if slot.gui_visible {
                    commands.push(NativeOverlayCommand::Vst3HideSlotGui {
                        idx: slot.idx,
                        path: slot.path.clone(),
                    });
                } else {
                    commands.push(NativeOverlayCommand::Vst3ShowSlotGui {
                        idx: slot.idx,
                        path: slot.path.clone(),
                    });
                }
            }
        });
    });
}

fn native_vst3_chain_slot_tooltip(slot: &NativeOverlayVst3ChainSlot) -> String {
    match slot.name.as_ref() {
        Some(name) => format!("{}\n{} 件", name, slot.plugin_count),
        None => format!("VST3 Slot {} は空です", slot.key_label),
    }
}

fn draw_native_metadata_panel(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    metadata: &NativeOverlayMetadata,
) {
    let rect = native_metadata_panel_rect(overlay_width_points, overlay_height_points);
    egui::Area::new(egui::Id::new("native_video_metadata_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            let rect = ui.min_rect();
            let painter = ui.painter();
            painter.rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(14, 14, 18, 232),
            );
            painter.line_segment(
                [rect.left_top(), rect.left_bottom()],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 55),
                ),
            );
            let _ = ui.interact(
                rect,
                egui::Id::new("native_video_metadata_panel_bg"),
                egui::Sense::click(),
            );
            painter.text(
                rect.min + egui::vec2(14.0, 14.0),
                egui::Align2::LEFT_TOP,
                "動画メタ情報",
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(238, 238, 238),
            );

            let title = metadata
                .title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(&metadata.file_name);
            let path_kind = if metadata.gpu_path_active {
                "GPU (D3D11 zero-copy)"
            } else {
                "CPU (readback + swscale)"
            };
            let decode_kind = if metadata.hw_decode_active {
                "HW"
            } else {
                "SW"
            };
            let d3d11va = if metadata.d3d11va_supported {
                "対応"
            } else {
                "非対応"
            };
            let mut rows = vec![
                ("ファイル", metadata.file_name.clone()),
                ("タイトル", title.to_string()),
                ("アーティスト", metadata.artist.clone().unwrap_or_default()),
                ("説明", metadata.description.clone().unwrap_or_default()),
                (
                    "動画",
                    format!(
                        "{}x{}  {}  {}",
                        metadata.width,
                        metadata.height,
                        format_fps(metadata.avg_fps),
                        metadata.video_codec
                    ),
                ),
                ("デコーダ", metadata.video_decoder.clone()),
                (
                    "音声",
                    metadata
                        .audio_codec
                        .clone()
                        .unwrap_or_else(|| "なし".to_string()),
                ),
                ("ビットレート", format_bitrate(metadata.bit_rate_bps)),
                ("長さ", format_overlay_time(metadata.duration_secs)),
                ("チャプター", metadata.chapter_count.to_string()),
                ("経路", path_kind.to_string()),
                ("デコード", decode_kind.to_string()),
                ("D3D11VA", d3d11va.to_string()),
            ];
            rows.retain(|(_, value)| !metadata_clean_text(value).is_empty());

            let content_rect = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 38.0), rect.max);
            let mut content_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            egui::ScrollArea::vertical()
                .id_salt("native_video_metadata_scroll")
                .auto_shrink([false; 2])
                .max_height(content_rect.height())
                .show(&mut content_ui, |ui| {
                    ui.add_space(6.0);
                    for (label, value) in rows {
                        let value = metadata_clean_text(&value);
                        ui.horizontal_top(|ui| {
                            ui.add_space(14.0);
                            ui.add_sized(
                                egui::vec2(88.0, 18.0),
                                egui::Label::new(
                                    egui::RichText::new(label)
                                        .monospace()
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(150)),
                                ),
                            );
                            ui.vertical(|ui| {
                                ui.set_width((rect.width() - 118.0).max(160.0));
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(value)
                                            .size(12.0)
                                            .color(egui::Color32::from_rgb(230, 230, 230)),
                                    )
                                    .wrap(),
                                );
                            });
                        });
                        ui.add_space(7.0);
                    }
                });
        });
}

fn draw_native_tile_overlay(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    state: &NativeOverlayTileOverlay,
    tile_texture_ids: &HashMap<usize, egui::TextureId>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    egui::Area::new(egui::Id::new("native_video_tile_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter();
            painter.rect_filled(full_rect, 0.0, egui::Color32::BLACK);
            let _ = ui.interact(
                full_rect,
                egui::Id::new("native_video_tile_overlay_bg"),
                egui::Sense::click(),
            );

            let interval = format_tile_interval(state.interval_secs);
            let header = format!(
                "タイル モード - 間隔 {interval} - {}/{}  [S]",
                state.progress_done, state.progress_total
            );
            painter.text(
                egui::pos2(16.0, 24.0),
                egui::Align2::LEFT_CENTER,
                header,
                egui::FontId::proportional(14.0),
                egui::Color32::from_rgb(224, 224, 224),
            );
            let close_rect = egui::Rect::from_min_size(
                egui::pos2((overlay_width_points - 44.0).max(8.0), 10.0),
                egui::vec2(32.0, 32.0),
            );
            let close_resp = ui.interact(
                close_rect,
                egui::Id::new("native_video_tile_close"),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(painter, close_rect, close_resp.hovered(), false);
            draw_overlay_close_icon(painter, close_rect);
            if close_resp.on_hover_text("動画に戻る [S]").clicked() {
                commands.push(NativeOverlayCommand::ToggleTileMode);
            }

            if state.progress_done == 0 && !state.finished {
                painter.text(
                    full_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "動画を準備中...",
                    egui::FontId::proportional(20.0),
                    egui::Color32::from_gray(180),
                );
            }

            let columns = state.columns.max(1);
            let label_h = 16.0;
            let gap_x = 6.0;
            let gap_y = 6.0;
            let grid_left = 16.0;
            let total_grid_w = (overlay_width_points - grid_left * 2.0).max(240.0);
            let tile_w = ((total_grid_w - gap_x * columns.saturating_sub(1) as f32)
                / columns as f32)
                .floor()
                .max(40.0);
            let aspect_h = if state.tile_w > 0 && state.tile_h > 0 {
                state.tile_h as f32 / state.tile_w as f32
            } else {
                9.0 / 16.0
            };
            let tile_h = (tile_w * aspect_h).round().max(30.0);
            let grid_top = 56.0;

            for idx in 0..state.timestamps.len() {
                let col = idx % columns;
                let row = idx / columns;
                let x0 = grid_left + (tile_w + gap_x) * col as f32;
                let y0 = grid_top + (tile_h + label_h + gap_y) * row as f32;
                let tile_rect =
                    egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(tile_w, tile_h));
                if tile_rect.max.y > overlay_height_points - 20.0 {
                    continue;
                }

                painter.rect_filled(tile_rect, 4.0, egui::Color32::from_rgb(28, 28, 32));
                if let Some(texture_id) = tile_texture_ids.get(&idx) {
                    painter.image(
                        *texture_id,
                        tile_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    painter.text(
                        tile_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "...",
                        egui::FontId::proportional(20.0),
                        egui::Color32::from_gray(120),
                    );
                }
                painter.rect_stroke(
                    tile_rect,
                    4.0,
                    egui::Stroke::new(1.0, egui::Color32::from_gray(82)),
                    egui::StrokeKind::Inside,
                );

                let pts = state.timestamps.get(idx).copied().unwrap_or(0.0);
                painter.text(
                    egui::pos2(tile_rect.center().x, tile_rect.max.y + label_h * 0.5),
                    egui::Align2::CENTER_CENTER,
                    format_overlay_time(pts),
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_rgb(220, 220, 220),
                );

                let resp = ui.interact(
                    tile_rect,
                    egui::Id::new(("native_video_tile", idx)),
                    egui::Sense::click(),
                );
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    painter.rect_stroke(
                        tile_rect.expand(1.0),
                        4.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(235, 235, 235)),
                        egui::StrokeKind::Inside,
                    );
                }
                if resp.clicked() {
                    commands.push(NativeOverlayCommand::TileSeek { target_secs: pts });
                }
            }
        });
}

fn native_perf_expected_frame_ms(history: &[NativeOverlayPerfSample]) -> f32 {
    native_perf_expected_frame_ms_from_values(
        history
            .iter()
            .rev()
            .take(180)
            .map(|sample| sample.source_delta_ms),
    )
    .unwrap_or(16.67)
}

fn native_perf_expected_frame_ms_from_samples<I>(samples: I) -> Option<f32>
where
    I: IntoIterator<Item = NativeOverlayPerfSample>,
{
    native_perf_expected_frame_ms_from_values(
        samples.into_iter().map(|sample| sample.source_delta_ms),
    )
}

fn native_perf_expected_frame_ms_from_values<I>(values: I) -> Option<f32>
where
    I: IntoIterator<Item = f32>,
{
    let mut values: Vec<f32> = values
        .into_iter()
        .filter(|value| value.is_finite() && *value > 0.5 && *value < 250.0)
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(values[values.len() / 2].clamp(1.0, 120.0))
}

fn native_perf_sample_has_frame_gap(sample: &NativeOverlayPerfSample) -> bool {
    let interval = sample.interval_ms;
    if !interval.is_finite() || interval <= 0.0 {
        return false;
    }
    let expected = if sample.source_delta_ms.is_finite() && sample.source_delta_ms > 1.0 {
        sample.source_delta_ms
    } else {
        16.67
    };
    let threshold = (expected * 1.35).max(expected + 4.0);
    interval > threshold
}

fn thumbnail_rgba_key(thumbnail: &NativeOverlayThumbnail) -> u64 {
    let ptr = Arc::as_ptr(&thumbnail.rgba) as usize as u64;
    ptr ^ thumbnail.target_secs.to_bits()
}

fn fit_rect_in_rect(content_size: egui::Vec2, outer: egui::Rect) -> egui::Rect {
    if content_size.x <= 0.0 || content_size.y <= 0.0 {
        return outer;
    }
    let scale = (outer.width() / content_size.x).min(outer.height() / content_size.y);
    let size = content_size * scale;
    egui::Rect::from_center_size(outer.center(), size)
}

fn native_jump_panel_width() -> f32 {
    320.0
}

fn native_metadata_panel_width() -> f32 {
    430.0
}

fn native_panel_top() -> f32 {
    56.0
}

fn native_panel_hover_bottom(overlay_height_points: f32) -> f32 {
    (overlay_height_points - 48.0).max(native_panel_top())
}

fn native_panel_hover_rect(
    min: egui::Pos2,
    size: egui::Vec2,
    overlay_height_points: f32,
) -> egui::Rect {
    let bottom = native_panel_hover_bottom(overlay_height_points);
    egui::Rect::from_min_max(egui::pos2(min.x, 0.0), egui::pos2(min.x + size.x, bottom))
}

fn native_jump_panel_rect(overlay_height_points: f32) -> egui::Rect {
    let top = native_panel_top();
    let panel_h = (overlay_height_points - top - 44.0).max(240.0);
    egui::Rect::from_min_size(
        egui::pos2(0.0, top),
        egui::vec2(native_jump_panel_width(), panel_h),
    )
}

fn native_metadata_panel_rect(overlay_width_points: f32, overlay_height_points: f32) -> egui::Rect {
    let panel_w = native_metadata_panel_width().min(overlay_width_points * 0.5);
    let top = native_panel_top();
    let panel_h = (overlay_height_points - top - 44.0).max(260.0);
    egui::Rect::from_min_size(
        egui::pos2(overlay_width_points - panel_w, top),
        egui::vec2(panel_w, panel_h),
    )
}

fn native_vst3_panel_rect(
    overlay_width_points: f32,
    overlay_height_points: f32,
    panel: &NativeOverlayVst3Panel,
) -> egui::Rect {
    let width = 380.0_f32.min((overlay_width_points - 32.0).max(260.0));
    let row_count = panel.slots.len().max(1).min(10) as f32;
    let desired_height = 154.0 + row_count * 28.0;
    let max_height = (overlay_height_points - native_panel_top() - 56.0).max(240.0);
    let height = desired_height.clamp(236.0, max_height.min(620.0));
    egui::Rect::from_min_size(
        egui::pos2(18.0, native_panel_top() + 10.0),
        egui::vec2(width, height),
    )
}

fn native_vst3_slot_list_height(panel: &NativeOverlayVst3Panel) -> f32 {
    let row_count = panel.slots.len().max(1).min(10) as f32;
    (row_count * 28.0 + 8.0).min(288.0)
}

fn metadata_clean_text(value: &str) -> String {
    let normalized = value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n");
    let mut lines = Vec::new();
    let mut last_was_blank = true;
    for line in normalized.lines() {
        let cleaned = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if cleaned.is_empty() {
            if !last_was_blank {
                lines.push(String::new());
                last_was_blank = true;
            }
        } else {
            lines.push(cleaned);
            last_was_blank = false;
        }
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn timeline_markers_match(
    a: &[NativeOverlayTimelineMarker],
    b: &[NativeOverlayTimelineMarker],
) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(a, b)| a.kind == b.kind && (a.pts_secs - b.pts_secs).abs() <= f64::EPSILON)
}

fn jump_entries_match(a: &[NativeOverlayJumpEntry], b: &[NativeOverlayJumpEntry]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(a, b)| {
            a.kind == b.kind
                && a.bookmark_id == b.bookmark_id
                && a.title == b.title
                && (a.pts_secs - b.pts_secs).abs() <= f64::EPSILON
                && a.thumbnail.as_ref().map(thumbnail_rgba_key)
                    == b.thumbnail.as_ref().map(thumbnail_rgba_key)
        })
}

fn target_has_marker(
    markers: &[NativeOverlayTimelineMarker],
    target_secs: f64,
    duration_secs: f64,
    kind_matches: impl Fn(NativeOverlayTimelineMarkerKind) -> bool,
) -> bool {
    let bucket_window = crate::video::thumbnail::SECONDS_PER_BUCKET * 1.5;
    let visual_window = (duration_secs / 300.0).clamp(0.15, 1.5);
    let tolerance = bucket_window.max(visual_window);
    markers.iter().any(|marker| {
        kind_matches(marker.kind) && (marker.pts_secs - target_secs).abs() <= tolerance
    })
}

fn draw_timeline_marker(
    painter: &egui::Painter,
    bar_rect: egui::Rect,
    duration_secs: f64,
    marker: NativeOverlayTimelineMarker,
) {
    if duration_secs <= 0.0 || !marker.pts_secs.is_finite() {
        return;
    }
    let frac = (marker.pts_secs / duration_secs).clamp(0.0, 1.0) as f32;
    let x = bar_rect.min.x + bar_rect.width() * frac;
    let (height, color) = match marker.kind {
        NativeOverlayTimelineMarkerKind::Pin => (30.0, egui::Color32::from_rgb(140, 245, 170)),
        NativeOverlayTimelineMarkerKind::Bookmark => (28.0, egui::Color32::from_rgb(255, 220, 82)),
        NativeOverlayTimelineMarkerKind::Chapter => (24.0, egui::Color32::from_rgb(115, 210, 255)),
    };
    let top = bar_rect.center().y - height * 0.5;
    let bottom = bar_rect.center().y + height * 0.5;
    painter.line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(2.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 150)),
    );
    painter.line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(1.0, color),
    );
}

fn draw_overlay_button_bg(painter: &egui::Painter, rect: egui::Rect, hovered: bool, active: bool) {
    let bg = if active {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 190)
    } else if hovered {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 34)
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 4.0, bg);
}

fn draw_overlay_play_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x - r * 0.45, c.y - r * 0.70),
            egui::pos2(c.x - r * 0.45, c.y + r * 0.70),
            egui::pos2(c.x + r * 0.65, c.y),
        ],
        egui::Color32::WHITE,
        egui::Stroke::NONE,
    ));
}

fn draw_overlay_pause_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let stroke = egui::Stroke::new((r * 0.34).max(2.0), egui::Color32::WHITE);
    painter.line_segment(
        [
            egui::pos2(c.x - r * 0.35, c.y - r),
            egui::pos2(c.x - r * 0.35, c.y + r),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + r * 0.35, c.y - r),
            egui::pos2(c.x + r * 0.35, c.y + r),
        ],
        stroke,
    );
}

fn draw_overlay_replay_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let bar_w = (r * 0.22).max(2.0);
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(c.x - r * 0.80, c.y - r * 0.72),
            egui::pos2(c.x - r * 0.80 + bar_w, c.y + r * 0.72),
        ),
        0.0,
        white,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x - r * 0.35, c.y),
            egui::pos2(c.x + r * 0.55, c.y - r * 0.70),
            egui::pos2(c.x + r * 0.55, c.y + r * 0.70),
        ],
        white,
        egui::Stroke::NONE,
    ));
}

fn draw_overlay_loop_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, color: egui::Color32) {
    let stroke = egui::Stroke::new((r * 0.16).max(1.5), color);
    let left = c.x - r * 0.78;
    let right = c.x + r * 0.78;
    let top = c.y - r * 0.42;
    let bottom = c.y + r * 0.42;
    painter.line_segment([egui::pos2(left, top), egui::pos2(right, top)], stroke);
    painter.line_segment(
        [egui::pos2(right, bottom), egui::pos2(left, bottom)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(left, top), egui::pos2(left, c.y - r * 0.12)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(right, bottom), egui::pos2(right, c.y + r * 0.12)],
        stroke,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(right + r * 0.03, top),
            egui::pos2(right - r * 0.34, top - r * 0.25),
            egui::pos2(right - r * 0.34, top + r * 0.25),
        ],
        color,
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(left - r * 0.03, bottom),
            egui::pos2(left + r * 0.34, bottom - r * 0.25),
            egui::pos2(left + r * 0.34, bottom + r * 0.25),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

fn draw_overlay_bookmark_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, fill: egui::Color32) {
    let rect = egui::Rect::from_center_size(c, egui::vec2(r * 1.10, r * 1.55));
    let notch = egui::pos2(rect.center().x, rect.max.y - r * 0.35);
    painter.add(egui::Shape::convex_polygon(
        vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            notch,
            rect.left_bottom(),
        ],
        fill,
        egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 245, 190)),
    ));
}

fn draw_overlay_pin_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, color: egui::Color32) {
    let stroke = egui::Stroke::new((r * 0.18).max(1.5), color);
    let head = egui::Rect::from_center_size(
        egui::pos2(c.x - r * 0.05, c.y - r * 0.32),
        egui::vec2(r * 0.95, r * 0.48),
    );
    painter.rect_filled(head, 1.5, color);
    painter.line_segment(
        [
            egui::pos2(c.x - r * 0.06, c.y - r * 0.05),
            egui::pos2(c.x + r * 0.32, c.y + r * 0.44),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + r * 0.32, c.y + r * 0.44),
            egui::pos2(c.x + r * 0.08, c.y + r * 0.72),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + r * 0.10, c.y + r * 0.26),
            egui::pos2(c.x - r * 0.48, c.y + r * 0.84),
        ],
        egui::Stroke::new((r * 0.12).max(1.2), color),
    );
}

fn draw_overlay_speaker_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, muted: bool) {
    let white = egui::Color32::WHITE;
    let body = egui::Rect::from_min_max(
        egui::pos2(c.x - r * 0.75, c.y - r * 0.38),
        egui::pos2(c.x - r * 0.40, c.y + r * 0.38),
    );
    painter.rect_filled(body, 1.0, white);
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(body.max.x, body.min.y),
            egui::pos2(c.x + r * 0.10, c.y - r * 0.68),
            egui::pos2(c.x + r * 0.10, c.y + r * 0.68),
            egui::pos2(body.max.x, body.max.y),
        ],
        white,
        egui::Stroke::NONE,
    ));
    if muted {
        let stroke = egui::Stroke::new((r * 0.16).max(2.0), egui::Color32::from_rgb(240, 100, 100));
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.30, c.y - r * 0.50),
                egui::pos2(c.x + r * 0.85, c.y + r * 0.50),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.85, c.y - r * 0.50),
                egui::pos2(c.x + r * 0.30, c.y + r * 0.50),
            ],
            stroke,
        );
    } else {
        let stroke = egui::Stroke::new((r * 0.13).max(1.4), white);
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.35, c.y - r * 0.35),
                egui::pos2(c.x + r * 0.35, c.y + r * 0.35),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.62, c.y - r * 0.55),
                egui::pos2(c.x + r * 0.62, c.y + r * 0.55),
            ],
            stroke,
        );
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

fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn finite_unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn format_overlay_time(secs: f64) -> String {
    let total = finite_nonnegative(secs).round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn format_tile_interval(secs: f64) -> String {
    let secs = finite_nonnegative(secs);
    if secs >= 60.0 {
        format!("{}分", (secs / 60.0).round() as u64)
    } else {
        format!("{}秒", secs.round() as u64)
    }
}

fn format_fps(fps: f64) -> String {
    if fps.is_finite() && fps > 0.0 {
        format!("{fps:.2}fps")
    } else {
        "fps ?".to_string()
    }
}

fn format_bitrate(bit_rate_bps: i64) -> String {
    if bit_rate_bps <= 0 {
        return "unknown".to_string();
    }
    let mbps = bit_rate_bps as f64 / 1_000_000.0;
    if mbps >= 1.0 {
        format!("{mbps:.1}Mbps")
    } else {
        format!("{}kbps", (bit_rate_bps as f64 / 1000.0).round() as i64)
    }
}

fn truncate_overlay_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return out;
        };
        out.push(ch);
    }
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
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
        NativePixelSample, compare_pixel_probe, copy_cpu_rgba_to_swapchain_bgra,
        metadata_clean_text, sample_cpu_rgba_pixel,
    };

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
