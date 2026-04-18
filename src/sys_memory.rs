//! 物理 RAM 容量の取得ヘルパー (v0.7.0)。
//!
//! 現時点の用途: ネスト ZIP バイト列キャッシュ (`zip_loader::NESTED_CACHE`) の
//! 容量上限を「搭載 RAM の 25%, ただし 4GB 頭打ち」で決定する。

/// 物理 RAM 総量 (bytes) を返す。取得失敗時は 8GB を仮定する。
pub fn total_physical_ram_bytes() -> u64 {
    query_total_physical_ram_bytes().unwrap_or(8 * 1024 * 1024 * 1024)
}

#[cfg(windows)]
fn query_total_physical_ram_bytes() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: MEMORYSTATUSEX は上で正しく初期化されており、ポインタは有効。
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok.is_ok() {
        Some(status.ullTotalPhys)
    } else {
        None
    }
}

#[cfg(not(windows))]
fn query_total_physical_ram_bytes() -> Option<u64> {
    None
}

/// ネスト ZIP バイト列キャッシュの推奨上限を返す。
///
/// - 物理 RAM の 25%
/// - ただし 4 GiB で頭打ち (`MAX_CAP_BYTES`)
/// - 下限 256 MiB (`MIN_CAP_BYTES`): 最低限ネスト 1 章程度は確実に入る
pub fn nested_zip_cache_budget() -> usize {
    const MAX_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
    const MIN_CAP_BYTES: u64 = 256 * 1024 * 1024;
    let total = total_physical_ram_bytes();
    let quarter = total / 4;
    let capped = quarter.min(MAX_CAP_BYTES).max(MIN_CAP_BYTES);
    capped as usize
}
