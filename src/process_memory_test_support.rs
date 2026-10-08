//! テストが使う自プロセスのメモリ使用量サンプラ (Windows 専用)。
//!
//! `windows` クレートは `Win32_System_ProcessStatus` feature を有効にしていないので
//! `GetProcessMemoryInfo` は手書きの extern 宣言で呼ぶ。同じシンボルを複数のモジュールが
//! 別々の構造体ポインタで宣言すると `clashing_extern_declarations` が lib test の
//! ビルドごとに出るため、宣言はこの 1 箇所に集約する。
//!
//! 構造体は `PROCESS_MEMORY_COUNTERS_EX` (`private_usage` 付き) 側に揃える。
//! `PROCESS_MEMORY_COUNTERS` は先頭 10 フィールドが同一レイアウトで、`cb` に渡した
//! サイズで PSAPI が埋める範囲が決まるだけなので、working set の値は変わらない。

/// `PROCESS_MEMORY_COUNTERS_EX` のうちテストが読む値。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProcessMemorySample {
    pub(crate) working_set_size: usize,
    pub(crate) peak_working_set_size: usize,
    pub(crate) private_usage: usize,
    pub(crate) peak_pagefile_usage: usize,
}

/// 呼び出し元プロセスの現在のメモリ使用量を 1 回読む。
pub(crate) fn sample_process_memory() -> ProcessMemorySample {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }

    let mut counters = ProcessMemoryCountersEx {
        cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
        private_usage: 0,
    };
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        )
    };
    assert_ne!(ok, 0, "GetProcessMemoryInfo failed");
    ProcessMemorySample {
        working_set_size: counters.working_set_size,
        peak_working_set_size: counters.peak_working_set_size,
        private_usage: counters.private_usage,
        peak_pagefile_usage: counters.peak_pagefile_usage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// このヘルパーの実利用者 (similar_index / full_inventory_benchmark_tests) は
    /// どちらも `#[ignore]` の手動計測なので、Ex 構造体サイズでの呼び出しが
    /// 成功することを通常の `cargo test` で 1 回だけ確かめておく。
    #[test]
    fn sampling_returns_a_nonzero_working_set() {
        let sample = sample_process_memory();
        assert!(sample.working_set_size > 0, "{sample:?}");
        assert!(
            sample.peak_working_set_size >= sample.working_set_size,
            "{sample:?}"
        );
        assert!(sample.private_usage > 0, "{sample:?}");
    }
}
