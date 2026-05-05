//! D3D11 NT shared handle を wgpu (D3D12) 側に import する。
//!
//! ## 経路
//! ```text
//! D3D11 ID3D11Texture2D (D3D11_RESOURCE_MISC_SHARED_NTHANDLE) ─┐
//!  └─ IDXGIResource1::CreateSharedHandle                       │
//!                                                              │ HANDLE (NT)
//!                                                              ▼
//!  ID3D12Device::OpenSharedHandle ──► ID3D12Resource           │
//!  (wgpu の wgpu_hal::dx12::Device 経由で得る)                 │
//!                                                              ▼
//!  wgpu_hal::dx12::Device::texture_from_raw ──► hal::dx12::Texture
//!                                                              ▼
//!  wgpu::Device::create_texture_from_hal::<Dx12> ──► wgpu::Texture
//! ```
//!
//! ## 同期
//! 共有テクスチャは `D3D11_RESOURCE_MISC_SHARED_NTHANDLE | KEYEDMUTEX` で作っているが、
//! D3D12 側は `ID3D12Resource` から `IDXGIKeyedMutex` を直接取得できないため
//! (ID3D12Resource は IDXGIKeyedMutex を実装しない)、書き込み完了の待ち合わせは
//! **`ID3D11Fence` + `D3D11_FENCE_FLAG_SHARED` ↔ `ID3D12Fence`** で行う。
//!
//! - D3D11 側 (decoder thread): VPP blit + CopyResource 完了後に
//!   `ID3D11DeviceContext4::Signal(fence, value)` で fence を進める
//! - D3D12 側 (UI thread): `ID3D12CommandQueue::Wait(fence, value)` を queue に積んでから
//!   テクスチャを sample。GPU レベルで CopyResource 完了が保証される
//!
//! KEYEDMUTEX flag は CreateTexture2D を通すための形式上の指定 (NVIDIA driver 仕様で
//! `NTHANDLE` 単独は E_INVALIDARG) で、AcquireSync/ReleaseSync 自体は decoder thread
//! 側で 0→1 を 1 回回すだけ。実 sync は fence が担っている。

use windows::Win32::Foundation::HANDLE as WinHandle061;
// wgpu-hal 27 が要求する windows 0.58 系の ID3D12Resource。本体の windows 0.61
// とは別の型なので、`windows_058` 別名で読み込んで OpenSharedHandle を呼ぶ。
use windows_058::Win32::Foundation::HANDLE as WinHandle058;
use windows_058::Win32::Graphics::Direct3D12::ID3D12Resource;

use super::d3d11_device::GpuVideoError;

/// import 結果。`wgpu::Texture` は `Drop` で解放されるが、内部の D3D12 リソースは
/// 共有 handle 経由で開いたものなので、元の D3D11 テクスチャの寿命は別途管理する
/// (= D3D11 側の `ID3D11Texture2D` を Drop しない限り D3D12 側も生き続ける、という
/// dx12 ドライバの保証に依存)。
#[allow(dead_code)]
pub struct ImportedTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    /// `texture` を作るのに使った NT handle。所有者は呼び出し側で、import 後に
    /// `CloseHandle` してもよい (D3D12 側は OpenSharedHandle 時点で内部参照を増やす)。
    pub handle: WinHandle061,
}

/// NT shared handle を開いて wgpu::Texture を作る。
///
/// `format` は D3D11 側で出力した format に対応する wgpu フォーマット。
/// 現在の decoder 出力は HDR 表示非対応のため BGRA8 に正規化される。
/// `ten_bit=true` の import は古い/防御的な経路として残している。
///
/// SAFETY: handle が valid であり、対応する D3D11 テクスチャが KEYEDMUTEX 付きで
/// 作られていること。寸法 / format が D3D11 側と一致していること。
#[allow(dead_code)]
pub unsafe fn import_shared_d3d11_texture(
    wgpu_device: &wgpu::Device,
    handle: WinHandle061,
    width: u32,
    height: u32,
    ten_bit: bool,
) -> Result<ImportedTexture, GpuVideoError> {
    // **重要**: D3D11 video processor の出力は典型的な SDR 動画の場合 BT.709 (≒ sRGB)
    // ガンマで gamma-corrected な画素値が入っている。これを `Bgra8UnormSrgb` で
    // sample すると egui のシェーダがさらに sRGB→linear デコードを掛けてしまう
    // (= 二重ガンマ補正)。`Bgra8Unorm` (linear interpretation) で受け取り、
    // 表示 pipeline に「画素は既に display-ready」として扱わせる方が正しい。
    let wgpu_format = if ten_bit {
        wgpu::TextureFormat::Rgb10a2Unorm
    } else {
        wgpu::TextureFormat::Bgra8Unorm
    };

    // windows 0.61 と 0.58 の HANDLE は **size_of/align_of 同一** で、内部は
    // どちらも単一の isize/ptr フィールドの transparent newtype。値を取り出して
    // 0.58 側の HANDLE に詰め直して使う (assertion で size/align 一致を強制)。
    const _: () = assert!(
        std::mem::size_of::<WinHandle061>() == std::mem::size_of::<WinHandle058>(),
        "HANDLE size mismatch between windows 0.61 and 0.58"
    );
    const _: () = assert!(
        std::mem::align_of::<WinHandle061>() == std::mem::align_of::<WinHandle058>(),
        "HANDLE alignment mismatch between windows 0.61 and 0.58"
    );
    let handle_058 = WinHandle058(handle.0);

    // D3D12 から OpenSharedHandle で ID3D12Resource を得る。
    // wgpu_hal::dx12::Device::raw_device() で内部 ID3D12Device を借りる。
    let resource: ID3D12Resource = unsafe {
        let hal_dev_opt = wgpu_device.as_hal::<wgpu_hal::api::Dx12>();
        let hal_dev = hal_dev_opt
            .ok_or_else(|| GpuVideoError::SharedHandle("wgpu device is not dx12".into()))?;
        let d3d12 = hal_dev.raw_device();
        let mut res: Option<ID3D12Resource> = None;
        d3d12
            .OpenSharedHandle(handle_058, &mut res)
            .map_err(|e| GpuVideoError::SharedHandle(format!("OpenSharedHandle: {e:?}")))?;
        res.ok_or_else(|| GpuVideoError::SharedHandle("OpenSharedHandle returned null".into()))?
    };

    // ID3D12Resource を hal::dx12::Texture に包む。
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let hal_tex = unsafe {
        wgpu_hal::dx12::Device::texture_from_raw(
            resource,
            wgpu_format,
            wgpu::TextureDimension::D2,
            extent,
            1, // mip_level_count
            1, // sample_count
        )
    };

    // wgpu::Texture に昇格。
    let desc = wgpu::TextureDescriptor {
        label: Some("video-shared-rgba"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu_format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    let texture =
        unsafe { wgpu_device.create_texture_from_hal::<wgpu_hal::api::Dx12>(hal_tex, &desc) };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    Ok(ImportedTexture {
        texture,
        view,
        width,
        height,
        handle,
    })
}
