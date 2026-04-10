//! GPU (DXGI) 情報の取得ヘルパー。
//!
//! 主目的はプライマリ GPU の VRAM 容量を取得し、
//! サムネイル VRAM 上限を % 指定で計算できるようにすること (段階 D)。

/// プライマリ GPU の専用 VRAM 容量 (bytes) を返す。
///
/// DXGI でアダプタを列挙し、ソフトウェアアダプタをスキップして
/// 最初に見つかった `DedicatedVideoMemory > 0` のアダプタを使う。
///
/// 取得失敗時は `None`。呼び出し側は妥当なフォールバック (例: 4 GiB) を使うこと。
pub fn query_primary_gpu_vram_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
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
                    return Some(desc.DedicatedVideoMemory as u64);
                }
            }
            None
        }
    }

    #[cfg(not(windows))]
    {
        None
    }
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
