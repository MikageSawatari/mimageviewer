//! 動画フレームを GPU 上で NV12 → RGBA 変換し、egui_wgpu 経由で表示するパス。
//!
//! ## 経路
//! ```text
//! FFmpeg HW decoder (D3D11VA) ─► AVFrame.format == AV_PIX_FMT_D3D11
//!     ├─ data[0] = ID3D11Texture2D*
//!     └─ data[1] = subresource index (intptr)
//!         │
//!         ▼
//!  ID3D11VideoProcessor (BT.601/709 → RGBA, bicubic)
//!         │ output: NT-shared ID3D11Texture2D (RGBA8 / RGBA16F)
//!         ▼
//!  IDXGIResource1::CreateSharedHandle (NT handle)
//!         ▼
//!  ID3D12Device::OpenSharedHandle ─► ID3D12Resource
//!         ▼
//!  wgpu_hal::dx12::Device::texture_from_raw ─► wgpu::Texture
//!         ▼
//!  egui_wgpu::CallbackTrait で fullscreen quad に貼って表示
//! ```
//!
//! CPU フォールバック経路 (DX12 backend が取れない / GpuVideoDevice 作成失敗時):
//! ```text
//! AVFrame[D3D11] ─► av_hwframe_transfer_data (GPU→CPU、12.5MB/frame@4K)
//!     ─► swscale CPU NV12→RGBA (24MB/frame、~36-50ms@4K)
//!     ─► ctx.load_texture (CPU→GPU、24MB)
//!     ─► egui::Image
//! ```
//!
//! ## なぜ複雑なことをするか
//! 4K HEVC 動画を 30fps でカクつかず再生するには、1 フレームあたりの
//! decode + transfer + 変換 + GPU upload が 33ms 以内に収まる必要がある。
//! HW デコード自体は 1-2ms / frame だが、`av_hwframe_transfer_data` (GPU→CPU)
//! と CPU 上の `swscale` (NV12→RGBA) と `ctx.load_texture` (CPU→GPU) で
//! 合計 36-50ms 食って予算を超過する (perf log 実測)。CPU readback を
//! 完全に省くには D3D11 ↔ wgpu (d3d12) のテクスチャ共有が必要。
//!
//! ## ライフタイム
//! - `GpuVideoDevice` 1 個 / アプリ寿命。Drop で全 D3D11 リソース解放。
//! - 出力テクスチャは毎フレーム新規作成、`D3d11Frame` が NT shared HANDLE の close を
//!   `Drop` で行う (= UI が描画中の HANDLE が close される race を防ぐ)。
//! - decoder thread の書き込み完了 → UI thread の sample の同期は **D3D11/D3D12 共有 fence**
//!   で行う (`ID3D11Fence::Signal` → `ID3D12CommandQueue::Wait`)。

#![cfg(windows)]

mod d3d11_device;
mod ffmpeg_d3d11;
mod video_paint;
mod wgpu_import;

pub use d3d11_device::{D3d11Frame, GpuVideoDevice, GpuVideoError, VideoColorHint};
#[allow(unused_imports)]
pub use ffmpeg_d3d11::create_ffmpeg_hw_device_ctx;
pub use video_paint::{VideoPaintCallback, VideoPipeline, init_video_pipeline};
#[allow(unused_imports)]
pub use wgpu_import::{ImportedTexture, import_shared_d3d11_texture};
