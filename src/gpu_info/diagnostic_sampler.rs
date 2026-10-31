//! Worker-only memory sampling for previous-run diagnostics.
//!
//! The UI may clone `AdapterProbe`; resolving its device identity and every
//! native query belong to the diagnostic worker, outside the mailbox lock.
//! Values describe the rendering adapter's process usage, never AI allocations.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Cloning this value does not call COM/DXGI or borrow the HAL device.
#[derive(Clone)]
pub(crate) struct AdapterProbe(Arc<dyn WorkerProbe + Send + Sync>);

// Keep wgpu's concrete graph behind this worker-only interface: publication
// owners need only the bounded probe's Send/Sync contract, not that graph.
trait WorkerProbe {
    fn descriptor(&self) -> ProbeDescriptor;
}

struct RenderAdapterProbe {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
}

impl AdapterProbe {
    pub(crate) fn from_render_state(state: &egui_wgpu::RenderState) -> Self {
        Self(Arc::new(RenderAdapterProbe {
            adapter: state.adapter.clone(),
            device: state.device.clone(),
        }))
    }

    fn descriptor(&self) -> ProbeDescriptor {
        self.0.descriptor()
    }

    /// In-memory source for ownership tests; never creates or queries a GPU.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        struct TestProbe;
        impl WorkerProbe for TestProbe {
            fn descriptor(&self) -> ProbeDescriptor {
                ProbeDescriptor {
                    identity: AdapterIdentity {
                        vendor_id: 100,
                        device_id: 200,
                        backend: DiagnosticBackend::Vulkan,
                        luid: None,
                    },
                    uma: None,
                }
            }
        }
        Self(Arc::new(TestProbe))
    }
}

impl WorkerProbe for RenderAdapterProbe {
    fn descriptor(&self) -> ProbeDescriptor {
        let info = self.adapter.get_info();
        ProbeDescriptor {
            identity: AdapterIdentity {
                vendor_id: info.vendor,
                device_id: info.device,
                backend: info.backend.into(),
                luid: device_luid(&self.device, info.backend),
            },
            // wgpu-hal uses windows 0.62, while this module uses windows 0.61.
            // Do not add a raw COM cast/dependency merely to query ARCHITECTURE.
            // DedicatedVideoMemory == 0 is not an authoritative UMA query.
            uma: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiagnosticBackend {
    Noop,
    Vulkan,
    Metal,
    Dx12,
    Gl,
    BrowserWebGpu,
}

impl From<wgpu::Backend> for DiagnosticBackend {
    fn from(value: wgpu::Backend) -> Self {
        match value {
            wgpu::Backend::Noop => Self::Noop,
            wgpu::Backend::Vulkan => Self::Vulkan,
            wgpu::Backend::Metal => Self::Metal,
            wgpu::Backend::Dx12 => Self::Dx12,
            wgpu::Backend::Gl => Self::Gl,
            wgpu::Backend::BrowserWebGpu => Self::BrowserWebGpu,
        }
    }
}

/// Only numeric/known-enum fields enter the stored record. Adapter/driver names
/// from get_info/GetDesc1 must not enter diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdapterIdentity {
    pub(crate) vendor_id: u32,
    pub(crate) device_id: u32,
    pub(crate) backend: DiagnosticBackend,
    pub(crate) luid: Option<[u32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GpuSampleStatus {
    Available,
    NoRenderAdapter,
    UnsupportedBackend,
    LuidUnavailable,
    AdapterMismatch,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiAdapterCorrelation {
    /// ORT owns its DirectML device. No identity has been obtained from it.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SegmentMemory {
    pub(crate) usage_bytes: u64,
    pub(crate) budget_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GpuMemorySample {
    pub(crate) status: GpuSampleStatus,
    pub(crate) adapter: Option<AdapterIdentity>,
    pub(crate) node_index: Option<u32>,
    /// None means ARCHITECTURE has not been queried; never inferred from size.
    pub(crate) uma: Option<bool>,
    pub(crate) local: Option<SegmentMemory>,
    pub(crate) nonlocal: Option<SegmentMemory>,
}

impl Default for GpuMemorySample {
    fn default() -> Self {
        Self {
            status: GpuSampleStatus::NoRenderAdapter,
            adapter: None,
            node_index: None,
            uma: None,
            local: None,
            nonlocal: None,
        }
    }
}

/// Fresh values only. The caller stamps the sampling operation using its run
/// UTC/monotonic clock; this type has neither a second clock nor stale values.
/// Missing CPU PrivateUsage means the query failed/is unsupported, not zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemorySample {
    pub(crate) cpu_private_bytes: Option<u64>,
    pub(crate) rendering: GpuMemorySample,
    pub(crate) ai_adapter_correlation: AiAdapterCorrelation,
}

impl Default for MemorySample {
    fn default() -> Self {
        Self {
            cpu_private_bytes: None,
            rendering: GpuMemorySample::default(),
            ai_adapter_correlation: AiAdapterCorrelation::Unknown,
        }
    }
}

/// Construct and retain on the diagnostic worker. Native handles never enter
/// the shared latest-value mailbox and Drop does not invalidate other owners.
#[derive(Default)]
pub(crate) struct DiagnosticSampler {
    reader: NativeReader,
    cpu_failure_logged: bool,
    gpu_failure_logged: bool,
}

impl DiagnosticSampler {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn sample(&mut self, probe: Option<&AdapterProbe>) -> MemorySample {
        self.sample_timed(probe, || ()).0
    }

    /// Capture each observation immediately after its query, including failed
    /// observations. `stamp` supplies the caller's UTC/run-monotonic clock.
    pub(crate) fn sample_timed<T>(
        &mut self,
        probe: Option<&AdapterProbe>,
        stamp: impl FnMut() -> T,
    ) -> (MemorySample, T, T) {
        let descriptor = probe.map(AdapterProbe::descriptor);
        let (sample, cpu_stamp, gpu_stamp) =
            sample_with_reader_timed(descriptor, &mut self.reader, stamp);
        if sample.cpu_private_bytes.is_none() && !self.cpu_failure_logged {
            self.cpu_failure_logged = true;
            crate::logger::log("Previous-run diagnostics: CPU private memory unavailable");
        }
        if matches!(
            sample.rendering.status,
            GpuSampleStatus::LuidUnavailable
                | GpuSampleStatus::AdapterMismatch
                | GpuSampleStatus::Unavailable
        ) && !self.gpu_failure_logged
        {
            self.gpu_failure_logged = true;
            crate::logger::log("Previous-run diagnostics: rendering adapter memory unavailable");
        }
        (sample, cpu_stamp, gpu_stamp)
    }
}

#[derive(Clone, Copy)]
struct ProbeDescriptor {
    identity: AdapterIdentity,
    uma: Option<bool>,
}

#[derive(Clone, Copy)]
enum Segment {
    Local,
    Nonlocal,
}

/// This seam exercises correlation/failures without opening a real GPU/device.
trait MemoryReader {
    fn cpu_private_bytes(&mut self) -> Option<u64>;
    fn resolve(&mut self, requested: AdapterIdentity) -> Option<AdapterIdentity>;
    fn segment(&mut self, segment: Segment) -> Option<SegmentMemory>;
}

#[cfg(test)]
fn sample_with_reader(
    probe: Option<ProbeDescriptor>,
    reader: &mut impl MemoryReader,
) -> MemorySample {
    sample_with_reader_timed(probe, reader, || ()).0
}

fn sample_with_reader_timed<T>(
    probe: Option<ProbeDescriptor>,
    reader: &mut impl MemoryReader,
    mut stamp: impl FnMut() -> T,
) -> (MemorySample, T, T) {
    let cpu_private_bytes = reader.cpu_private_bytes();
    let cpu_stamp = stamp();
    let rendering = sample_gpu_with_reader(probe, reader);
    let gpu_stamp = stamp();
    (
        MemorySample {
            cpu_private_bytes,
            rendering,
            ai_adapter_correlation: AiAdapterCorrelation::Unknown,
        },
        cpu_stamp,
        gpu_stamp,
    )
}

fn sample_gpu_with_reader(
    probe: Option<ProbeDescriptor>,
    reader: &mut impl MemoryReader,
) -> GpuMemorySample {
    let mut gpu = GpuMemorySample::default();
    let Some(probe) = probe else {
        return gpu;
    };
    gpu.adapter = Some(probe.identity);
    gpu.uma = probe.uma;
    if probe.identity.backend != DiagnosticBackend::Dx12 {
        gpu.status = GpuSampleStatus::UnsupportedBackend;
        return gpu;
    }
    if probe.identity.luid.is_none() {
        gpu.status = GpuSampleStatus::LuidUnavailable;
        return gpu;
    }
    let Some(resolved) = reader.resolve(probe.identity) else {
        gpu.status = GpuSampleStatus::Unavailable;
        return gpu;
    };
    if resolved != probe.identity {
        gpu.status = GpuSampleStatus::AdapterMismatch;
        return gpu;
    }
    gpu.node_index = Some(0);
    gpu.local = reader.segment(Segment::Local);
    gpu.nonlocal = reader.segment(Segment::Nonlocal);
    gpu.status = if gpu.local.is_some() && gpu.nonlocal.is_some() {
        GpuSampleStatus::Available
    } else {
        GpuSampleStatus::Unavailable
    };
    gpu
}

#[cfg(windows)]
fn device_luid(device: &wgpu::Device, backend: wgpu::Backend) -> Option<[u32; 2]> {
    if backend != wgpu::Backend::Dx12 {
        return None;
    }
    // SAFETY: Only borrows the existing device on the background worker. There
    // is no submission, wait, destruction, or retained HAL/raw COM pointer.
    let device = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }?;
    // SAFETY: GetAdapterLuid reads immutable identity while the HAL guard lives.
    let luid = unsafe { device.raw_device().GetAdapterLuid() };
    Some([luid.LowPart, luid.HighPart as u32])
}

#[cfg(not(windows))]
fn device_luid(_device: &wgpu::Device, _backend: wgpu::Backend) -> Option<[u32; 2]> {
    None
}

#[cfg(windows)]
#[derive(Default)]
struct NativeReader {
    cached: Option<(
        AdapterIdentity,
        windows::Win32::Graphics::Dxgi::IDXGIAdapter3,
    )>,
}

#[cfg(windows)]
impl MemoryReader for NativeReader {
    fn cpu_private_bytes(&mut self) -> Option<u64> {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        // SAFETY: The initialized, correctly sized output buffer belongs to
        // this call; the pseudo process handle is borrowed and never closed.
        unsafe {
            let mut memory: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            memory.cb = std::mem::size_of_val(&memory) as u32;
            let ok = GetProcessMemoryInfo(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                &mut memory as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                memory.cb,
            );
            (ok != 0).then_some(memory.PrivateUsage as u64)
        }
    }

    fn resolve(&mut self, requested: AdapterIdentity) -> Option<AdapterIdentity> {
        use windows::Win32::Foundation::LUID;
        use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory4};
        if let Some((identity, _)) = &self.cached
            && *identity == requested
        {
            return Some(*identity);
        }
        // A changed source never uses the previous adapter's cached values.
        self.cached = None;
        let [low, high] = requested.luid?;
        // SAFETY: Factory/adapter handles are local to this worker; lookup is
        // by the actual rendering-device LUID, never enumeration order/size.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory1().ok()?;
            let adapter: IDXGIAdapter3 = factory
                .EnumAdapterByLuid(LUID {
                    LowPart: low,
                    HighPart: high as i32,
                })
                .ok()?;
            let desc = adapter.GetDesc1().ok()?;
            let identity = AdapterIdentity {
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                backend: DiagnosticBackend::Dx12,
                luid: Some([desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart as u32]),
            };
            // DedicatedVideoMemory may be zero (UMA). Do not exclude it.
            self.cached = Some((identity, adapter));
            Some(identity)
        }
    }

    fn segment(&mut self, segment: Segment) -> Option<SegmentMemory> {
        use windows::Win32::Graphics::Dxgi::{
            DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL,
            DXGI_QUERY_VIDEO_MEMORY_INFO,
        };
        let (_, adapter) = self.cached.as_ref()?;
        let group = match segment {
            Segment::Local => DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
            Segment::Nonlocal => DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL,
        };
        let mut memory = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: Reads process usage/budget from matched adapter/node 0 into
        // a local correctly sized buffer; no device creation or GPU submission.
        unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut memory) }.ok()?;
        Some(SegmentMemory {
            usage_bytes: memory.CurrentUsage,
            budget_bytes: memory.Budget,
        })
    }
}

#[cfg(not(windows))]
#[derive(Default)]
struct NativeReader;

#[cfg(not(windows))]
impl MemoryReader for NativeReader {
    fn cpu_private_bytes(&mut self) -> Option<u64> {
        None
    }

    fn resolve(&mut self, _requested: AdapterIdentity) -> Option<AdapterIdentity> {
        None
    }

    fn segment(&mut self, _segment: Segment) -> Option<SegmentMemory> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeReader {
        cpu: Option<u64>,
        resolved: Option<AdapterIdentity>,
        local: Option<SegmentMemory>,
        nonlocal: Option<SegmentMemory>,
        resolutions: usize,
        reads: usize,
    }

    impl Default for FakeReader {
        fn default() -> Self {
            Self {
                cpu: Some(1234),
                resolved: Some(descriptor().identity),
                local: Some(SegmentMemory {
                    usage_bytes: 3,
                    budget_bytes: 4,
                }),
                nonlocal: Some(SegmentMemory {
                    usage_bytes: 5,
                    budget_bytes: 6,
                }),
                resolutions: 0,
                reads: 0,
            }
        }
    }

    impl MemoryReader for FakeReader {
        fn cpu_private_bytes(&mut self) -> Option<u64> {
            self.cpu
        }
        fn resolve(&mut self, _requested: AdapterIdentity) -> Option<AdapterIdentity> {
            self.resolutions += 1;
            self.resolved
        }
        fn segment(&mut self, segment: Segment) -> Option<SegmentMemory> {
            self.reads += 1;
            match segment {
                Segment::Local => self.local,
                Segment::Nonlocal => self.nonlocal,
            }
        }
    }

    fn descriptor() -> ProbeDescriptor {
        ProbeDescriptor {
            identity: AdapterIdentity {
                vendor_id: 0x8086,
                device_id: 100,
                backend: DiagnosticBackend::Dx12,
                luid: Some([1, 2]),
            },
            uma: None,
        }
    }

    #[test]
    fn matching_adapter_reads_both_segments_but_dml_remains_unknown() {
        let mut reader = FakeReader::default();
        let sample = sample_with_reader(Some(descriptor()), &mut reader);
        assert_eq!(sample.rendering.status, GpuSampleStatus::Available);
        assert_eq!(sample.rendering.adapter, Some(descriptor().identity));
        assert_eq!(sample.rendering.node_index, Some(0));
        assert_eq!(sample.rendering.uma, None);
        assert_eq!(sample.rendering.local, reader.local);
        assert_eq!(sample.rendering.nonlocal, reader.nonlocal);
        assert_eq!(sample.ai_adapter_correlation, AiAdapterCorrelation::Unknown);
        assert_eq!(reader.reads, 2);
    }

    #[test]
    fn mismatched_luid_or_pci_identity_never_reads_other_gpu() {
        for identity in [
            AdapterIdentity {
                luid: Some([8, 9]),
                ..descriptor().identity
            },
            AdapterIdentity {
                device_id: 101,
                ..descriptor().identity
            },
            AdapterIdentity {
                vendor_id: 0x10de,
                ..descriptor().identity
            },
        ] {
            let mut reader = FakeReader {
                resolved: Some(identity),
                ..FakeReader::default()
            };
            let sample = sample_with_reader(Some(descriptor()), &mut reader);
            assert_eq!(sample.rendering.status, GpuSampleStatus::AdapterMismatch);
            assert_eq!(sample.rendering.local, None);
            assert_eq!(sample.rendering.nonlocal, None);
            assert_eq!(sample.rendering.node_index, None);
            assert_eq!(reader.reads, 0);
        }
    }

    #[test]
    fn uma_with_zero_local_budget_is_not_filtered_or_inferred() {
        let mut probe = descriptor();
        probe.uma = Some(true);
        let mut reader = FakeReader {
            local: Some(SegmentMemory {
                usage_bytes: 0,
                budget_bytes: 0,
            }),
            ..FakeReader::default()
        };
        let sample = sample_with_reader(Some(probe), &mut reader);
        assert_eq!(sample.rendering.status, GpuSampleStatus::Available);
        assert_eq!(sample.rendering.uma, Some(true));
        assert_eq!(sample.rendering.local.unwrap().budget_bytes, 0);
        assert_eq!(reader.reads, 2);
        probe.uma = None;
        assert_eq!(
            sample_with_reader(Some(probe), &mut reader).rendering.uma,
            None
        );
    }

    #[test]
    fn missing_luid_and_non_dx12_do_not_resolve_or_read() {
        let mut unknown = descriptor();
        unknown.identity.luid = None;
        let mut vulkan = descriptor();
        vulkan.identity.backend = DiagnosticBackend::Vulkan;
        for (probe, expected) in [
            (None, GpuSampleStatus::NoRenderAdapter),
            (Some(unknown), GpuSampleStatus::LuidUnavailable),
            (Some(vulkan), GpuSampleStatus::UnsupportedBackend),
        ] {
            let mut reader = FakeReader::default();
            let sample = sample_with_reader(probe, &mut reader);
            assert_eq!(sample.rendering.status, expected);
            assert_eq!((reader.resolutions, reader.reads), (0, 0));
            assert_eq!(sample.rendering.local, None);
            assert_eq!(sample.rendering.nonlocal, None);
        }
    }

    #[test]
    fn failed_queries_do_not_reuse_successful_values_or_replace_with_zero() {
        let mut reader = FakeReader::default();
        assert_eq!(
            sample_with_reader(Some(descriptor()), &mut reader)
                .rendering
                .status,
            GpuSampleStatus::Available
        );
        reader.cpu = None;
        reader.local = None;
        let failed = sample_with_reader(Some(descriptor()), &mut reader);
        assert_eq!(failed.cpu_private_bytes, None);
        assert_eq!(failed.rendering.status, GpuSampleStatus::Unavailable);
        assert_eq!(failed.rendering.local, None);
        assert_eq!(failed.rendering.nonlocal, reader.nonlocal);
        reader.resolved = None;
        reader.reads = 0;
        let unresolved = sample_with_reader(Some(descriptor()), &mut reader);
        assert_eq!(unresolved.rendering.status, GpuSampleStatus::Unavailable);
        assert_eq!(unresolved.rendering.nonlocal, None);
        assert_eq!(reader.reads, 0);
    }

    #[test]
    fn record_roundtrip_contains_only_allowlisted_fields() {
        let sample = sample_with_reader(Some(descriptor()), &mut FakeReader::default());
        let encoded = serde_json::to_string(&sample).unwrap();
        assert_eq!(
            serde_json::from_str::<MemorySample>(&encoded).unwrap(),
            sample
        );
        let mut record = serde_json::to_value(sample).unwrap();
        record["driver_name"] = serde_json::json!("private path");
        assert!(serde_json::from_value::<MemorySample>(record).is_err());
    }

    #[test]
    fn stamps_follow_individual_queries_even_when_gpu_is_slow_or_queries_fail() {
        use std::cell::Cell;

        struct TimedReader<'a> {
            reader: FakeReader,
            clock: &'a Cell<u64>,
        }

        impl MemoryReader for TimedReader<'_> {
            fn cpu_private_bytes(&mut self) -> Option<u64> {
                self.clock.set(self.clock.get() + 1);
                self.reader.cpu_private_bytes()
            }

            fn resolve(&mut self, requested: AdapterIdentity) -> Option<AdapterIdentity> {
                // A slow native adapter query must not move the CPU timestamp.
                self.clock.set(self.clock.get() + 1000);
                self.reader.resolve(requested)
            }

            fn segment(&mut self, segment: Segment) -> Option<SegmentMemory> {
                self.clock.set(self.clock.get() + 100);
                self.reader.segment(segment)
            }
        }

        for (cpu_fails, gpu_fails) in [(false, false), (true, false), (false, true), (true, true)] {
            let clock = Cell::new(0);
            let mut reader = TimedReader {
                reader: FakeReader {
                    cpu: (!cpu_fails).then_some(1234),
                    resolved: (!gpu_fails).then_some(descriptor().identity),
                    ..FakeReader::default()
                },
                clock: &clock,
            };
            let (sample, cpu_stamp, gpu_stamp) =
                sample_with_reader_timed(Some(descriptor()), &mut reader, || clock.get());
            assert_eq!(cpu_stamp, 1);
            assert_eq!(gpu_stamp, if gpu_fails { 1001 } else { 1201 });
            assert_eq!(sample.cpu_private_bytes.is_none(), cpu_fails);
            assert_eq!(
                sample.rendering.status == GpuSampleStatus::Unavailable,
                gpu_fails
            );
        }
    }
}
