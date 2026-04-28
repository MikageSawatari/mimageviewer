//! D3D11 デバイス + ビデオプロセッサ + 共有 RGBA 出力テクスチャ管理。
//!
//! このモジュールは D3D11 単独で完結し、wgpu / egui には依存しない。FFmpeg の
//! `hw_device_ctx` に渡せる `ID3D11Device` と、NV12 → RGBA 変換を実行する
//! `ID3D11VideoProcessor` を管理する。NVIDIA RTX VSR の opt-in もここで行う。
//!
//! ## 共有テクスチャの作り方
//! wgpu (d3d12) と D3D11 の間でフレームを受け渡すには **NT shared handle** を使う。
//! D3D11 側で `D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`
//! でテクスチャを作り、`IDXGIResource1::CreateSharedHandle` で HANDLE を取り、
//! それを `ID3D12Device::OpenSharedHandle` で D3D12 側で開く。Keyed Mutex は
//! 書き込み (D3D11) と読み取り (D3D12) の同期に使う。
//!
//! ## エラー方針
//! D3D11 / VideoDevice の作成は **失敗してもパニックしない**。失敗時は呼び出し側に
//! `Err` を返し、呼び出し側は SW (CPU readback + swscale) 経路にフォールバックする。

use std::ptr;
use std::sync::Arc;

use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_RATIONAL,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIResource1;
use windows::core::Interface;

use super::vsr::{self, VsrCapability};

/// 出力テクスチャのリングバッファサイズ。decoder が書き → UI が読む間
/// に少なくとも 2 枚 in-flight を許す。
#[allow(dead_code)]
const OUTPUT_RING_SIZE: usize = 3;

/// 1 フレームの GPU 出力。`KeyedMutex` で wgpu 側との読み書き排他。
pub struct D3d11Frame {
    pub width: u32,
    pub height: u32,
    /// pts (秒)。decoder が書く。
    pub pts_secs: f64,
    /// シーク世代。
    pub seek_serial: u64,
    /// NT shared handle. wgpu 側で `OpenSharedHandle` する。
    /// `Drop` で `CloseHandle` する責任は所有者にある。
    pub shared_handle: HANDLE,
    /// keyed mutex の key。書き込み完了時 1、読み取り完了時 0 を release。
    pub keyed_mutex_key: u64,
}

#[derive(Debug)]
pub enum GpuVideoError {
    DeviceCreate(String),
    NoVideoDevice,
    EnumeratorCreate(String),
    ProcessorCreate(String),
    TextureCreate(String),
    SharedHandle(String),
    Blt(String),
}

impl std::fmt::Display for GpuVideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceCreate(s) => write!(f, "D3D11CreateDevice failed: {s}"),
            Self::NoVideoDevice => write!(f, "ID3D11VideoDevice not supported on this adapter"),
            Self::EnumeratorCreate(s) => write!(f, "CreateVideoProcessorEnumerator failed: {s}"),
            Self::ProcessorCreate(s) => write!(f, "CreateVideoProcessor failed: {s}"),
            Self::TextureCreate(s) => write!(f, "CreateTexture2D failed: {s}"),
            Self::SharedHandle(s) => write!(f, "CreateSharedHandle failed: {s}"),
            Self::Blt(s) => write!(f, "VideoProcessorBlt failed: {s}"),
        }
    }
}

impl std::error::Error for GpuVideoError {}

/// HW デコード + GPU NV12→RGBA 変換用の共有 D3D11 デバイス。
///
/// FFmpeg の `AVCodecContext.hw_device_ctx` にこのデバイスを渡すと、HW デコーダの
/// 出力 NV12 テクスチャが本デバイス上に生成され、`VideoProcessorBlt` で同じデバイス
/// 内のリング出力テクスチャに RGBA 変換できる (= GPU→CPU 転送なし)。
#[allow(dead_code)]
pub struct GpuVideoDevice {
    device: ID3D11Device,
    /// SAFETY: ID3D11DeviceContext は **Send/Sync 安全ではない**。本構造体は全体を
    /// `Mutex` で囲む前提で外から使い、内部では context を直接共有する。
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    /// processor + enumerator のキャッシュ。同一 (in_size, out_size, format) なら使い回す。
    processor_cache: std::cell::RefCell<Option<ProcessorState>>,
    /// VSR を opt-in するか (= 設定の `video_rtx_vsr`)。`VideoProcessorBlt` 直前に
    /// `SetStreamExtension` で NVIDIA 拡張 GUID を流す。RTX 非搭載 / コンパネ OFF の
    /// 場合は no-op (ドライバが拡張を無視する)。
    vsr_enabled: bool,
    vsr_capability: VsrCapability,
}

struct ProcessorState {
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    in_w: u32,
    in_h: u32,
    in_format: DXGI_FORMAT,
    out_format: DXGI_FORMAT,
}

impl GpuVideoDevice {
    /// D3D11 デバイス + ビデオデバイスを作成する。失敗時はフォールバックを呼び出し側で
    /// 処理する想定。
    pub fn new(vsr_enabled: bool) -> Result<Arc<Self>, GpuVideoError> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL::default();
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];

        // BGRA_SUPPORT: wgpu 側で BGRA8UnormSrgb を使うため
        // VIDEO_SUPPORT: ID3D11VideoDevice / ID3D11VideoContext の取得に必要
        let flags = D3D11_CREATE_DEVICE_FLAG(
            D3D11_CREATE_DEVICE_BGRA_SUPPORT.0 | D3D11_CREATE_DEVICE_VIDEO_SUPPORT.0,
        );

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
            .map_err(|e| GpuVideoError::DeviceCreate(format!("{e:?}")))?;
        }
        let device = device.ok_or_else(|| {
            GpuVideoError::DeviceCreate("D3D11CreateDevice returned null device".into())
        })?;
        let context = context.ok_or_else(|| {
            GpuVideoError::DeviceCreate("D3D11CreateDevice returned null context".into())
        })?;

        let video_device: ID3D11VideoDevice = device
            .cast()
            .map_err(|_| GpuVideoError::NoVideoDevice)?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(|_| GpuVideoError::NoVideoDevice)?;

        let vsr_capability = vsr::detect_vsr_capability(&device);
        crate::logger::log(format!(
            "GpuVideoDevice: created (vsr_enabled={vsr_enabled}, capability={vsr_capability:?})"
        ));

        Ok(Arc::new(Self {
            device,
            context,
            video_device,
            video_context,
            processor_cache: std::cell::RefCell::new(None),
            vsr_enabled,
            vsr_capability,
        }))
    }

    pub fn raw_device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn vsr_capability(&self) -> VsrCapability {
        self.vsr_capability
    }

    pub fn set_vsr_enabled(&mut self, enabled: bool) {
        self.vsr_enabled = enabled;
    }

    /// 入力 NV12 / P010 テクスチャを RGBA 共有テクスチャに blit する。
    /// 出力テクスチャは新規作成 (リング管理は呼び出し側)。
    ///
    /// SAFETY: `input_texture` / `subresource` が valid であること。FFmpeg の AVFrame
    /// から渡す前提。
    pub unsafe fn blit_nv12_to_rgba(
        &self,
        input_texture: &ID3D11Texture2D,
        subresource: u32,
        out_w: u32,
        out_h: u32,
        ten_bit: bool,
    ) -> Result<(ID3D11Texture2D, HANDLE), GpuVideoError> {
        // 1. 入力テクスチャの属性を読む
        let in_desc = unsafe {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            input_texture.GetDesc(&mut d);
            d
        };
        let in_w = in_desc.Width;
        let in_h = in_desc.Height;
        let in_format = in_desc.Format;
        let out_format = if ten_bit {
            DXGI_FORMAT_R10G10B10A2_UNORM
        } else {
            DXGI_FORMAT_B8G8R8A8_UNORM
        };

        // 2. processor / enumerator を確保 (キャッシュ)
        self.ensure_processor(in_w, in_h, in_format, out_w, out_h, out_format)?;
        let cache = self.processor_cache.borrow();
        let state = cache.as_ref().expect("ensured above");

        // 3. 出力 RGBA テクスチャ (NT shared) を作成
        let (out_tex, shared_handle) = self.create_shared_output(out_w, out_h, out_format)?;

        // 4. 入力 view を作成
        let mut in_view: Option<ID3D11VideoProcessorInputView> = None;
        let in_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: windows::Win32::Graphics::Direct3D11::D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: subresource,
                },
            },
        };
        unsafe {
            self.video_device
                .CreateVideoProcessorInputView(
                    input_texture,
                    &state.enumerator,
                    &in_view_desc,
                    Some(&mut in_view),
                )
                .map_err(|e| GpuVideoError::Blt(format!("CreateInputView: {e:?}")))?;
        }
        let in_view = in_view.ok_or_else(|| GpuVideoError::Blt("InputView null".into()))?;

        // 5. 出力 view を作成
        let mut out_view: Option<ID3D11VideoProcessorOutputView> = None;
        let out_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: windows::Win32::Graphics::Direct3D11::D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        unsafe {
            self.video_device
                .CreateVideoProcessorOutputView(
                    &out_tex,
                    &state.enumerator,
                    &out_view_desc,
                    Some(&mut out_view),
                )
                .map_err(|e| GpuVideoError::Blt(format!("CreateOutputView: {e:?}")))?;
        }
        let out_view = out_view.ok_or_else(|| GpuVideoError::Blt("OutputView null".into()))?;

        // 6. VSR opt-in (有効化されており、ドライバが対応する場合のみ)
        if self.vsr_enabled && self.vsr_capability.is_available() {
            unsafe {
                vsr::apply_nvidia_vsr_extension(&self.video_context, &state.processor);
            }
        }

        // 7. ストリームを構成して blit
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(in_view.clone())),
            ppFutureSurfaces: ptr::null_mut(),
            ppPastSurfacesRight: ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: ptr::null_mut(),
        };
        unsafe {
            self.video_context
                .VideoProcessorBlt(&state.processor, &out_view, 0, &[stream])
                .map_err(|e| GpuVideoError::Blt(format!("Blt: {e:?}")))?;
        }

        Ok((out_tex, shared_handle))
    }

    fn ensure_processor(
        &self,
        in_w: u32,
        in_h: u32,
        in_format: DXGI_FORMAT,
        out_w: u32,
        out_h: u32,
        out_format: DXGI_FORMAT,
    ) -> Result<(), GpuVideoError> {
        let cur = self.processor_cache.borrow();
        if let Some(s) = cur.as_ref() {
            if s.in_w == in_w
                && s.in_h == in_h
                && s.in_format == in_format
                && s.out_format == out_format
            {
                return Ok(());
            }
        }
        drop(cur);

        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: out_w,
            OutputHeight: out_h,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe {
            self.video_device
                .CreateVideoProcessorEnumerator(&content_desc)
                .map_err(|e| GpuVideoError::EnumeratorCreate(format!("{e:?}")))?
        };
        let processor = unsafe {
            self.video_device
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(|e| GpuVideoError::ProcessorCreate(format!("{e:?}")))?
        };
        *self.processor_cache.borrow_mut() = Some(ProcessorState {
            enumerator,
            processor,
            in_w,
            in_h,
            in_format,
            out_format,
        });
        Ok(())
    }

    fn create_shared_output(
        &self,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
    ) -> Result<(ID3D11Texture2D, HANDLE), GpuVideoError> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
                | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .map_err(|e| GpuVideoError::TextureCreate(format!("{e:?}")))?;
        }
        let tex = tex.ok_or_else(|| GpuVideoError::TextureCreate("null texture".into()))?;
        let dxgi: IDXGIResource1 = tex
            .cast()
            .map_err(|e| GpuVideoError::SharedHandle(format!("cast IDXGIResource1: {e:?}")))?;
        let handle = unsafe {
            dxgi.CreateSharedHandle(
                None,
                windows::Win32::Foundation::GENERIC_ALL.0,
                None,
            )
            .map_err(|e| GpuVideoError::SharedHandle(format!("{e:?}")))?
        };
        Ok((tex, handle))
    }
}

// ID3D11Device / ID3D11DeviceContext は COM オブジェクトで、`windows` crate の
// `Interface` 実装によって `Send + Sync` (内部 refcount は thread-safe な atomic)。
// ただし ID3D11DeviceContext の **同時呼び出し** は未定義動作 → 利用側で Mutex 化すること。
unsafe impl Send for GpuVideoDevice {}
unsafe impl Sync for GpuVideoDevice {}
