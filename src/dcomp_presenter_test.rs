use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11Device1, ID3D11Device5, ID3D11DeviceContext,
    ID3D11DeviceContext4, ID3D11Fence, ID3D11RenderTargetView, ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGIOutput, IDXGISwapChain1,
    IDXGISwapChain2,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, IDC_ARROW, LoadCursorW, MSG, PM_REMOVE,
    PeekMessageW, PostQuitMessage, RegisterClassW, SW_SHOW, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_NCCREATE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE,
};
use windows::core::{Interface, w};

use crate::video::decoder::{VideoFrame, VideoFrameData};
use crate::video::engine::actor::state_code;
use crate::video::gpu_renderer::GpuVideoDevice;

#[derive(Clone, Debug)]
pub struct DcompPresenterTestConfig {
    pub path: PathBuf,
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
    pub sync_interval: u32,
    pub start_secs: f64,
}

pub fn parse_config() -> Option<DcompPresenterTestConfig> {
    let args: Vec<String> = std::env::args().collect();
    let mut path = None;
    let mut duration = Duration::from_secs(10);
    let mut width = 1920u32;
    let mut height = 1080u32;
    let mut sync_interval = 1u32;
    let mut start_secs = 0.0f64;
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--dcomp-presenter-test" => {
                if let Some(v) = args.get(i + 1) {
                    path = Some(PathBuf::from(v));
                    i += 1;
                }
            }
            "--dcomp-duration" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<f64>().ok()) {
                    duration = Duration::from_secs_f64(v.max(0.1));
                    i += 1;
                }
            }
            "--dcomp-window-size" => {
                if let Some(v) = args.get(i + 1)
                    && let Some((w, h)) = parse_size(v)
                {
                    width = w;
                    height = h;
                    i += 1;
                }
            }
            "--dcomp-sync-interval" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<u32>().ok()) {
                    sync_interval = v.min(4);
                    i += 1;
                }
            }
            "--dcomp-start" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<f64>().ok()) {
                    start_secs = v.max(0.0);
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    path.map(|path| DcompPresenterTestConfig {
        path,
        duration,
        width,
        height,
        sync_interval,
        start_secs,
    })
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    let w = w.parse::<u32>().ok()?.clamp(64, 16384);
    let h = h.parse::<u32>().ok()?.clamp(64, 16384);
    Some((w, h))
}

pub fn run(config: DcompPresenterTestConfig) -> Result<(), String> {
    let _com = ComApartment::init()?;
    let window = NativeWindow::create(config.width, config.height)?;
    let gpu = GpuVideoDevice::new().map_err(|e| e.to_string())?;
    let mut presenter = DcompPresenter::new(window.hwnd, config.width, config.height, &gpu)?;

    let seek_serial = Arc::new(AtomicU64::new(0));
    let clock = Arc::new(crate::video::clock::AvClock::new(1.0, seek_serial));
    clock.set_muted(true);
    clock.set_fallback_anchor(config.start_secs);
    clock.request_seek(config.start_secs);
    clock.set_playing(true);

    let cancel = Arc::new(AtomicBool::new(false));
    let engine_state = Arc::new(AtomicU8::new(state_code::PLAYING));
    let skipped_frame_count = Arc::new(AtomicU64::new(0));
    let (engine_event_tx, _engine_event_rx) = crossbeam_channel::unbounded();
    let handles = crate::video::decoder::spawn(
        config.path.clone(),
        Arc::clone(&clock),
        Arc::clone(&cancel),
        48_000,
        true,
        Some(Arc::clone(&gpu)),
        engine_state,
        engine_event_tx,
        Arc::clone(&skipped_frame_count),
    );
    let audio_clock = Arc::clone(&clock);
    let audio_rx = handles.audio_rx.clone();
    let audio_cancel = Arc::clone(&cancel);
    let audio_drain = std::thread::Builder::new()
        .name("dcomp-audio-drain".into())
        .spawn(move || {
            let mut active_serial = None;
            while !audio_cancel.load(Ordering::Acquire) {
                match audio_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(frame) => {
                        if active_serial != Some(frame.seek_serial) {
                            active_serial = Some(frame.seek_serial);
                            audio_clock.notify_audio_active();
                            audio_clock.set_audio_pts_jump(frame.pts_secs);
                            audio_clock.clear_seek_target_override(frame.seek_serial);
                        } else {
                            audio_clock.set_audio_pts(frame.pts_secs);
                        }
                        audio_clock.add_audio_tx_queued_secs(-frame.duration_secs);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|e| format!("spawn audio drain: {e}"))?;

    let info = handles
        .info_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("video info timeout: {e}"))?
        .map_err(|e| format!("video open: {e}"))?;
    log_event(
        "open",
        &[
            ("path", Value::from(config.path.display().to_string())),
            ("width", Value::from(info.width as i64)),
            ("height", Value::from(info.height as i64)),
            ("avg_fps", Value::from(info.avg_fps)),
            ("video_codec", Value::from(info.video_codec.clone())),
            ("video_decoder", Value::from(info.video_decoder.clone())),
            ("hw_decode_active", Value::from(info.hw_decode_active)),
            ("gpu_path_active", Value::from(info.gpu_path_active)),
            ("sync_interval", Value::from(config.sync_interval as i64)),
        ],
    );

    let mut stats = PresentStats::default();
    let mut frame_queue = VecDeque::new();
    let mut timeline_base_pts = None;
    let mut timeline_started_at = None;
    let run_started = Instant::now();
    let mut last_present_wall = None;
    let mut quit = false;
    while !quit
        && timeline_started_at
            .map(|started: Instant| started.elapsed() < config.duration)
            .unwrap_or_else(|| run_started.elapsed() < config.duration)
    {
        quit = pump_messages();
        while let Ok(frame) = handles.video_rx.try_recv() {
            if timeline_base_pts.is_none() {
                timeline_base_pts = Some(frame.pts_secs);
                timeline_started_at = Some(Instant::now());
                log_event(
                    "timeline_latch",
                    &[("base_pts", Value::from(frame.pts_secs))],
                );
            }
            frame_queue.push_back(frame);
        }

        let elapsed = timeline_started_at
            .map(|started: Instant| started.elapsed().as_secs_f64() + config.start_secs)
            .unwrap_or(config.start_secs);
        if let Some(frame) = frame_queue.front()
            && frame.pts_secs - timeline_base_pts.unwrap_or(frame.pts_secs) <= elapsed + 0.001
        {
            let frame = frame_queue.pop_front().unwrap();
            let frame_elapsed = frame.pts_secs - timeline_base_pts.unwrap_or(frame.pts_secs);
            let late_ms = ((elapsed - frame_elapsed) * 1000.0).max(0.0);
            if late_ms > 50.0 && frame_queue.len() > 1 {
                stats.late_drop += 1;
                log_event(
                    "late_drop",
                    &[
                        ("pts", Value::from(frame.pts_secs)),
                        ("late_ms", Value::from(late_ms)),
                        ("queue_len", Value::from(frame_queue.len() as i64)),
                    ],
                );
                continue;
            }

            let present_t0 = Instant::now();
            let outcome = presenter.present_frame(&frame, config.sync_interval)?;
            let total_ms = present_t0.elapsed().as_secs_f64() * 1000.0;
            let interval_ms = last_present_wall
                .map(|last: Instant| {
                    present_t0.saturating_duration_since(last).as_secs_f64() * 1000.0
                })
                .unwrap_or(0.0);
            last_present_wall = Some(present_t0);
            stats.record(&outcome, late_ms, total_ms, interval_ms);
            log_event(
                "present",
                &[
                    ("pts", Value::from(frame.pts_secs)),
                    ("frame_elapsed", Value::from(frame_elapsed)),
                    ("late_ms", Value::from(late_ms)),
                    ("queue_len", Value::from(frame_queue.len() as i64)),
                    ("path", Value::from(outcome.path)),
                    ("wait_ms", Value::from(outcome.wait_ms)),
                    ("fence_wait_ms", Value::from(outcome.fence_wait_ms)),
                    ("copy_ms", Value::from(outcome.copy_ms)),
                    ("present_ms", Value::from(outcome.present_ms)),
                    ("total_ms", Value::from(total_ms)),
                    ("interval_ms", Value::from(interval_ms)),
                ],
            );
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    cancel.store(true, Ordering::Release);
    let _ = audio_drain.join();
    stats.emit_summary(config.duration);
    unsafe {
        let _ = DestroyWindow(window.hwnd);
    }
    Ok(())
}

struct ComApartment;

impl ComApartment {
    fn init() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| format!("CoInitializeEx: {e:?}"))?;
        }
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

struct NativeWindow {
    hwnd: HWND,
}

impl NativeWindow {
    fn create(width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            let hmodule = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e:?}"))?;
            let hinstance = HINSTANCE(hmodule.0);
            let cursor = LoadCursorW(None, IDC_ARROW).ok();
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance,
                hCursor: cursor.unwrap_or_default(),
                lpszClassName: w!("mIVDcompPresenterTest"),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
            let ex_style = WINDOW_EX_STYLE::default();
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            AdjustWindowRectEx(&mut rect, style, false, ex_style)
                .map_err(|e| format!("AdjustWindowRectEx: {e:?}"))?;
            let hwnd = CreateWindowExW(
                ex_style,
                w!("mIVDcompPresenterTest"),
                w!("mIV DirectComposition Presenter Test"),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(hinstance),
                None,
            )
            .map_err(|e| format!("CreateWindowExW: {e:?}"))?;
            let _ = ShowWindow(hwnd, SW_SHOW);
            Ok(Self { hwnd })
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let _ = lparam.0 as *const CREATESTRUCTW;
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_KEYDOWN if wparam.0 as u32 == 0x1B => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn pump_messages() -> bool {
    let mut quit = false;
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                quit = true;
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    quit
}

struct DcompPresenter {
    swap_chain: IDXGISwapChain1,
    waitable: HANDLE,
    d3d_device1: ID3D11Device1,
    d3d_device5: ID3D11Device5,
    d3d_context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    d3d_context4: ID3D11DeviceContext4,
    backbuffer: ID3D11Texture2D,
    fence_cache: Option<(u64, isize, ID3D11Fence)>,
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
            HMODULE::default(),
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
        "dcomp-presenter-test: presenter D3D11 device created (feature_level=0x{:X})",
        feature_level.0
    ));
    Ok((device, context))
}

impl DcompPresenter {
    fn new(hwnd: HWND, width: u32, height: u32, _gpu: &GpuVideoDevice) -> Result<Self, String> {
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
                .CreateTargetForHwnd(hwnd, true)
                .map_err(|e| format!("CreateTargetForHwnd: {e:?}"))?;
            let visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual: {e:?}"))?;
            visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent: {e:?}"))?;
            target
                .SetRoot(&visual)
                .map_err(|e| format!("IDCompositionTarget::SetRoot: {e:?}"))?;
            dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit: {e:?}"))?;

            let backbuffer: ID3D11Texture2D = swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("IDXGISwapChain::GetBuffer: {e:?}"))?;
            let mut backbuffer_view = None;
            d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut backbuffer_view))
                .map_err(|e| format!("CreateRenderTargetView: {e:?}"))?;
            let backbuffer_view: ID3D11RenderTargetView = backbuffer_view
                .ok_or_else(|| "CreateRenderTargetView returned null".to_string())?;
            d3d_context.ClearRenderTargetView(&backbuffer_view, &[0.0, 0.0, 0.0, 1.0]);
            swap_chain
                .Present(1, Default::default())
                .ok()
                .map_err(|e| format!("initial IDXGISwapChain::Present: {e:?}"))?;
            log_event(
                "init",
                &[
                    ("width", Value::from(width as i64)),
                    ("height", Value::from(height as i64)),
                    ("buffer_count", Value::from(3)),
                    ("latency", Value::from(1)),
                ],
            );
            Ok(Self {
                swap_chain,
                waitable,
                d3d_device1,
                d3d_device5,
                d3d_context,
                d3d_context4,
                backbuffer,
                fence_cache: None,
            })
        }
    }

    fn present_frame(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
    ) -> Result<PresentOutcome, String> {
        let wait_t0 = Instant::now();
        let wait_result = unsafe { WaitForSingleObject(self.waitable, 100) };
        let wait_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        let timed_out = wait_result == WAIT_TIMEOUT;

        let copy_t0 = Instant::now();
        let mut fence_wait_ms = 0.0;
        let path = match &frame.data {
            VideoFrameData::Cpu(bytes) => {
                unsafe {
                    self.d3d_context.UpdateSubresource(
                        &self.backbuffer,
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
                    return Err("10-bit D3D11 frame is not supported by this prototype".into());
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
                    let dst_res: ID3D11Resource = self
                        .backbuffer
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
        Ok(PresentOutcome {
            path,
            wait_ms,
            wait_timed_out: timed_out,
            fence_wait_ms,
            copy_ms,
            present_ms,
        })
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
}

impl Drop for DcompPresenter {
    fn drop(&mut self) {
        if !self.waitable.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.waitable);
            }
        }
    }
}

struct PresentOutcome {
    path: &'static str,
    wait_ms: f64,
    wait_timed_out: bool,
    fence_wait_ms: f64,
    copy_ms: f64,
    present_ms: f64,
}

#[derive(Default)]
struct PresentStats {
    presented: u64,
    gpu: u64,
    cpu: u64,
    late_drop: u64,
    wait_timeout: u64,
    max_late_ms: f64,
    max_total_ms: f64,
    max_interval_ms: f64,
}

impl PresentStats {
    fn record(&mut self, outcome: &PresentOutcome, late_ms: f64, total_ms: f64, interval_ms: f64) {
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

    fn emit_summary(&self, duration: Duration) {
        let actual_fps = if duration.as_secs_f64() > 0.0 {
            self.presented as f64 / duration.as_secs_f64()
        } else {
            0.0
        };
        log_event(
            "summary",
            &[
                ("presented", Value::from(self.presented as i64)),
                ("gpu_frames", Value::from(self.gpu as i64)),
                ("cpu_frames", Value::from(self.cpu as i64)),
                ("late_drop", Value::from(self.late_drop as i64)),
                ("wait_timeout", Value::from(self.wait_timeout as i64)),
                ("actual_fps", Value::from(actual_fps)),
                ("max_late_ms", Value::from(self.max_late_ms)),
                ("max_total_ms", Value::from(self.max_total_ms)),
                ("max_interval_ms", Value::from(self.max_interval_ms)),
            ],
        );
        crate::logger::log(format!(
            "dcomp-presenter-test summary: presented={} fps={:.1} gpu={} cpu={} late_drop={} max_late_ms={:.1} max_interval_ms={:.1}",
            self.presented,
            actual_fps,
            self.gpu,
            self.cpu,
            self.late_drop,
            self.max_late_ms,
            self.max_interval_ms
        ));
    }
}

fn log_event(kind: &str, fields: &[(&str, Value)]) {
    crate::perf::event("native_presenter", kind, None, 0, fields);
}
