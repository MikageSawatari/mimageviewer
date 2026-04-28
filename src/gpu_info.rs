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

/// プライマリ GPU の CUDA Compute Capability を `major * 10 + minor` 形式で返す。
/// 例: RTX 4090 → 89, RTX 3090 → 86, RTX 5090 → 120, A100 → 80, T4 → 75。
///
/// **本関数は TRT pack インストール前にも呼ばれる**ため、CUDA Toolkit / TRT pack の
/// DLL に依存できない。実装は以下の優先順位:
///
/// 1. `nvcuda.dll` (NVIDIA ドライバに同梱、CUDA Toolkit 不要) を `LoadLibrary` で
///    動的ロード → `cuInit` + `cuDeviceComputeCapability` を呼んで authoritative な
///    値を取る (信頼度: 最高)
/// 2. DXGI Description 文字列 (例: "NVIDIA GeForce RTX 4090") から既知世代を pattern
///    match で推定 (信頼度: 中、新世代カードや RTX A シリーズは未網羅)
/// 3. それでも判定できなければ `None` (= UI で「対応 GPU を検出できませんでした」と表示)
///
/// 戻り値が `Some(sm)` でも、TRT pack の対応 SM 範囲外なら installer 側でエラーになる。
pub fn query_primary_gpu_sm() -> Option<u32> {
    if !is_nvidia_gpu() {
        return None;
    }
    if let Some(sm) = query_sm_via_cuda_driver() {
        return Some(sm);
    }
    // フォールバック: Description 文字列から推測
    if let Some(desc) = query_primary_gpu_description() {
        if let Some(sm) = sm_from_description(&desc) {
            return Some(sm);
        }
    }
    None
}

/// `nvcuda.dll` を動的ロードして `cuDeviceComputeCapability(0)` を呼ぶ。
/// 失敗時 (DLL 不在、シンボル解決失敗、CUDA エラー等) は全て `None` を返す。
#[cfg(windows)]
fn query_sm_via_cuda_driver() -> Option<u32> {
    use std::ffi::CString;
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows::core::PCSTR;

    // CUDA Driver API のサブセット FFI バインディング。
    // 公式 cuda.h と同じ ABI。CUresult (i32) 戻り値、0 = CUDA_SUCCESS。
    type CuInitFn = unsafe extern "system" fn(flags: u32) -> i32;
    type CuDeviceComputeCapabilityFn =
        unsafe extern "system" fn(major: *mut i32, minor: *mut i32, dev: i32) -> i32;

    unsafe {
        let dll_name = CString::new("nvcuda.dll").ok()?;
        let module = match LoadLibraryA(PCSTR(dll_name.as_ptr() as *const u8)) {
            Ok(m) if !m.is_invalid() => m,
            _ => return None,
        };

        // FFI: const char* 名前で symbol 取得 → Option<unsafe extern "system" fn() -> isize>
        // を希望の関数型に transmute する。失敗時は FreeLibrary して None。
        let resolve = |name: &str| -> Option<*const ()> {
            let c = CString::new(name).ok()?;
            let proc = GetProcAddress(module, PCSTR(c.as_ptr() as *const u8))?;
            Some(proc as *const ())
        };

        let cu_init_ptr = match resolve("cuInit") {
            Some(p) => p,
            None => {
                let _ = FreeLibrary(module);
                return None;
            }
        };
        let cu_compute_cap_ptr = match resolve("cuDeviceComputeCapability") {
            Some(p) => p,
            None => {
                let _ = FreeLibrary(module);
                return None;
            }
        };
        let cu_init: CuInitFn = std::mem::transmute(cu_init_ptr);
        let cu_compute_cap: CuDeviceComputeCapabilityFn =
            std::mem::transmute(cu_compute_cap_ptr);

        // CUDA_SUCCESS = 0。cuInit は冪等、複数回呼んでも問題ない。
        if cu_init(0) != 0 {
            let _ = FreeLibrary(module);
            return None;
        }
        let mut major: i32 = 0;
        let mut minor: i32 = 0;
        let res = cu_compute_cap(&mut major as *mut i32, &mut minor as *mut i32, 0);
        let _ = FreeLibrary(module);
        if res != 0 || major < 0 || minor < 0 {
            return None;
        }
        Some((major as u32) * 10 + (minor as u32))
    }
}

#[cfg(not(windows))]
fn query_sm_via_cuda_driver() -> Option<u32> {
    None
}

/// DXGI Description 文字列 (例: "NVIDIA GeForce RTX 4090") から既知世代を判定する。
/// `nvcuda.dll` 経由が失敗したときの保険。
///
/// 表は consumer 主要モデルのみ網羅。RTX A 系・Tesla・Quadro RTX・新世代カードは
/// 未対応 (= None)。
fn sm_from_description(desc: &str) -> Option<u32> {
    let d = desc.to_ascii_uppercase();
    // Blackwell (sm120 = sm12.0 = consumer RTX 50)
    if d.contains("RTX 50") || d.contains("RTX 5090") || d.contains("RTX 5080") {
        return Some(120);
    }
    // Hopper / GB100 datacenter (sm90)。consumer 機ではほぼ来ない。
    if d.contains("H100") || d.contains("H200") || d.contains("GH200") {
        return Some(90);
    }
    // Ada Lovelace (sm89 = consumer RTX 40)
    if d.contains("RTX 40")
        || d.contains("RTX 4090")
        || d.contains("RTX 4080")
        || d.contains("RTX 4070")
        || d.contains("RTX 4060")
        || d.contains("L40")
    {
        return Some(89);
    }
    // Ampere consumer (sm86 = RTX 30 series)
    if d.contains("RTX 30")
        || d.contains("RTX 3090")
        || d.contains("RTX 3080")
        || d.contains("RTX 3070")
        || d.contains("RTX 3060")
        || d.contains("RTX 3050")
    {
        return Some(86);
    }
    // Ampere datacenter (sm80 = A100, A30 等)
    if d.contains("A100") || d.contains("A30") || d.contains("A40") {
        return Some(80);
    }
    // Turing (sm75 = RTX 20 series, GTX 16 series, T4)
    if d.contains("RTX 20") || d.contains("RTX 2080") || d.contains("RTX 2070")
        || d.contains("RTX 2060") || d.contains("GTX 16")
        || d.contains("GTX 1660") || d.contains("GTX 1650")
        || d.contains("TESLA T4") || d.contains(" T4 ")
    {
        return Some(75);
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sm_from_description_known_models() {
        assert_eq!(sm_from_description("NVIDIA GeForce RTX 4090"), Some(89));
        assert_eq!(sm_from_description("NVIDIA GeForce RTX 4080 Ti"), Some(89));
        assert_eq!(sm_from_description("NVIDIA GeForce RTX 3090"), Some(86));
        assert_eq!(sm_from_description("NVIDIA GeForce RTX 3060 Laptop"), Some(86));
        assert_eq!(sm_from_description("NVIDIA GeForce RTX 5090"), Some(120));
        assert_eq!(sm_from_description("NVIDIA A100-SXM4-80GB"), Some(80));
        assert_eq!(sm_from_description("NVIDIA GeForce RTX 2080 Super"), Some(75));
        assert_eq!(sm_from_description("NVIDIA GeForce GTX 1660 Ti"), Some(75));
        assert_eq!(sm_from_description("NVIDIA H100 80GB HBM3"), Some(90));
    }

    #[test]
    fn sm_from_description_unknown_returns_none() {
        // Pascal (sm61) は対応外として None
        assert_eq!(sm_from_description("NVIDIA GeForce GTX 1080 Ti"), None);
        // Maxwell (sm52) も対応外
        assert_eq!(sm_from_description("NVIDIA GeForce GTX 980"), None);
        // 非 NVIDIA 文字列
        assert_eq!(sm_from_description("Intel UHD Graphics 630"), None);
    }
}
