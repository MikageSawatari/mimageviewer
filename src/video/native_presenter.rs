use std::time::Instant;

use serde_json::Value;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11Device1, ID3D11Device5, ID3D11DeviceContext,
    ID3D11DeviceContext1, ID3D11DeviceContext4, ID3D11Fence, ID3D11RenderTargetView,
    ID3D11Resource, ID3D11Texture2D, ID3D11View,
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
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGIOutput, IDXGISwapChain1,
    IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::Interface;

use crate::video::decoder::{VideoFrame, VideoFrameData};

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
    _video_visual: IDCompositionVisual,
    backbuffer: Option<ID3D11Texture2D>,
    test_overlay: Option<NativeTestOverlay>,
    egui_overlay: Option<NativeEguiOverlay>,
    fence_cache: Option<(u64, isize, ID3D11Fence)>,
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
    _visual: IDCompositionVisual,
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
    width: u32,
    height: u32,
}

pub struct NativePresentOutcome {
    pub path: &'static str,
    pub wait_ms: f64,
    pub wait_timed_out: bool,
    pub fence_wait_ms: f64,
    pub copy_ms: f64,
    pub present_ms: f64,
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
            let video_visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual video: {e:?}"))?;
            video_visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent video: {e:?}"))?;
            root_visual
                .AddVisual(&video_visual, false, None::<&IDCompositionVisual>)
                .map_err(|e| format!("IDCompositionVisual::AddVisual video: {e:?}"))?;
            let mut egui_overlay = None;
            if config.egui_overlay {
                let overlay_visual = dcomp_device
                    .CreateVisual()
                    .map_err(|e| format!("CreateVisual egui overlay: {e:?}"))?;
                match NativeEguiOverlay::new(overlay_visual, config.width, config.height) {
                    Ok(mut overlay) => {
                        root_visual
                            .AddVisual(&overlay._visual, true, &video_visual)
                            .map_err(|e| {
                                format!("IDCompositionVisual::AddVisual egui overlay: {e:?}")
                            })?;
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
                _video_visual: video_visual,
                backbuffer: None,
                test_overlay,
                egui_overlay,
                fence_cache: None,
                width: config.width,
                height: config.height,
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
                .map_err(|e| format!("IDXGISwapChain::ResizeBuffers: {e:?}"))?;
        }
        self.width = width;
        self.height = height;
        self.recreate_backbuffer(false)?;
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
        let timed_out = wait_result == WAIT_TIMEOUT;

        let copy_t0 = Instant::now();
        let mut fence_wait_ms = 0.0;
        let path = match &frame.data {
            VideoFrameData::Cpu(bytes) => {
                let backbuffer = self
                    .backbuffer
                    .as_ref()
                    .ok_or_else(|| "native presenter backbuffer is not initialized".to_string())?;
                unsafe {
                    self.d3d_context.UpdateSubresource(
                        backbuffer,
                        0,
                        None,
                        bytes.as_ptr().cast(),
                        frame.width.saturating_mul(4),
                        0,
                    );
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
                let fence = self.open_fence(gpu_frame.fence_gen, gpu_frame.fence_shared_handle)?;
                let fence_t0 = Instant::now();
                unsafe {
                    self.d3d_context4
                        .Wait(&fence, gpu_frame.fence_value)
                        .map_err(|e| format!("D3D11 fence wait: {e:?}"))?;
                }
                fence_wait_ms = fence_t0.elapsed().as_secs_f64() * 1000.0;
                let src: ID3D11Texture2D = unsafe {
                    self.d3d_device1
                        .OpenSharedResource1(gpu_frame.shared_handle)
                        .map_err(|e| format!("OpenSharedResource1 frame texture: {e:?}"))?
                };
                unsafe {
                    let backbuffer = self.backbuffer.as_ref().ok_or_else(|| {
                        "native presenter backbuffer is not initialized".to_string()
                    })?;
                    let dst_res: ID3D11Resource = backbuffer
                        .cast()
                        .map_err(|e| format!("cast backbuffer resource: {e:?}"))?;
                    let src_res: ID3D11Resource = src
                        .cast()
                        .map_err(|e| format!("cast source resource: {e:?}"))?;
                    self.d3d_context.CopyResource(&dst_res, &src_res);
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
        let present_ms = present_t0.elapsed().as_secs_f64() * 1000.0;
        Ok(NativePresentOutcome {
            path,
            wait_ms,
            wait_timed_out: timed_out,
            fence_wait_ms,
            copy_ms,
            present_ms,
        })
    }

    pub fn handle_window_events(
        &mut self,
        events: &[crate::video::native_window::NativeVideoWindowEvent],
    ) -> Result<(), String> {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.push_native_events(events);
            overlay.render_if_dirty()?;
        }
        Ok(())
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

impl NativeEguiOverlay {
    fn new(visual: IDCompositionVisual, width: u32, height: u32) -> Result<Self, String> {
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
        let this = Self {
            surface,
            _visual: visual,
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
        self.render_once()
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
                let pos = native_pos(mouse.x, mouse.y);
                self.pointer_pos = Some(pos);
                self.modifiers = egui_modifiers(mouse.shift, mouse.ctrl, false);
                self.pending_events.push(egui::Event::PointerMoved(pos));
                self.dirty = true;
            }
            NativeEvent::MouseButton(button) => {
                let pos = native_pos(button.x, button.y);
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
                let pos = native_pos(wheel.x, wheel.y);
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

    fn render_if_dirty(&mut self) -> Result<(), String> {
        if !self.dirty && self.pending_events.is_empty() {
            return Ok(());
        }
        self.render_once()
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

    fn render_once(&mut self) -> Result<(), String> {
        let render_t0 = Instant::now();
        let ppp = 1.0f32;
        let event_count = self.event_count;
        let pointer_pos = self.pointer_pos;
        let pending_event_count = self.pending_events.len();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(self.width as f32 / ppp, self.height as f32 / ppp),
            )),
            time: Some(self.started_at.elapsed().as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.pending_events),
            ..Default::default()
        };
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("native_egui_overlay_spike"),
            ));
            let rect = egui::Rect::from_min_size(egui::pos2(32.0, 32.0), egui::vec2(360.0, 74.0));
            painter.rect_filled(
                rect,
                egui::CornerRadius::same(6),
                egui::Color32::from_rgba_premultiplied(0, 46, 21, 168),
            );
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(46.0, 48.0), egui::vec2(28.0, 28.0)),
                egui::CornerRadius::same(4),
                egui::Color32::from_rgba_premultiplied(0, 92, 38, 188),
            );
            painter.text(
                egui::pos2(88.0, 46.0),
                egui::Align2::LEFT_TOP,
                "DComp egui overlay",
                egui::FontId::proportional(19.0),
                egui::Color32::from_rgb(210, 246, 218),
            );
            painter.text(
                egui::pos2(88.0, 72.0),
                egui::Align2::LEFT_TOP,
                "wgpu SurfaceTarget::CompositionVisual",
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(158, 206, 172),
            );
            let pointer = pointer_pos
                .map(|p| format!("{:.0},{:.0}", p.x, p.y))
                .unwrap_or_else(|| "outside".to_string());
            painter.text(
                egui::pos2(88.0, 90.0),
                egui::Align2::LEFT_TOP,
                format!("events={event_count} pointer={pointer}"),
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(132, 188, 148),
            );
        });

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
                ("shapes", Value::from(shape_count as i64)),
                ("paint_jobs", Value::from(paint_jobs.len() as i64)),
                (
                    "render_ms",
                    Value::from(render_t0.elapsed().as_secs_f64() * 1000.0),
                ),
            ],
        );
        Ok(())
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

fn native_pos(x: i32, y: i32) -> egui::Pos2 {
    egui::pos2(x as f32, y as f32)
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
