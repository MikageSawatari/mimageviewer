pub mod executor;
pub mod raw_decoder;

pub use executor::{RawDevelopExecutor, RawPriority, RawTicket};
pub use raw_decoder::{
    RawBrightness, RawDevelopScale, RawDevelopSupport, RawError, RawInfo, RawOwnedSource,
    RawPreview, RawPreviewInfo, RawPreviewUnavailableReason, RawSource, RawUnsupportedReason,
};

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use std::path::PathBuf;
    use std::sync::{atomic::Ordering, mpsc};
    use std::time::Duration;

    #[cfg(windows)]
    fn samples() -> Vec<serde_json::Value> {
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read("tests/raw-samples.json").unwrap()).unwrap();
        let samples = manifest["samples"].as_array().unwrap().clone();
        for sample in &samples {
            let path = PathBuf::from("vendor/raw-samples").join(sample["file"].as_str().unwrap());
            assert!(
                path.is_file(),
                "Missing {}; run .\\scripts\\setup-raw-samples.ps1",
                path.display()
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().len(),
                sample["size"].as_u64().unwrap(),
                "Wrong sample size: {}. Rerun .\\scripts\\setup-raw-samples.ps1",
                path.display()
            );
        }
        samples
    }

    #[cfg(windows)]
    #[test]
    fn all_cc0_samples_info_develop_and_preview_orientation() {
        let executor = RawDevelopExecutor::new(3).unwrap();
        for sample in samples() {
            let path = PathBuf::from("vendor/raw-samples").join(sample["file"].as_str().unwrap());
            let info = raw_decoder::info(RawSource::Path(&path))
                .unwrap_or_else(|error| panic!("{} info: {error}", path.display()));
            let supported = matches!(info.develop_support, RawDevelopSupport::Supported);
            assert_eq!(
                supported,
                sample["expected"]["develop_supported"].as_bool().unwrap(),
                "{} develop support",
                path.display()
            );
            if let Some(flip) = sample["expected"]["flip"].as_u64() {
                assert_eq!(u64::from(info.flip), flip, "{} flip", path.display());
            }
            let expected_preview = sample["expected"]["has_usable_preview"]
                .as_bool()
                .expect("manifest preview expectation");
            match raw_decoder::preview(RawSource::Path(&path)) {
                Ok(preview) => {
                    assert!(expected_preview, "{} unexpected preview", path.display());
                    let [width, height] = preview.info.dims;
                    let flip = preview
                        .info
                        .tflip
                        .filter(|&flip| flip != 0)
                        .unwrap_or(info.flip);
                    let expected_dims = if flip & 4 != 0 {
                        (height, width)
                    } else {
                        (width, height)
                    };
                    assert_eq!(
                        preview.image.dimensions(),
                        expected_dims,
                        "{} preview orientation",
                        path.display()
                    );
                    if sample["expected"]["portrait_preview"].as_bool() == Some(true) {
                        assert!(
                            preview.image.height() > preview.image.width(),
                            "{} should have a portrait preview",
                            path.display()
                        );
                    }
                }
                Err(RawError::NoUsablePreview(_)) => {
                    assert!(!expected_preview, "{} missing preview", path.display());
                }
                Err(error) => panic!("{} preview: {error}", path.display()),
            }
            if !supported {
                let (send, receive) = mpsc::channel();
                let _ticket = executor.submit(
                    RawOwnedSource::Path(path.clone()),
                    RawDevelopScale::Full,
                    RawBrightness::Auto001,
                    RawPriority::High,
                    send,
                );
                let actual = receive.recv_timeout(Duration::from_secs(30));
                assert!(
                    matches!(
                        &actual,
                        Ok(Err(RawError::Unsupported(RawUnsupportedReason::Decoder)))
                    ),
                    "{} unsupported develop: {actual:?}",
                    path.display()
                );
                continue;
            }
            let (send, receive) = mpsc::channel();
            let _ticket = executor.submit(
                RawOwnedSource::Path(path.clone()),
                RawDevelopScale::Full,
                RawBrightness::Auto001,
                RawPriority::High,
                send,
            );
            let image = receive
                .recv_timeout(Duration::from_secs(180))
                .unwrap()
                .unwrap_or_else(|error| panic!("{} develop: {error}", path.display()));
            assert_eq!(
                [image.width(), image.height()],
                info.developed_dims,
                "{} developed dimensions",
                path.display()
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn callback_cancel_and_truncated_file_are_typed() {
        let _ = samples();
        let executor = RawDevelopExecutor::new(1).unwrap();
        let path = PathBuf::from("vendor/raw-samples/3914.raf");
        let (send, receive) = mpsc::channel();
        let ticket = executor.submit(
            RawOwnedSource::Path(path),
            RawDevelopScale::Full,
            RawBrightness::Auto001,
            RawPriority::High,
            send,
        );
        let progress = ticket.progress();
        loop {
            if progress.load(Ordering::Acquire) >= 5 {
                break;
            }
            assert!(
                matches!(
                    receive.recv_timeout(Duration::from_millis(10)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ),
                "RAW finished before cancel point"
            );
        }
        ticket.cancel();
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(180)),
            Ok(Err(RawError::Cancelled))
        ));
        let source = std::fs::read("vendor/raw-samples/1023.dng").unwrap();
        assert!(matches!(
            raw_decoder::info(RawSource::Bytes(&source[..1024])),
            Err(RawError::Corrupt(_)) | Err(RawError::Io(_))
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_stub() {
        assert!(matches!(
            raw_decoder::info(RawSource::Bytes(b"raw")),
            Err(RawError::Unsupported(RawUnsupportedReason::Platform))
        ));
    }
}
