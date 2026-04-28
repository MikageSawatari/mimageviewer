//! GPU (DXGI) 情報の取得ヘルパー。
//!
//! 主目的はプライマリ GPU の VRAM 容量を取得し、
//! サムネイル VRAM 上限を % 指定で計算できるようにすること (段階 D)。

/// GPU ベンダー識別。TensorRT 設定の gating に使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other(u32),
}

impl GpuVendor {
    /// PCI Vendor ID から判定する。
    /// 0x10DE = NVIDIA, 0x1002 = AMD, 0x8086 = Intel
    pub fn from_pci_id(vendor_id: u32) -> GpuVendor {
        match vendor_id {
            0x10DE => GpuVendor::Nvidia,
            0x1002 => GpuVendor::Amd,
            0x8086 => GpuVendor::Intel,
            other => GpuVendor::Other(other),
        }
    }
}

/// DXGI 列挙の戻り値。ベンダー / VRAM / GPU 名 (Description) をまとめて返す。
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub vendor_id: u32,
    pub vram_bytes: u64,
    /// DXGI Description (例: "NVIDIA GeForce RTX 4090")。
    /// `query_primary_gpu_sm` での SM 推定に使う。
    pub description: String,
}

/// DXGI でプライマリ adapter (ソフトウェアアダプタ以外) を列挙する。
///
/// VRAM 取得 / vendor 判定 / GPU 名取得で同じ列挙ロジックを共有する。
#[cfg(windows)]
fn enumerate_primary_adapter() -> Option<AdapterInfo> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1,
    };

    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for i in 0u32..8 {
            let adapter: IDXGIAdapter1 = match factory.EnumAdapters1(i) {
                Ok(a) => a,
                Err(_) => break,
            };
            let desc = match adapter.GetDesc1() {
                Ok(d) => d,
                Err(_) => continue,
            };
            // ソフトウェアアダプタ (WARP 等) はスキップ
            let flags = desc.Flags as i32;
            if (flags & DXGI_ADAPTER_FLAG_SOFTWARE.0) != 0 {
                continue;
            }
            if desc.DedicatedVideoMemory > 0 {
                let description = String::from_utf16_lossy(
                    &desc.Description[..desc
                        .Description
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(desc.Description.len())],
                );
                return Some(AdapterInfo {
                    vendor_id: desc.VendorId,
                    vram_bytes: desc.DedicatedVideoMemory as u64,
                    description,
                });
            }
        }
        None
    }
}

#[cfg(not(windows))]
fn enumerate_primary_adapter() -> Option<AdapterInfo> {
    None
}

/// プライマリ GPU の専用 VRAM 容量 (bytes) を返す。
///
/// DXGI でアダプタを列挙し、ソフトウェアアダプタをスキップして
/// 最初に見つかった `DedicatedVideoMemory > 0` のアダプタを使う。
///
/// 取得失敗時は `None`。呼び出し側は妥当なフォールバック (例: 4 GiB) を使うこと。
pub fn query_primary_gpu_vram_bytes() -> Option<u64> {
    enumerate_primary_adapter().map(|info| info.vram_bytes)
}

/// プライマリ GPU のベンダーを返す。TensorRT 設定の gating に使用。
pub fn query_primary_gpu_vendor() -> Option<GpuVendor> {
    enumerate_primary_adapter().map(|info| GpuVendor::from_pci_id(info.vendor_id))
}

/// プライマリ GPU の Description (例: "NVIDIA GeForce RTX 4090") を返す。
/// AMPERE_PLUS mode 判定 / SM 推定 / UI 表示に使う。
#[allow(dead_code)]
pub fn query_primary_gpu_description() -> Option<String> {
    enumerate_primary_adapter().map(|info| info.description)
}

/// プライマリ GPU が NVIDIA かどうか。TensorRT 機能の有効化判定。
pub fn is_nvidia_gpu() -> bool {
    matches!(query_primary_gpu_vendor(), Some(GpuVendor::Nvidia))
}

/// VRAM 容量に対する % 指定から実バイト数を算出する。
///
/// VRAM の取得失敗時は 4 GiB を仮定する保守的フォールバックを使う。
pub fn vram_cap_from_percent(percent: u32) -> u64 {
    const FALLBACK_VRAM_BYTES: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB
    let total = query_primary_gpu_vram_bytes().unwrap_or(FALLBACK_VRAM_BYTES);
    total.saturating_mul(percent as u64) / 100
}

/// VRAM 容量を取得し、表示用に (総 MiB, 使用可能 MiB) を返す。
/// 失敗時は `None`。
pub fn query_vram_summary_mib() -> Option<u64> {
    query_primary_gpu_vram_bytes().map(|b| b / (1024 * 1024))
}
