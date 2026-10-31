//! One startup-only owner for root GPU observations and dependency errors.
//! No I/O occurs while its lock is held; the logger keeps the complete lines.
use crate::wgpu_backend_policy::GpuBackendRequest;
use egui_wgpu::{StartupDiagnosticEvent as Event, StartupGpuStage as Stage};
use std::sync::Mutex;
use std::thread::ThreadId;

static OWNER: Mutex<Option<Diagnostics>> = Mutex::new(None);

pub(crate) fn is_gpu_target(target: &str) -> bool {
    ["wgpu_hal", "wgpu_core", "gpu_allocator"]
        .iter()
        .any(|name| {
            target == *name
                || target
                    .strip_prefix(name)
                    .is_some_and(|s| s.starts_with("::"))
        })
}

pub(crate) fn capturing() -> bool {
    OWNER.lock().unwrap().as_ref().is_some_and(|d| d.active)
}

pub(crate) fn begin(request: GpuBackendRequest) {
    *OWNER.lock().unwrap() = Some(Diagnostics::new(request));
    miv_startup::record_detail("gpu.request", request.as_str());
}

pub(crate) fn error_line(line: &str) {
    if let Some(owner) = OWNER.lock().unwrap().as_mut() {
        owner.error_line(std::thread::current().id(), line);
    }
}

pub(crate) fn candidates(backends: &[wgpu::Backend]) {
    if let Some(owner) = OWNER.lock().unwrap().as_mut() {
        owner.snapshot.vulkan_available = Some(backends.contains(&wgpu::Backend::Vulkan));
    }
}

pub(crate) fn observe(event: &Event) {
    let lines = OWNER.lock().unwrap().as_mut().map(|d| d.observe(event));
    // Dependency logger also acquires OWNER: enqueue only after releasing it.
    if let Some(lines) = lines {
        for (key, value, log_line) in lines {
            miv_startup::record_detail(key, &value);
            crate::logger::log(log_line);
        }
    }
}

pub(crate) fn stop_snapshot() -> Snapshot {
    OWNER
        .lock()
        .unwrap()
        .as_mut()
        .map_or_else(Snapshot::default, |d| {
            d.active = false;
            d.snapshot.lines = d.lines.clone();
            if d.snapshot.lines.is_empty() {
                d.snapshot.lines = d
                    .snapshot
                    .attempts
                    .last()
                    .map(|a| a.lines.clone())
                    .unwrap_or_default();
            }
            d.snapshot.clone()
        })
}

/// Bounded single-line text; character boundaries are preserved in UTF-8.
pub(crate) fn short(text: &str, max: usize) -> String {
    let mut chars = text.chars().map(|c| if c.is_control() { ' ' } else { c });
    let mut result: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        result.push_str("...");
    }
    result
}

pub(crate) fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Dx12 => "DX12",
        wgpu::Backend::Vulkan => "Vulkan",
        _ => "不明",
    }
}

pub(crate) fn driver(info: &wgpu::AdapterInfo) -> String {
    if info.backend == wgpu::Backend::Dx12 {
        for value in [&info.driver_info, &info.driver] {
            if value.split('.').count() == 4
                && value
                    .split('.')
                    .all(|n| !n.is_empty() && n.parse::<u16>().is_ok())
                && value != "0.0.0.0"
            {
                return value.clone();
            }
        }
        "不明".into()
    } else {
        let value = format!("{} {}", info.driver, info.driver_info);
        if value.trim().is_empty() {
            "不明".into()
        } else {
            value.trim().into()
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Attempt {
    pub info: wgpu::AdapterInfo,
    pub result: Result<(), String>,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    pub request: GpuBackendRequest,
    pub adapter: Option<wgpu::AdapterInfo>,
    pub stage: Option<Stage>,
    pub lines: Vec<String>,
    pub attempts: Vec<Attempt>,
    pub vulkan_available: Option<bool>,
}

impl Snapshot {
    fn advice(&self) -> Option<&'static str> {
        match self.request {
            GpuBackendRequest::Dx12 => Some("vulkan"),
            GpuBackendRequest::Vulkan => Some("dx12"),
            GpuBackendRequest::Auto => {
                let selected_dx12 = self
                    .adapter
                    .as_ref()
                    .is_some_and(|a| a.backend == wgpu::Backend::Dx12);
                let dx12_device_failed = self
                    .attempts
                    .iter()
                    .any(|a| a.info.backend == wgpu::Backend::Dx12 && a.result.is_err());
                (selected_dx12
                    && !dx12_device_failed
                    && self.vulkan_available != Some(false)
                    && !self
                        .attempts
                        .iter()
                        .any(|a| a.info.backend == wgpu::Backend::Vulkan))
                .then_some("vulkan")
            }
        }
    }

    pub(crate) fn dialog(&self, stage: Option<Stage>) -> String {
        let gpu = self.adapter.as_ref().map_or_else(
            || "不明".into(),
            |a| {
                format!(
                    "{} / {} / {}",
                    short(&a.name, 100),
                    backend_name(a.backend),
                    match a.device_type {
                        wgpu::DeviceType::DiscreteGpu => "専用 GPU",
                        wgpu::DeviceType::IntegratedGpu => "内蔵 GPU",
                        wgpu::DeviceType::VirtualGpu => "仮想 GPU",
                        wgpu::DeviceType::Cpu => "CPU",
                        wgpu::DeviceType::Other => "不明",
                    }
                )
            },
        );
        let version = self
            .adapter
            .as_ref()
            .map(driver)
            .unwrap_or_else(|| "不明".into());
        let stage = match stage.or(self.stage) {
            Some(Stage::Device) => "DeviceRequest".into(),
            Some(s) => format!("{s:?}"),
            None => "不明".into(),
        };
        let lines = if self.lines.is_empty() {
            "追加情報なし".into()
        } else {
            self.lines
                .iter()
                .map(|s| short(s, 200))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let mut body = format!(
            "\n\nGPU: {gpu}\nドライバ: {}\n段階: {stage}\nGPU 診断: {lines}",
            short(&version, 100)
        );
        if self.attempts.len() > 1 {
            body.push_str(&format!(
                "\n試した API: {}",
                self.attempts
                    .iter()
                    .map(|a| format!(
                        "{} ({})",
                        backend_name(a.info.backend),
                        if a.result.is_ok() { "成功" } else { "失敗" }
                    ))
                    .collect::<Vec<_>>()
                    .join(" → ")
            ));
        }
        if let Some(api) = self.advice() {
            body.push_str(&format!(
                "\n\n別の描画方式で起動するには、--gpu-backend {api} を付けて起動してください。"
            ));
        }
        body
    }
}

struct Diagnostics {
    active: bool,
    thread: ThreadId,
    snapshot: Snapshot,
    lines: Vec<String>,
}

impl Diagnostics {
    fn new(request: GpuBackendRequest) -> Self {
        Self {
            active: true,
            thread: std::thread::current().id(),
            snapshot: Snapshot {
                request,
                ..Default::default()
            },
            lines: Vec::new(),
        }
    }
    fn error_line(&mut self, thread: ThreadId, line: &str) {
        if !self.active || thread != self.thread {
            return;
        }
        if self.lines.len() == 2 {
            self.lines.remove(0);
        }
        self.lines.push(short(line, 512));
    }
    fn observe(&mut self, event: &Event) -> Vec<(&'static str, String, String)> {
        if !self.active || std::thread::current().id() != self.thread {
            return Vec::new();
        }
        match event {
            Event::Begin(stage) => self.snapshot.stage = Some(*stage),
            Event::End { stage, success } => {
                // End(false) is an observation, never proof of fatal failure.
                // Any later successful operation replaces a recovered stage.
                self.snapshot.stage = if *success { None } else { Some(*stage) };
            }
            Event::AdapterSelected(info) => self.snapshot.adapter = Some(info.clone()),
            Event::DeviceAttempt {
                attempt,
                info,
                result,
            } => {
                let lines = std::mem::take(&mut self.lines);
                self.snapshot.attempts.push(Attempt {
                    info: info.clone(),
                    result: result.clone(),
                    lines,
                });
                let detail = format!(
                    "{} {:?} {:?} vendor={} device={} driver={} driver_info={}",
                    info.name,
                    info.backend,
                    info.device_type,
                    info.vendor,
                    info.device,
                    info.driver,
                    info.driver_info
                );
                let outcome = match result {
                    Ok(()) => "ok".into(),
                    Err(e) => format!("error stage=DeviceRequest error={e}"),
                };
                let (result_name, error) = match result {
                    Ok(()) => ("ok", String::new()),
                    Err(error) => ("error", format!(" error={error}")),
                };
                // Root policy has at most one candidate for each of two APIs.
                let (adapter_key, attempt_key) = if *attempt == 1 {
                    ("gpu.adapter.1", "gpu.attempt.1")
                } else {
                    ("gpu.adapter.2", "gpu.attempt.2")
                };
                let mut records = vec![
                    (
                        adapter_key,
                        detail.clone(),
                        short(
                            &format!("gpu adapter attempt={attempt} {detail}"),
                            usize::MAX,
                        ),
                    ),
                    (
                        attempt_key,
                        outcome.clone(),
                        short(
                            &format!(
                                "gpu attempt={attempt} backend={} adapter={} driver={} result={result_name} stage=DeviceRequest{error}",
                                backend_name(info.backend),
                                info.name,
                                driver(info)
                            ),
                            usize::MAX,
                        ),
                    ),
                ];
                if result.is_ok() {
                    self.snapshot.stage = None;
                    records.push((
                        "gpu.selected",
                        format!(
                            "{} request={}",
                            backend_name(info.backend),
                            self.snapshot.request.as_str()
                        ),
                        format!(
                            "gpu selected backend={} request={}",
                            backend_name(info.backend),
                            self.snapshot.request.as_str()
                        ),
                    ));
                }
                return records;
            }
            Event::RootPresented { .. } => {
                self.active = false;
                self.snapshot.stage = None;
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(backend: wgpu::Backend) -> wgpu::AdapterInfo {
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, backend);
        info.name = "Test GPU".into();
        info.vendor = 1;
        info.device = 2;
        info
    }
    fn attempt(d: &mut Diagnostics, n: usize, backend: wgpu::Backend, result: Result<(), String>) {
        d.observe(&Event::AdapterSelected(info(backend)));
        d.observe(&Event::Begin(Stage::Device));
        d.observe(&Event::End {
            stage: Stage::Device,
            success: result.is_ok(),
        });
        d.observe(&Event::DeviceAttempt {
            attempt: n,
            info: info(backend),
            result,
        });
    }

    #[test]
    fn gpu_startup_diagnostic_target_includes_crate_and_children_only() {
        for target in [
            "wgpu_hal",
            "wgpu_hal::dx12",
            "wgpu_core",
            "wgpu_core::instance",
            "gpu_allocator",
            "gpu_allocator::d3d12",
        ] {
            assert!(is_gpu_target(target), "{target}");
        }
        for target in [
            "wgpu",
            "wgpu_hal_extra",
            "wgpu_corex::instance",
            "gpu_allocator_other",
            "other",
        ] {
            assert!(!is_gpu_target(target), "{target}");
        }
    }

    #[test]
    fn gpu_startup_diagnostic_captures_enumeration_but_not_other_threads_or_after_present() {
        let mut d = Diagnostics::new(GpuBackendRequest::Auto);
        let other = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        // Active even before Begin(Instance).
        d.error_line(d.thread, "instance error");
        d.observe(&Event::Begin(Stage::AdapterEnumeration));
        d.error_line(d.thread, "enumeration error");
        d.error_line(other, "HUD error");
        attempt(
            &mut d,
            1,
            wgpu::Backend::Dx12,
            Err("limits exceeded".into()),
        );
        assert_eq!(
            d.snapshot.attempts[0].lines,
            ["instance error", "enumeration error"]
        );
        assert!(d.lines.is_empty());
        attempt(&mut d, 2, wgpu::Backend::Vulkan, Ok(()));
        assert!(d.snapshot.attempts[1].lines.is_empty());
        assert_eq!(d.snapshot.stage, None);
        assert!(d.snapshot.attempts[0].result.is_err());
        assert_eq!(
            d.snapshot.adapter.as_ref().unwrap().backend,
            wgpu::Backend::Vulkan
        );
        d.observe(&Event::End {
            stage: Stage::SurfaceAcquire,
            success: false,
        });
        d.observe(&Event::Begin(Stage::SurfaceConfigure));
        d.observe(&Event::End {
            stage: Stage::SurfaceConfigure,
            success: true,
        });
        assert!(d.snapshot.stage.is_none());
        d.observe(&Event::RootPresented {
            frame: 1,
            normal: false,
        });
        assert!(!d.active);
        d.error_line(d.thread, "runtime error");
        assert!(d.lines.is_empty());
    }

    #[test]
    fn gpu_startup_diagnostic_keeps_two_bounded_utf8_lines_per_attempt() {
        let mut d = Diagnostics::new(GpuBackendRequest::Auto);
        d.error_line(d.thread, "oldest");
        d.error_line(d.thread, "second");
        d.error_line(d.thread, &"長".repeat(900));
        assert_eq!(d.lines.len(), 2);
        assert_eq!(d.lines[0], "second");
        assert_eq!(d.lines[1].chars().count(), 515);
        assert!(d.lines[1].ends_with("..."));
        attempt(&mut d, 1, wgpu::Backend::Dx12, Err("first".into()));
        d.error_line(d.thread, "vulkan line");
        let records = d.observe(&Event::DeviceAttempt {
            attempt: 2,
            info: info(wgpu::Backend::Vulkan),
            result: Err("second".into()),
        });
        assert!(records.iter().any(|(_, _, l)| l.contains("error=second")));
        assert_eq!(d.snapshot.attempts[0].lines[0], "second");
        assert_eq!(d.snapshot.attempts[1].lines, ["vulkan line"]);
    }

    #[test]
    fn gpu_startup_diagnostic_driver_unknown_and_four_component_dx12() {
        let mut a = info(wgpu::Backend::Dx12);
        assert_eq!(driver(&a), "不明");
        a.driver_info = "0.0.0.0".into();
        assert_eq!(driver(&a), "不明");
        a.driver_info = "32.0.15.1234".into();
        assert_eq!(driver(&a), "32.0.15.1234");
        a.driver_info = "not a version".into();
        assert_eq!(driver(&a), "不明");
        a.backend = wgpu::Backend::Vulkan;
        assert_eq!(driver(&a), "not a version");
    }

    #[test]
    fn gpu_startup_diagnostic_advice_obeys_request_attempts_and_known_availability() {
        let mut d = Diagnostics::new(GpuBackendRequest::Auto);
        assert_eq!(d.snapshot.advice(), None); // No invented last GPU.
        assert!(d.snapshot.dialog(None).contains("GPU: 不明"));
        assert!(d.snapshot.dialog(None).contains("追加情報なし"));
        d.observe(&Event::AdapterSelected(info(wgpu::Backend::Dx12)));
        assert_eq!(d.snapshot.advice(), Some("vulkan"));
        d.snapshot.vulkan_available = Some(false);
        assert_eq!(d.snapshot.advice(), None);
        d.snapshot.vulkan_available = Some(true);
        attempt(&mut d, 1, wgpu::Backend::Dx12, Err("limits".into()));
        assert_eq!(d.snapshot.advice(), None);
        attempt(&mut d, 2, wgpu::Backend::Vulkan, Err("lost".into()));
        assert_eq!(d.snapshot.advice(), None);
        assert!(
            d.snapshot
                .dialog(Some(Stage::Device))
                .contains("DX12 (失敗) → Vulkan (失敗)")
        );
        d.snapshot.request = GpuBackendRequest::Dx12;
        assert_eq!(d.snapshot.advice(), Some("vulkan"));
        d.snapshot.request = GpuBackendRequest::Vulkan;
        assert_eq!(d.snapshot.advice(), Some("dx12"));
        d.snapshot.lines = vec!["one".into(), "two".into()];
        assert!(d.snapshot.dialog(None).contains("GPU 診断: one\ntwo"));
    }
}
