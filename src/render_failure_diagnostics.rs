//! Root renderer failure observation. Error ownership stays intact until the
//! existing detailed panic hook renders it; the small sink receives only enums.

use crate::previous_run_diagnostics::checkpoint::CheckpointPublisher;
use crate::previous_run_diagnostics::fatal::PanicKind;
use std::any::Any;
use std::sync::Arc;

/// A typed payload avoids guessing GPU failures from arbitrary panic text.
/// Keep the original error so the existing local detailed log loses no context.
pub(crate) struct RenderPanic {
    pub(crate) error: wgpu::Error,
}

/// This hook-entry path performs no allocation, formatting, or string scanning.
pub(crate) fn classify_panic(payload: &(dyn Any + Send)) -> PanicKind {
    match payload.downcast_ref::<RenderPanic>() {
        Some(RenderPanic {
            error: wgpu::Error::OutOfMemory { .. },
        }) => PanicKind::GraphicsOutOfMemory,
        Some(RenderPanic {
            error: wgpu::Error::Validation { .. },
        }) => PanicKind::GraphicsValidation,
        Some(RenderPanic {
            error: wgpu::Error::Internal { .. },
        }) => PanicKind::GraphicsInternal,
        None => PanicKind::Rust,
    }
}

/// Runs at the successful root device boundary, before renderer allocation.
/// Excluded modes retain fatal error handling, with their small sink unprepared.
pub(crate) fn install_root_device(device: &wgpu::Device, publisher: Option<&CheckpointPublisher>) {
    device.on_uncaptured_error(Arc::new(panic_uncaptured_error));
    if let Some(latch) = publisher.and_then(CheckpointPublisher::register_device_loss_source) {
        // Capture only this immutable device's latch. No publisher/device cycle,
        // callback I/O, message processing, clock sampling, or wgpu reentry.
        device.set_device_lost_callback(move |reason, _message| latch.observe(reason));
    }
}

fn panic_uncaptured_error(error: wgpu::Error) {
    // Preserve wgpu's fatal contract. Formatting belongs after the small record
    // in the panic hook, never before it in the uncaptured-error callback.
    std::panic::panic_any(RenderPanic { error });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD_DIR: &str = "MIV_RENDER_PANIC_CHILD_DIR";
    const CHILD_KIND: &str = "MIV_RENDER_PANIC_CHILD_KIND";
    const PRIVATE_TEXT: &str = r"PRIVATE_RENDER_SENTINEL C:\private\book.zip\secret.png";

    struct NoFormatting;
    impl std::fmt::Debug for NoFormatting {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("classification must not call arbitrary Debug")
        }
    }
    impl std::fmt::Display for NoFormatting {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("classification must not call arbitrary Display")
        }
    }
    impl std::error::Error for NoFormatting {}

    #[test]
    fn diagnostic_panic_classifier_uses_only_typed_error_variants() {
        for (error, kind) in [
            (
                wgpu::Error::OutOfMemory {
                    source: Box::new(NoFormatting),
                },
                PanicKind::GraphicsOutOfMemory,
            ),
            (
                wgpu::Error::Validation {
                    source: Box::new(NoFormatting),
                    description: PRIVATE_TEXT.to_owned(),
                },
                PanicKind::GraphicsValidation,
            ),
            (
                wgpu::Error::Internal {
                    source: Box::new(NoFormatting),
                    description: PRIVATE_TEXT.to_owned(),
                },
                PanicKind::GraphicsInternal,
            ),
        ] {
            assert_eq!(classify_panic(&RenderPanic { error }), kind);
        }
        assert_eq!(classify_panic(&PRIVATE_TEXT), PanicKind::Rust);
        assert_eq!(
            classify_panic(&format!("Out of Memory {PRIVATE_TEXT}")),
            PanicKind::Rust
        );
        assert_eq!(classify_panic(&NoFormatting), PanicKind::Rust);
        // An arbitrary Error payload did not originate at our typed boundary.
        assert_eq!(
            classify_panic(&wgpu::Error::OutOfMemory {
                source: Box::new(NoFormatting),
            }),
            PanicKind::Rust
        );
    }

    /// Only this libtest case runs in the child. No product/UI/GPU/ORT startup.
    #[test]
    fn actual_diagnostic_panic_hook_child() {
        let Some(directory) = std::env::var_os(CHILD_DIR) else {
            return;
        };
        let kind = std::env::var(CHILD_KIND).unwrap();
        assert!(matches!(kind.as_str(), "rust" | "validation"));
        let _lock = crate::data_dir::test_override_lock();
        crate::data_dir::set_test_override(Some(directory.clone().into()));
        crate::previous_run_diagnostics::test_support::prepare_panic_sink(Path::new(&directory));
        crate::install_panic_log_hook();
        if kind == "validation" {
            panic_uncaptured_error(wgpu::Error::Validation {
                source: Box::new(std::io::Error::other("injected validation source")),
                description: PRIVATE_TEXT.to_owned(),
            });
        } else {
            panic!("{PRIVATE_TEXT}");
        }
    }

    #[test]
    fn diagnostic_real_panic_hook_keeps_private_details_only_in_existing_log() {
        for (kind, expected) in [
            ("rust", PanicKind::Rust),
            ("validation", PanicKind::GraphicsValidation),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "render_failure_diagnostics::tests::actual_diagnostic_panic_hook_child",
                    "--nocapture",
                ])
                .env(CHILD_DIR, directory.path())
                .env(CHILD_KIND, kind)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id();
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("limited panic-hook libtest child timed out: {kind}");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(!status.success(), "the actual libtest case must panic");
            assert_eq!(
                crate::previous_run_diagnostics::test_support::panic_kinds(directory.path(), pid),
                vec![expected]
            );
            let small = std::fs::read(directory.path().join("panic-events.log")).unwrap();
            let small = String::from_utf8_lossy(&small);
            assert!(!small.contains("PRIVATE_RENDER_SENTINEL"));
            assert!(!small.contains("private"));
            assert!(!small.contains("secret.png"));
            let detailed =
                std::fs::read_to_string(directory.path().join("logs/panic.log")).unwrap();
            assert!(detailed.contains(PRIVATE_TEXT));
            if kind == "validation" {
                assert!(detailed.contains(&format!("wgpu error: {PRIVATE_TEXT}\n")));
            }
        }
    }
}
