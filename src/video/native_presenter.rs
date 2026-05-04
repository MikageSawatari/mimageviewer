use std::time::Instant;

use serde_json::Value;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, WAIT_TIMEOUT};
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
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::Interface;

use crate::video::decoder::{VideoFrame, VideoFrameData};

pub struct NativePresenterConfig {
    pub hwnd: HWND,
    pub width: u32,
    pub height: u32,
}

pub struct NativeVideoPresenter {
    swap_chain: IDXGISwapChain1,
    waitable: HANDLE,
    d3d_device1: ID3D11Device1,
    d3d_device5: ID3D11Device5,
    d3d_context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    d3d_context4: ID3D11DeviceContext4,
    backbuffer: ID3D11Texture2D,
    fence_cache: Option<(u64, isize, ID3D11Fence)>,
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
                    ("width", Value::from(config.width as i64)),
                    ("height", Value::from(config.height as i64)),
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
        Ok(NativePresentOutcome {
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
