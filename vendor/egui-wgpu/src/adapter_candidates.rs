//! Ordered startup device requests. No later rendering error retries a candidate.

use crate::{StartupDiagnosticEvent, StartupGpuStage, WgpuConfiguration};

/// An absent candidate callback preserves the single-selector path. Keep both
/// callbacks lazy: selecting candidates must never also invoke the selector.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn select_candidates<A, E>(
    candidates: Option<impl FnOnce() -> Result<Vec<A>, E>>,
    selector: Option<impl FnOnce() -> Result<A, E>>,
) -> Option<Result<Vec<A>, E>> {
    if let Some(candidates) = candidates {
        Some(candidates())
    } else {
        selector.map(|selector| selector().map(|adapter| vec![adapter]))
    }
}

/// Inject the native request so ordering, errors and observations are testable
/// without a GPU. The retained adapter and its request clone refer to the same
/// wgpu adapter. Empty candidates produce no request or device-stage event.
pub(crate) async fn request_device_candidates<
    A: Clone,
    D,
    E: std::fmt::Display,
    F: Future<Output = Result<D, E>>,
>(
    candidates: Vec<A>,
    config: &WgpuConfiguration,
    adapter_info: impl Fn(&A) -> wgpu::AdapterInfo,
    mut request: impl FnMut(A) -> F,
) -> Result<Option<(A, D)>, E> {
    let mut last_error = None;
    for (index, adapter) in candidates.into_iter().enumerate() {
        let info = config
            .startup_diagnostics
            .as_ref()
            .map(|_| adapter_info(&adapter));
        if let Some(info) = &info {
            config.startup_event(StartupDiagnosticEvent::AdapterSelected(info.clone()));
        }
        config.startup_begin(StartupGpuStage::Device);
        let result = request(adapter.clone()).await;
        config.startup_end(StartupGpuStage::Device, result.is_ok());
        if let Some(info) = info {
            config.startup_event(StartupDiagnosticEvent::DeviceAttempt {
                attempt: index + 1,
                info,
                result: result.as_ref().map(|_| ()).map_err(ToString::to_string),
            });
        }
        match result {
            Ok(device) => return Ok(Some((adapter, device))),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::{
        cell::Cell,
        sync::{Arc, Mutex},
    };

    use super::*;

    type Events = Arc<Mutex<Vec<StartupDiagnosticEvent>>>;

    fn observed_config() -> (WgpuConfiguration, Events) {
        let events = Events::default();
        let recorded = Arc::clone(&events);
        let config = WgpuConfiguration {
            startup_diagnostics: Some(Arc::new(move |event| {
                recorded.lock().unwrap().push(event);
            })),
            ..Default::default()
        };
        (config, events)
    }

    fn info(backend: &wgpu::Backend) -> wgpu::AdapterInfo {
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, *backend);
        info.name = format!("fake {backend:?}");
        info.driver = "test driver".to_owned();
        info
    }

    fn assert_attempt(
        events: &[StartupDiagnosticEvent],
        attempt: usize,
        backend: wgpu::Backend,
        expected_result: Result<(), &str>,
    ) {
        assert_eq!(events.len(), 4);
        match &events[0] {
            StartupDiagnosticEvent::AdapterSelected(selected) => {
                assert_eq!(*selected, info(&backend))
            }
            event => panic!("expected AdapterSelected, got {event:?}"),
        }
        assert!(matches!(
            events[1],
            StartupDiagnosticEvent::Begin(StartupGpuStage::Device)
        ));
        assert!(
            matches!(events[2], StartupDiagnosticEvent::End { stage: StartupGpuStage::Device, success } if success == expected_result.is_ok())
        );
        match &events[3] {
            StartupDiagnosticEvent::DeviceAttempt {
                attempt: actual,
                info: actual_info,
                result,
            } => {
                assert_eq!(*actual, attempt);
                assert_eq!(*actual_info, info(&backend));
                assert_eq!(
                    result.as_ref().map(|_| ()).map_err(String::as_str),
                    expected_result
                );
            }
            event => panic!("expected DeviceAttempt, got {event:?}"),
        }
    }

    #[test]
    fn first_success_does_not_request_next_backend() {
        let (config, events) = observed_config();
        let mut calls = Vec::new();
        let result = pollster::block_on(request_device_candidates(
            vec![wgpu::Backend::Dx12, wgpu::Backend::Vulkan],
            &config,
            info,
            |backend| {
                calls.push(backend);
                std::future::ready(Ok::<_, String>("device"))
            },
        ));
        assert_eq!(result, Ok(Some((wgpu::Backend::Dx12, "device"))));
        assert_eq!(calls, [wgpu::Backend::Dx12]);
        assert_attempt(&events.lock().unwrap(), 1, wgpu::Backend::Dx12, Ok(()));
    }

    #[test]
    fn failed_request_is_recorded_before_next_request_and_retained_on_success() {
        let (config, events) = observed_config();
        let mut calls = Vec::new();
        let result = pollster::block_on(request_device_candidates(
            vec![wgpu::Backend::Dx12, wgpu::Backend::Vulkan],
            &config,
            info,
            |backend| {
                calls.push(backend);
                if backend == wgpu::Backend::Dx12 {
                    std::future::ready(Err("DX12 failed"))
                } else {
                    // The next request starts only after the first result was published.
                    assert_attempt(
                        &events.lock().unwrap()[..4],
                        1,
                        wgpu::Backend::Dx12,
                        Err("DX12 failed"),
                    );
                    std::future::ready(Ok("vulkan device"))
                }
            },
        ));
        assert_eq!(result, Ok(Some((wgpu::Backend::Vulkan, "vulkan device"))));
        assert_eq!(calls, [wgpu::Backend::Dx12, wgpu::Backend::Vulkan]);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 8);
        assert_attempt(&events[..4], 1, wgpu::Backend::Dx12, Err("DX12 failed"));
        assert_attempt(&events[4..], 2, wgpu::Backend::Vulkan, Ok(()));
    }

    #[test]
    fn all_requests_fail_with_the_last_original_error() {
        let (config, events) = observed_config();
        let result = pollster::block_on(request_device_candidates(
            vec![wgpu::Backend::Dx12, wgpu::Backend::Vulkan],
            &config,
            info,
            |backend| {
                std::future::ready(Err::<(), _>(if backend == wgpu::Backend::Dx12 {
                    "DX12 failed"
                } else {
                    "Vulkan failed"
                }))
            },
        ));
        assert_eq!(result, Err("Vulkan failed"));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 8);
        assert_attempt(&events[..4], 1, wgpu::Backend::Dx12, Err("DX12 failed"));
        assert_attempt(&events[4..], 2, wgpu::Backend::Vulkan, Err("Vulkan failed"));
    }

    #[test]
    fn empty_candidates_never_request_a_device() {
        let (config, events) = observed_config();
        let result = pollster::block_on(request_device_candidates(
            Vec::<wgpu::Backend>::new(),
            &config,
            |_| panic!("empty list has no adapter"),
            |_| -> std::future::Ready<Result<(), String>> { panic!("empty list must not request") },
        ));
        assert_eq!(result, Ok(None));
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn absent_candidates_keep_one_selector_and_one_request() {
        let selected = Cell::new(0);
        let requested = Cell::new(0);
        let candidates = select_candidates(
            None::<fn() -> Result<Vec<wgpu::Backend>, String>>,
            Some(|| {
                selected.set(selected.get() + 1);
                Ok(wgpu::Backend::Dx12)
            }),
        )
        .unwrap()
        .unwrap();
        let (config, events) = observed_config();
        let result =
            pollster::block_on(request_device_candidates(candidates, &config, info, |_| {
                requested.set(requested.get() + 1);
                std::future::ready(Err::<(), _>("single request failed"))
            }));
        assert_eq!(result, Err("single request failed"));
        assert_eq!(selected.get(), 1);
        assert_eq!(requested.get(), 1);
        assert_attempt(
            &events.lock().unwrap(),
            1,
            wgpu::Backend::Dx12,
            Err("single request failed"),
        );
    }

    #[test]
    fn candidate_callback_takes_precedence_even_when_empty_or_failed() {
        for result in [
            Ok(vec![wgpu::Backend::Vulkan]),
            Ok(Vec::new()),
            Err("selection failed"),
        ] {
            let expected = result.clone();
            assert_eq!(
                select_candidates(
                    Some(|| result),
                    Some(|| -> Result<_, &str> {
                        panic!("candidate callback replaces the selector")
                    })
                ),
                Some(expected)
            );
        }
        assert!(
            select_candidates::<(), String>(
                None::<fn() -> Result<Vec<()>, String>>,
                None::<fn() -> Result<(), String>>
            )
            .is_none()
        );
        assert_eq!(
            select_candidates(
                None::<fn() -> Result<Vec<()>, &'static str>>,
                Some(|| Err("selector failed"))
            ),
            Some(Err("selector failed"))
        );
    }

    #[test]
    fn absent_observer_does_not_fetch_adapter_info() {
        let result = pollster::block_on(request_device_candidates(
            vec![wgpu::Backend::Dx12],
            &WgpuConfiguration::default(),
            |_| panic!("adapter info is only needed for the observer"),
            |_| std::future::ready(Ok::<_, String>("device")),
        ));
        assert_eq!(result, Ok(Some((wgpu::Backend::Dx12, "device"))));
    }

    #[test]
    fn setup_defaults_clone_and_debug_preserve_optional_candidates() {
        let mut setup = crate::WgpuSetupCreateNew::without_display_handle();
        assert!(setup.native_adapter_candidates.is_none());
        setup.native_adapter_candidates = Some(Arc::new(|_, _| Ok(Vec::new())));
        setup.native_adapter_selector = Some(Arc::new(|_, _| Err("selector".to_owned())));
        let cloned = setup.clone();
        assert!(Arc::ptr_eq(
            setup.native_adapter_candidates.as_ref().unwrap(),
            cloned.native_adapter_candidates.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            setup.native_adapter_selector.as_ref().unwrap(),
            cloned.native_adapter_selector.as_ref().unwrap()
        ));
        assert!(format!("{cloned:?}").contains("native_adapter_candidates: true"));
    }
}
