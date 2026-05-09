//! 動画フレームを GPU 上で NV12 → RGBA 変換し、NT-shared D3D11 テクスチャとして
//! native presenter (`crate::video::native_presenter`) に渡すパス。
//!
//! ## 経路
//! ```text
//! FFmpeg HW decoder (D3D11VA) ─► AVFrame.format == AV_PIX_FMT_D3D11
//!     ├─ data[0] = ID3D11Texture2D*
//!     └─ data[1] = subresource index (intptr)
//!         │
//!         ▼
//!  ID3D11VideoProcessor (BT.601/709 → RGBA, bicubic)
//!         │ output: NT-shared ID3D11Texture2D (RGBA8 / RGBA16F) + 共有 fence
//!         ▼
//!  decoder thread → UI thread (channel 経由)
//!         ▼
//!  NativeVideoPresenter が `OpenSharedHandle` で取得 → 自身の swap chain backbuffer に copy
//! ```
//!
//! HW デコードできない場合 (= GpuVideoDevice 未作成 / D3D11VA 失敗) は
//! `decoder.rs` 内で SW デコード + swscale を走らせ、`VideoFrameData::Cpu` 形式で
//! native presenter に渡される (= native presenter 側で `UpdateSubresource` で
//! バックバッファに upload)。
//!
//! ## なぜ複雑なことをするか
//! 4K HEVC 動画を 30fps でカクつかず再生するには、1 フレームあたりの
//! decode + transfer + 変換 + GPU upload が 33ms 以内に収まる必要がある。
//! HW デコード自体は 1-2ms / frame だが、`av_hwframe_transfer_data` (GPU→CPU)
//! と CPU 上の `swscale` (NV12→RGBA) と CPU→GPU upload で合計 36-50ms 食って
//! 予算を超過する (perf log 実測)。CPU readback を完全に省くには D3D11 video
//! processor で直接 NT 共有テクスチャに書き込み、native presenter の D3D11 device で
//! `OpenSharedHandle` するのが最速。
//!
//! ## ライフタイム
//! - `GpuVideoDevice` 1 個 / アプリ寿命。Drop で全 D3D11 リソース解放。
//! - 出力テクスチャは毎フレーム新規作成、`D3d11Frame` が NT shared HANDLE の close を
//!   `Drop` で行う (= UI が描画中の HANDLE が close される race を防ぐ)。
//! - decoder thread の書き込み完了 → native presenter の sample の同期は
//!   **D3D11 共有 fence** で行う (`ID3D11Fence::Signal` → 受信側の
//!   `ID3D11DeviceContext4::Wait`)。

#![cfg(windows)]

mod d3d11_device;
mod ffmpeg_d3d11;

pub use d3d11_device::{D3d11Frame, GpuVideoDevice, GpuVideoError, VideoColorHint};
#[allow(unused_imports)]
pub use ffmpeg_d3d11::create_ffmpeg_hw_device_ctx;
