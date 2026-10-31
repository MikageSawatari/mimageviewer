//! The single post-run boundary, including failures before an App exists.
use std::error::Error;
use std::path::{Path, PathBuf};

pub(crate) struct FailureReport {
    pub kind: &'static str,
    pub log_detail: String,
    pub dialog_body: String,
}

/// No filesystem lookup: even a missing logs directory must be displayable.
fn absolute_logs_dir(data_dir: &Path) -> std::io::Result<PathBuf> {
    std::path::absolute(data_dir.join("logs"))
}

/// Classification and wording have no side effects. Both renderers are enabled
/// in Cargo.toml; native-only variants follow eframe/egui-wgpu's target cfgs.
#[cfg(test)]
fn run_native_failure_report(error: &eframe::Error, logs_dir_display: &str) -> FailureReport {
    report_with_gpu(error, logs_dir_display, &Default::default())
}

fn report_with_gpu(
    error: &eframe::Error,
    logs_dir_display: &str,
    gpu: &crate::gpu_startup_diagnostics::Snapshot,
) -> FailureReport {
    let (kind, graphics, inner): (&str, bool, Option<&dyn Error>) = match error {
        eframe::Error::AppCreation(cause) => ("AppCreation", false, Some(cause.as_ref())),
        #[cfg(not(target_arch = "wasm32"))]
        eframe::Error::Winit(cause) => ("Winit", true, Some(cause)),
        #[cfg(not(target_arch = "wasm32"))]
        eframe::Error::WinitEventLoop(cause) => ("WinitEventLoop", false, Some(cause)),
        #[cfg(not(target_arch = "wasm32"))]
        eframe::Error::Glutin(cause) => ("Glutin", false, Some(cause)),
        #[cfg(not(target_arch = "wasm32"))]
        eframe::Error::NoGlutinConfigs(_, cause) => {
            ("NoGlutinConfigs", false, Some(cause.as_ref()))
        }
        eframe::Error::OpenGL(cause) => ("OpenGL", false, Some(cause)),
        eframe::Error::Wgpu(cause) => {
            let (kind, inner): (&str, Option<&dyn Error>) = match cause {
                #[cfg(windows)]
                egui_wgpu::WgpuError::WindowSizeLimit(cause) => {
                    ("Wgpu.WindowSizeLimit", Some(cause))
                }
                egui_wgpu::WgpuError::RequestAdapterError(cause) => {
                    ("Wgpu.RequestAdapterError", Some(cause))
                }
                egui_wgpu::WgpuError::CustomNativeAdapterSelectionError(_) => {
                    ("Wgpu.CustomNativeAdapterSelectionError", None)
                }
                egui_wgpu::WgpuError::NoSurfaceFormatsAvailable => {
                    ("Wgpu.NoSurfaceFormatsAvailable", None)
                }
                egui_wgpu::WgpuError::UnsupportedSurface { .. } => {
                    ("Wgpu.UnsupportedSurface", None)
                }
                egui_wgpu::WgpuError::RequestDeviceError(cause) => {
                    ("Wgpu.RequestDeviceError", Some(cause))
                }
                egui_wgpu::WgpuError::CreateSurfaceError(cause) => {
                    ("Wgpu.CreateSurfaceError", Some(cause))
                }
                egui_wgpu::WgpuError::HandleError(cause) => ("Wgpu.HandleError", Some(cause)),
            };
            (kind, true, inner)
        }
    };
    // eframe::Error does not expose source(). Transparent WgpuError variants
    // may also skip their contained error, so start from the explicit payload.
    let mut detail = error.to_string();
    let mut next = inner;
    while let Some(cause) = next {
        detail.push_str(&format!(" -> {cause}"));
        next = cause.source();
    }
    let summary = error.to_string();
    let mut chars = summary.chars().map(|c| {
        if c.is_whitespace() || c == '\0' {
            ' '
        } else {
            c
        }
    });
    let mut short: String = chars.by_ref().take(200).collect();
    if chars.next().is_some() {
        short.push_str("...");
    }
    let first_line = if graphics {
        "画面の表示処理に失敗したため、mImageViewer を終了します。"
    } else {
        "mImageViewer の実行中に問題が起きたため、終了します。"
    };
    let guidance = if graphics {
        "グラフィックスドライバーを最新版に更新すると直ることがあります。\n直らない場合は、"
    } else {
        ""
    };
    let mut report = FailureReport {
        kind,
        log_detail: format!("run_native failed: kind={kind} detail={detail}"),
        dialog_body: format!(
            "{first_line}\n\n{guidance}次のフォルダーのログを添えてお知らせください。\n{logs_dir_display}\n\n詳細: {kind}: {short}"
        ),
    };
    if graphics {
        use egui_wgpu::StartupGpuStage as Stage;
        let stage = match error {
            eframe::Error::Wgpu(egui_wgpu::WgpuError::RequestDeviceError(_)) => Some(Stage::Device),
            eframe::Error::Wgpu(
                egui_wgpu::WgpuError::RequestAdapterError(_)
                | egui_wgpu::WgpuError::CustomNativeAdapterSelectionError(_),
            ) => Some(Stage::AdapterSelection),
            eframe::Error::Wgpu(egui_wgpu::WgpuError::NoSurfaceFormatsAvailable) => {
                Some(Stage::PipelineCreate)
            }
            eframe::Error::Wgpu(egui_wgpu::WgpuError::UnsupportedSurface { .. }) => {
                Some(Stage::SurfaceCapabilities)
            }
            eframe::Error::Wgpu(egui_wgpu::WgpuError::CreateSurfaceError(_)) => {
                Some(Stage::SurfaceCreate)
            }
            _ => None,
        };
        report.dialog_body.push_str(&gpu.dialog(stage));
        report
            .log_detail
            .push_str(&format!("; gpu diagnostic={gpu:?}"));
    }
    report
}

// Effects are injected so tests exercise the same shutdown boundary without
// starting eframe, touching process-global logs, or opening a native dialog.
pub(crate) trait ExitEffects {
    fn suspend_watchdog(&mut self);
    fn record(&mut self, report: &FailureReport);
    fn terminal(&mut self, outcome: miv_startup::Outcome);
    fn finish_logs(&mut self, deadline: std::time::Instant);
    fn show(&mut self, body: &str);
}

pub(crate) struct NativeExit;
impl ExitEffects for NativeExit {
    fn suspend_watchdog(&mut self) {
        crate::set_ui_heartbeat_suspended(true, "run_native returned; shutting down".into());
    }
    fn record(&mut self, report: &FailureReport) {
        crate::logger::log(&report.log_detail);
        miv_startup::record_detail("run_native.failure.kind", report.kind);
    }
    fn terminal(&mut self, outcome: miv_startup::Outcome) {
        miv_startup::terminal(outcome);
    }
    fn finish_logs(&mut self, deadline: std::time::Instant) {
        crate::finish_logs_until(deadline);
    }
    fn show(&mut self, body: &str) {
        crate::data_dir::show_fatal_messagebox(body);
    }
}

/// Returned to main before any shutdown wait or user-response wait begins.
pub struct PreparedRunExit {
    result: eframe::Result,
    notification: Option<String>,
}

impl PreparedRunExit {
    pub(crate) fn from_result(result: eframe::Result) -> Self {
        Self {
            result,
            notification: None,
        }
    }
}

/// One final boundary: drain logs and persist the terminal before showing a
/// synchronous dialog. Time spent reading the dialog cannot consume the budget.
pub(crate) fn finish_at_main(
    prepared: PreparedRunExit,
    mut effects: impl ExitEffects,
    save_terminal: impl FnOnce(bool, std::time::Instant),
) -> eframe::Result {
    finish_at_main_until(
        prepared,
        &mut effects,
        save_terminal,
        crate::shutdown_deadline(),
    )
}

fn finish_at_main_until(
    prepared: PreparedRunExit,
    effects: &mut impl ExitEffects,
    save_terminal: impl FnOnce(bool, std::time::Instant),
    deadline: std::time::Instant,
) -> eframe::Result {
    effects.terminal(if prepared.result.is_ok() {
        miv_startup::Outcome::Ok
    } else {
        miv_startup::Outcome::Error
    });
    effects.finish_logs(deadline);
    save_terminal(prepared.result.is_ok(), deadline);
    if let Some(body) = prepared.notification {
        effects.show(&body);
    }
    prepared.result
}

pub(crate) fn finish_run_native(
    result: eframe::Result,
    scripted_run: bool,
    data_dir: &Path,
    cleanup: impl FnOnce(),
    mut effects: impl ExitEffects,
) -> PreparedRunExit {
    // Stop before cleanup can initialize/drop unrelated graphics resources.
    let gpu = crate::gpu_startup_diagnostics::stop_snapshot();
    let report = result.as_ref().err().map(|error| {
        effects.suspend_watchdog();
        // A rare path-resolution failure is recorded and notified, without
        // turning the original rendering error into a panic or retrying I/O.
        let logs_dir = absolute_logs_dir(data_dir);
        let location = match &logs_dir {
            Ok(path) => path.display().to_string(),
            Err(_) => "ログフォルダーの場所を取得できませんでした。".into(),
        };
        let mut report = report_with_gpu(error, &location, &gpu);
        if let Err(error) = logs_dir {
            report
                .log_detail
                .push_str(&format!("; logs directory unavailable: {error}"));
        }
        effects.record(&report);
        report
    });
    cleanup();
    PreparedRunExit {
        result,
        notification: report
            .filter(|_| !scripted_run)
            .map(|report| report.dialog_body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    impl<T: ExitEffects> ExitEffects for &mut T {
        fn suspend_watchdog(&mut self) {
            (**self).suspend_watchdog();
        }
        fn record(&mut self, report: &FailureReport) {
            (**self).record(report);
        }
        fn terminal(&mut self, outcome: miv_startup::Outcome) {
            (**self).terminal(outcome);
        }
        fn finish_logs(&mut self, deadline: std::time::Instant) {
            (**self).finish_logs(deadline);
        }
        fn show(&mut self, body: &str) {
            (**self).show(body);
        }
    }

    // Existing shutdown regression scenarios now cross both product boundaries.
    fn finish_run_native(
        result: eframe::Result,
        scripted_run: bool,
        data_dir: &Path,
        cleanup: impl FnOnce(),
        mut effects: impl ExitEffects,
    ) -> eframe::Result {
        let prepared =
            super::finish_run_native(result, scripted_run, data_dir, cleanup, &mut effects);
        finish_at_main_until(
            prepared,
            &mut effects,
            |_, _| {},
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        )
    }

    #[test]
    fn startup_surface_failure_is_saved_before_notification() {
        for mode in ["dialog", "script"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "run_native_failure::tests::startup_surface_failure_saved_log_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("MIV_STARTUP_SURFACE_FAILURE_TEST_CHILD", mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: isolated log/notification test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    #[ignore = "isolated helper invoked by startup_surface_failure_is_saved_before_notification"]
    fn startup_surface_failure_saved_log_child() {
        let mode = std::env::var("MIV_STARTUP_SURFACE_FAILURE_TEST_CHILD").unwrap();
        assert!(matches!(mode.as_str(), "dialog" | "script"));
        let directory = tempfile::tempdir().unwrap();
        let logs = directory.path().join("logs");
        std::fs::create_dir(&logs).unwrap();
        let writer = crate::log_writer::test_support::writer(&logs, 32, 65536);
        crate::log_writer::test_support::install_global(&writer);
        let log_path = logs.join("mimageviewer.log");
        let shown = RefCell::new(Vec::<String>::new());
        let events = RefCell::new(Vec::new());

        struct SavedLogExit<'a> {
            path: &'a Path,
            events: &'a RefCell<Vec<&'static str>>,
            shown: &'a RefCell<Vec<String>>,
        }
        impl ExitEffects for SavedLogExit<'_> {
            fn suspend_watchdog(&mut self) {
                self.events.borrow_mut().push("suspend");
            }
            fn record(&mut self, report: &FailureReport) {
                // Exercise the actual product path, which calls logger::log
                // directly and does not depend on the dependency-target filter.
                NativeExit.record(report);
                self.events.borrow_mut().push("record");
            }
            fn terminal(&mut self, outcome: miv_startup::Outcome) {
                NativeExit.terminal(outcome);
                self.events.borrow_mut().push("terminal");
            }
            fn finish_logs(&mut self, deadline: std::time::Instant) {
                NativeExit.finish_logs(deadline);
                self.events.borrow_mut().push("flushed");
            }
            fn show(&mut self, body: &str) {
                assert_eq!(self.events.borrow().last(), Some(&"flushed"));
                let saved = std::fs::read_to_string(self.path).unwrap();
                assert!(saved.contains("run_native failed: kind=Wgpu.UnsupportedSurface"));
                assert!(saved.contains("injected startup GPU"));
                assert!(saved.contains("SurfaceCapabilities"));
                assert!(saved.contains("logging shutdown requested"));
                assert!(body.contains("injected startup GPU"));
                assert!(body.contains("段階: SurfaceCapabilities"));
                assert!(body.contains("--gpu-backend vulkan"));
                assert!(body.contains(&self.path.parent().unwrap().display().to_string()));
                self.shown.borrow_mut().push(body.to_owned());
            }
        }

        use crate::gpu_startup_diagnostics as gpu;
        gpu::begin(crate::wgpu_backend_policy::GpuBackendRequest::Dx12);
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, wgpu::Backend::Dx12);
        info.name = "injected startup GPU".into();
        gpu::observe(&egui_wgpu::StartupDiagnosticEvent::AdapterSelected(
            info.clone(),
        ));
        gpu::observe(&egui_wgpu::StartupDiagnosticEvent::Begin(
            egui_wgpu::StartupGpuStage::SurfaceCapabilities,
        ));
        gpu::observe(&egui_wgpu::StartupDiagnosticEvent::End {
            stage: egui_wgpu::StartupGpuStage::SurfaceCapabilities,
            success: false,
        });
        // Inject the same typed Err returned by the first root configuration.
        // No device, window or native MessageBox is created by this fixture.
        let result = finish_run_native(
            Err(eframe::Error::Wgpu(
                egui_wgpu::WgpuError::UnsupportedSurface {
                    adapter: info.name,
                    backend: info.backend,
                },
            )),
            mode == "script",
            directory.path(),
            || events.borrow_mut().push("cleanup"),
            SavedLogExit {
                path: &log_path,
                events: &events,
                shown: &shown,
            },
        );
        assert!(matches!(
            result,
            Err(eframe::Error::Wgpu(
                egui_wgpu::WgpuError::UnsupportedSurface { .. }
            ))
        ));
        assert_eq!(
            *events.borrow(),
            ["suspend", "record", "cleanup", "terminal", "flushed"]
        );
        let saved = std::fs::read_to_string(&log_path).unwrap();
        assert!(saved.contains("run_native failed: kind=Wgpu.UnsupportedSurface"));
        assert!(saved.contains("injected startup GPU"));
        assert!(saved.contains("SurfaceCapabilities"));
        assert!(saved.contains("logging shutdown requested"));
        assert_eq!(shown.borrow().len(), usize::from(mode == "dialog"));
    }

    #[test]
    fn unsupported_surface_reports_gpu_and_capabilities_stage() {
        let error = eframe::Error::Wgpu(egui_wgpu::WgpuError::UnsupportedSurface {
            adapter: "injected GPU".into(),
            backend: wgpu::Backend::Dx12,
        });
        let report = report_with_gpu(&error, "logs", &Default::default());
        assert_eq!(report.kind, "Wgpu.UnsupportedSurface");
        assert!(report.log_detail.contains("injected GPU"));
        assert!(report.dialog_body.contains("injected GPU"));
        assert!(report.dialog_body.contains("段階: SurfaceCapabilities"));
        assert!(report.dialog_body.contains("ドライバー"));
    }

    #[test]
    fn gpu_startup_diagnostics_only_extend_graphics_failure_and_resolve_fatal_stage() {
        let mut gpu = crate::gpu_startup_diagnostics::Snapshot::default();
        gpu.request = crate::wgpu_backend_policy::GpuBackendRequest::Vulkan;
        gpu.stage = Some(egui_wgpu::StartupGpuStage::SurfaceAcquire);
        gpu.lines = vec!["wgpu detail".into()];
        let graphics = report_with_gpu(
            &eframe::Error::Wgpu(egui_wgpu::WgpuError::NoSurfaceFormatsAvailable),
            "logs",
            &gpu,
        );
        assert!(graphics.dialog_body.contains("段階: PipelineCreate"));
        assert!(!graphics.dialog_body.contains("段階: SurfaceAcquire"));
        assert!(graphics.dialog_body.contains("--gpu-backend dx12"));
        assert!(graphics.dialog_body.contains("wgpu detail"));
        let app = report_with_gpu(
            &eframe::Error::AppCreation(Box::new(std::io::Error::other("app"))),
            "logs",
            &gpu,
        );
        assert!(!app.dialog_body.contains("GPU:"));
        assert!(!app.dialog_body.contains("--gpu-backend"));
    }

    struct FakeExit<'a>(&'a RefCell<Vec<String>>);
    impl ExitEffects for FakeExit<'_> {
        fn suspend_watchdog(&mut self) {
            self.0.borrow_mut().push("suspend".into());
        }
        fn record(&mut self, report: &FailureReport) {
            assert!(report.log_detail.contains("injected failure"));
            assert!(report.log_detail.starts_with("run_native failed: kind="));
            self.0.borrow_mut().push(format!("record:{}", report.kind));
        }
        fn terminal(&mut self, outcome: miv_startup::Outcome) {
            self.0.borrow_mut().push(format!("terminal:{outcome:?}"));
        }
        fn finish_logs(&mut self, _deadline: std::time::Instant) {
            self.0.borrow_mut().push("flushed".into());
        }
        fn show(&mut self, body: &str) {
            assert_eq!(self.0.borrow().last().unwrap(), "flushed");
            assert!(body.contains("injected failure"));
            self.0.borrow_mut().push("show".into());
        }
    }

    fn cleanup(events: &RefCell<Vec<String>>) {
        events.borrow_mut().push("cleanup".into());
    }

    // Result injection exists only in this cfg(test) harness. The product calls
    // this boundary with the unmodified result returned by eframe::run_native.
    #[test]
    fn failed_run_records_flushes_then_shows_and_returns_original_error() {
        let events = RefCell::new(Vec::new());
        let error = Box::new(std::io::Error::other("injected failure"));
        let identity = &*error as *const std::io::Error;
        let result = finish_run_native(
            Err(eframe::Error::AppCreation(error)),
            false,
            Path::new("relative-data"),
            || cleanup(&events),
            FakeExit(&events),
        );
        assert_eq!(
            *events.borrow(),
            [
                "suspend",
                "record:AppCreation",
                "cleanup",
                "terminal:Error",
                "flushed",
                "show"
            ]
        );
        let Err(eframe::Error::AppCreation(returned)) = result else {
            panic!("original error lost")
        };
        assert_eq!(
            returned.downcast_ref::<std::io::Error>().unwrap() as *const _,
            identity
        );
    }

    #[test]
    fn scripted_failure_keeps_record_and_flush_but_suppresses_dialog() {
        let events = RefCell::new(Vec::new());
        assert!(
            finish_run_native(
                Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
                    "injected failure"
                )))),
                true,
                Path::new("relative-data"),
                || cleanup(&events),
                FakeExit(&events),
            )
            .is_err()
        );
        assert_eq!(
            *events.borrow(),
            [
                "suspend",
                "record:AppCreation",
                "cleanup",
                "terminal:Error",
                "flushed"
            ]
        );
    }

    #[test]
    fn successful_run_never_reports_or_shows() {
        for scripted in [false, true] {
            let events = RefCell::new(Vec::new());
            finish_run_native(
                Ok(()),
                scripted,
                Path::new("relative-data"),
                || cleanup(&events),
                FakeExit(&events),
            )
            .unwrap();
            assert_eq!(*events.borrow(), ["cleanup", "terminal:Ok", "flushed"]);
        }
    }

    #[test]
    fn returned_error_is_persisted_before_a_slow_notification() {
        use crate::previous_run_diagnostics::{GuiRunDiagnostics, GuiRunOutcome, test_support};
        use std::cell::Cell;
        use std::time::{Duration, Instant};

        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        let elapsed = Cell::new(Duration::ZERO);
        let events = RefCell::new(Vec::new());
        struct SlowNotice<'a> {
            directory: &'a Path,
            elapsed: &'a Cell<Duration>,
            events: &'a RefCell<Vec<&'static str>>,
        }
        impl ExitEffects for SlowNotice<'_> {
            fn suspend_watchdog(&mut self) {}
            fn record(&mut self, _: &FailureReport) {}
            fn terminal(&mut self, _: miv_startup::Outcome) {}
            fn finish_logs(&mut self, _: Instant) {
                self.events.borrow_mut().push("logs saved");
            }
            fn show(&mut self, _: &str) {
                assert_eq!(
                    test_support::terminal(self.directory),
                    Some(GuiRunOutcome::ReturnedError)
                );
                assert_eq!(*self.events.borrow(), ["logs saved", "terminal saved"]);
                // Simulate reading the native dialog for longer than the entire
                // shutdown budget. No real UI or wall-clock sleep is involved.
                self.elapsed
                    .set(self.elapsed.get() + Duration::from_secs(3));
                self.events.borrow_mut().push("notification returned");
            }
        }
        let mut effects = SlowNotice {
            directory: directory.path(),
            elapsed: &elapsed,
            events: &events,
        };
        let prepared = super::finish_run_native(
            Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
                "injected failure",
            )))),
            false,
            directory.path(),
            || {},
            &mut effects,
        );
        assert!(
            events.borrow().is_empty(),
            "run_native must not wait or notify"
        );
        let result = finish_at_main_until(
            prepared,
            &mut effects,
            |ok, deadline| {
                assert!(!ok);
                assert_eq!(elapsed.get(), Duration::ZERO);
                assert!(owner.finish_until(GuiRunOutcome::ReturnedError, deadline));
                events.borrow_mut().push("terminal saved");
            },
            Instant::now() + Duration::from_secs(2),
        );
        assert!(result.is_err());
        assert!(elapsed.get() > Duration::from_secs(2));
        drop(owner);
        let mut next = GuiRunDiagnostics::new();
        test_support::admit(&mut next, directory.path());
        assert!(next.report_handle().unwrap().take_report().is_none());
        assert!(next.finish_until(
            GuiRunOutcome::ReturnedOk,
            Instant::now() + Duration::from_secs(2)
        ));
    }

    #[cfg(all(windows, not(feature = "portable")))]
    #[test]
    fn runtime_lease_remains_held_through_main_notification() {
        let directory = tempfile::tempdir().unwrap();
        let version = directory.path().join("runtime/4.10.0");
        std::fs::create_dir_all(&version).unwrap();
        let mut session = crate::GuiRunSession {
            _runtime_lease: Some(crate::runtime_locks::shared(&version).unwrap()),
            ..Default::default()
        };
        struct PinnedNotice<'a>(&'a Path);
        impl ExitEffects for PinnedNotice<'_> {
            fn suspend_watchdog(&mut self) {}
            fn record(&mut self, _: &FailureReport) {}
            fn terminal(&mut self, _: miv_startup::Outcome) {}
            fn finish_logs(&mut self, _: std::time::Instant) {}
            fn show(&mut self, _: &str) {
                assert!(
                    crate::runtime_locks::exclusive(self.0, crate::runtime_locks::IN_USE).is_err()
                );
            }
        }
        let prepared = PreparedRunExit {
            result: Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
                "fixture",
            )))),
            notification: Some("fixture".into()),
        };
        assert!(
            finish_at_main_until(
                prepared,
                &mut PinnedNotice(&version),
                |ok, deadline| session.finish_until(ok, deadline),
                std::time::Instant::now() + std::time::Duration::from_secs(2),
            )
            .is_err()
        );
        drop(session);
        assert!(crate::runtime_locks::exclusive(&version, crate::runtime_locks::IN_USE).is_ok());
    }

    #[derive(Debug)]
    struct NestedError(&'static str, Option<Box<NestedError>>);
    impl std::fmt::Display for NestedError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl Error for NestedError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.1.as_deref().map(|source| source as &dyn Error)
        }
    }

    #[test]
    fn app_creation_keeps_the_deepest_source_without_gpu_advice() {
        let error = eframe::Error::AppCreation(Box::new(NestedError(
            "outer",
            Some(Box::new(NestedError(
                "middle",
                Some(Box::new(NestedError("deepest cause", None))),
            ))),
        )));
        let report = run_native_failure_report(&error, "C:\\app\\logs");
        assert_eq!(report.kind, "AppCreation");
        assert!(
            report
                .log_detail
                .contains("outer -> middle -> deepest cause")
        );
        assert!(
            report
                .dialog_body
                .starts_with("mImageViewer の実行中に問題が起きたため、終了します。")
        );
        assert!(!report.dialog_body.contains("ドライバー"));
        assert!(report.dialog_body.contains("C:\\app\\logs"));
    }

    #[test]
    fn representative_rendering_variants_include_driver_advice_and_detail() {
        let mut cases = vec![
            (
                egui_wgpu::WgpuError::CustomNativeAdapterSelectionError(
                    "adapter unavailable".into(),
                ),
                "Wgpu.CustomNativeAdapterSelectionError",
                "adapter unavailable",
            ),
            (
                egui_wgpu::WgpuError::NoSurfaceFormatsAvailable,
                "Wgpu.NoSurfaceFormatsAvailable",
                "no valid format",
            ),
            (
                egui_wgpu::WgpuError::HandleError(eframe::wgpu::rwh::HandleError::Unavailable),
                "Wgpu.HandleError",
                "not available",
            ),
        ];
        #[cfg(windows)]
        cases.push((
            egui_wgpu::WgpuError::WindowSizeLimit(std::io::Error::other("window size denied")),
            "Wgpu.WindowSizeLimit",
            "window size denied",
        ));
        for (error, kind, detail) in cases {
            let report = run_native_failure_report(&eframe::Error::Wgpu(error), "C:\\app\\logs");
            assert_eq!(report.kind, kind);
            assert!(report.log_detail.contains(detail), "{}", report.log_detail);
            assert!(
                report
                    .dialog_body
                    .starts_with("画面の表示処理に失敗したため、mImageViewer を終了します。")
            );
            assert!(
                report
                    .dialog_body
                    .contains("グラフィックスドライバーを最新版に更新")
            );
        }
    }

    #[test]
    fn event_loop_and_other_errors_do_not_blame_the_gpu() {
        for (error, kind) in [
            (
                eframe::Error::WinitEventLoop(winit::error::EventLoopError::ExitFailure(42)),
                "WinitEventLoop",
            ),
            (
                eframe::Error::OpenGL("painter failed".to_owned().into()),
                "OpenGL",
            ),
        ] {
            let report = run_native_failure_report(&error, "C:\\app\\logs");
            assert_eq!(report.kind, kind);
            assert!(
                report
                    .dialog_body
                    .starts_with("mImageViewer の実行中に問題が起きたため、終了します。")
            );
            assert!(!report.dialog_body.contains("ドライバー"));
            assert!(report.log_detail.contains(&error.to_string()));
        }
    }

    #[test]
    fn only_dialog_detail_is_shortened_and_made_single_line() {
        let long = format!("\n\0{}tail detail", "長い理由".repeat(100));
        let error = eframe::Error::AppCreation(Box::new(std::io::Error::other(long.clone())));
        let report = run_native_failure_report(&error, "C:\\app\\logs");
        assert!(report.log_detail.contains(&long));
        let detail = report.dialog_body.split("詳細: ").nth(1).unwrap();
        assert!(detail.ends_with("..."));
        assert!(!detail.contains('\n'));
        assert!(!detail.contains('\0'));
        assert!(detail.chars().count() <= "AppCreation: ".len() + 203);
    }

    #[test]
    fn normal_portable_and_relative_data_dirs_produce_absolute_log_paths() {
        let temp = tempfile::tempdir().unwrap();
        for data in [
            temp.path().join("Roaming/mimageviewer"),
            temp.path().join("portable/data"),
            PathBuf::from("target").join(temp.path().file_name().unwrap()),
        ] {
            let path = absolute_logs_dir(&data).unwrap();
            assert!(path.is_absolute(), "{}", path.display());
            assert_eq!(path.file_name().unwrap(), "logs");
            assert!(
                !path.exists(),
                "the helper must not need/create the logs folder"
            );
            let report = run_native_failure_report(
                &eframe::Error::Wgpu(egui_wgpu::WgpuError::NoSurfaceFormatsAvailable),
                &path.display().to_string(),
            );
            assert!(report.dialog_body.contains(&path.display().to_string()));
        }
    }

    #[test]
    fn native_dialog_title_matches_the_build_flavor() {
        assert_eq!(
            crate::data_dir::FATAL_DIALOG_TITLE,
            if cfg!(feature = "portable") {
                "mImageViewer (ポータブル版)"
            } else {
                "mImageViewer"
            }
        );
    }
}
