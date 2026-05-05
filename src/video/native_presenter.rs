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
    pixel_probe_enabled: bool,
    last_pixel_probe: Option<Instant>,
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
    last_seek_target_secs: Option<f64>,
    visual_attached: bool,
    pixels_per_point: f32,
    width: u32,
    height: u32,
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

#[derive(Clone, Copy, Debug)]
pub enum NativeOverlayCommand {
    Seek { target_secs: f64 },
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
                pixel_probe_enabled: std::env::var_os("MIV_NATIVE_VIDEO_PIXEL_PROBE").is_some(),
                last_pixel_probe: None,
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
                        bytes.as_ptr().cast(),
                        frame.width.saturating_mul(4),
                        0,
                    );
                    copy_call_ms = copy_call_t0.elapsed().as_secs_f64() * 1000.0;
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
                            gpu_frame.fence_gen,
                            gpu_frame.fence_value,
                            fence_wait_ms,
                            src_probe,
                            Some(backbuffer_probe),
                        );
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
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.update_video_state(position_secs, duration_secs, is_playing);
        }
    }

    pub fn tick_overlay_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
    ) -> Result<(), String> {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.update_video_state(position_secs, duration_secs, is_playing);
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
        let scale = (width / surface_width).min(height / surface_height);
        let offset_x = (width - surface_width * scale) * 0.5;
        let offset_y = (height - surface_height * scale) * 0.5;
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
            "native-presenter: pixel_probe fence_gen={fence_gen} fence_value={fence_value} \
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
            last_seek_target_secs: None,
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

    fn update_video_state(&mut self, position_secs: f64, duration_secs: f64, is_playing: bool) {
        let position_secs = finite_nonnegative(position_secs);
        let duration_secs = finite_nonnegative(duration_secs);
        let duration_changed = (self.video_duration_secs - duration_secs).abs() > 0.001;
        let position_changed = (self.video_position_secs - position_secs).abs() >= 0.25;
        let playing_changed = self.video_is_playing != is_playing;
        self.video_position_secs = position_secs;
        self.video_duration_secs = duration_secs;
        self.video_is_playing = is_playing;
        if duration_changed || position_changed || playing_changed {
            self.dirty = true;
        }
    }

    fn native_pos(&self, x: i32, y: i32) -> egui::Pos2 {
        egui::pos2(
            x as f32 / self.pixels_per_point,
            y as f32 / self.pixels_per_point,
        )
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
            .is_some_and(|pos| pos.y >= (overlay_height_points - 76.0).max(0.0))
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
        let ppp = self.pixels_per_point;
        let event_count = self.event_count;
        let pointer_pos = self.pointer_pos;
        let overlay_width_points = self.width as f32 / ppp;
        let overlay_height_points = self.height as f32 / ppp;
        let position_secs = self.video_position_secs;
        let duration_secs = self.video_duration_secs;
        let is_playing = self.video_is_playing;
        let hud_visible = self.hud_visible();
        let pending_event_count = self.pending_events.len();
        let mut commands = Vec::new();
        let mut last_seek_target_secs = self.last_seek_target_secs;
        if !hud_visible {
            self.set_visual_attached(false)?;
            last_seek_target_secs = None;
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
            if !hud_visible {
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

                    let side_pad = 24.0;
                    let time_w = 132.0;
                    let bar_min_x = hud_rect.min.x + side_pad + time_w + 12.0;
                    let bar_max_x = (hud_rect.max.x - side_pad).max(bar_min_x + 1.0);
                    let center_y = hud_rect.center().y;
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
                        egui::pos2(hud_rect.min.x + side_pad, center_y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(14.0),
                        egui::Color32::from_rgb(238, 238, 238),
                    );
                    let play_glyph = if is_playing { "||" } else { ">" };
                    painter.text(
                        egui::pos2(hud_rect.min.x + 10.0, center_y),
                        egui::Align2::CENTER_CENTER,
                        play_glyph,
                        egui::FontId::proportional(15.0),
                        egui::Color32::from_rgb(220, 220, 220),
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
                    if duration_secs > 0.0
                        && seek_resp.hovered()
                        && let Some(pos) = pointer_pos
                    {
                        let x = pos.x.clamp(bar_rect.min.x, bar_rect.max.x);
                        let frac = ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
                        let target = duration_secs * frac as f64;
                        painter.line_segment(
                            [
                                egui::pos2(x, hud_rect.min.y + 6.0),
                                egui::pos2(x, hud_rect.max.y - 6.0),
                            ],
                            egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 214, 106)),
                        );
                        painter.text(
                            egui::pos2(x, hud_rect.min.y - 8.0),
                            egui::Align2::CENTER_BOTTOM,
                            format_overlay_time(target),
                            egui::FontId::proportional(12.0),
                            egui::Color32::from_rgb(255, 232, 160),
                        );
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
        });
        // Query after `run`: egui updates these flags from the just-processed
        // frame, which lets this presenter decide whether to forward the same
        // native input batch to the legacy fullscreen shortcut path.
        self.wants_pointer_input = self.egui_ctx.wants_pointer_input();
        self.wants_keyboard_input = self.egui_ctx.wants_keyboard_input();
        self.last_seek_target_secs = last_seek_target_secs;

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
        if hud_visible {
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
