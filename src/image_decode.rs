//! Shared primary -> WIC boundary. AVIF selection lives here, including fakes.
//! Susie admission/priority remains with each existing caller.
use crate::avif_decoder::Workload;
use crate::stats::DecodeSource;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy)]
pub enum Source<'a> {
    Path(&'a Path),
    Bytes { bytes: &'a [u8], hint: &'a str },
}
impl Source<'_> {
    pub fn extension(&self) -> &str {
        let path = match self {
            Self::Path(path) => path,
            Self::Bytes { hint, .. } => Path::new(hint),
        };
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    }

    fn is_avif(&self) -> bool {
        self.extension().eq_ignore_ascii_case("avif")
            // Embedded artwork has no filename. Inspect its already-owned header
            // only when the caller has no extension; never reread a file here.
            || (self.extension().is_empty()
                && matches!(self, Self::Bytes { bytes, .. }
                    if image::guess_format(bytes).ok() == Some(image::ImageFormat::Avif)))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    NeedsExif,
    Applied,
}
pub struct Decoded {
    pub image: image::DynamicImage,
    pub orientation: Orientation,
    pub decoder: DecodeSource,
}
impl Decoded {
    pub fn legacy(image: image::DynamicImage, decoder: DecodeSource) -> Self {
        Self {
            image,
            orientation: Orientation::NeedsExif,
            decoder,
        }
    }
    pub fn oriented(self, source: Source<'_>) -> image::DynamicImage {
        if self.orientation == Orientation::Applied {
            return self.image;
        }
        match source {
            Source::Path(path) => crate::thumb_loader::apply_exif_orientation(self.image, path),
            Source::Bytes { bytes, .. } => {
                crate::thumb_loader::apply_exif_orientation_from_bytes(self.image, bytes)
            }
        }
    }
}

pub(crate) trait Backend {
    fn avif(
        &self,
        source: Source<'_>,
        workload: Workload,
        budget: Option<u64>,
    ) -> Result<image::DynamicImage, crate::avif_decoder::Error>;
    fn wic(&self, source: Source<'_>, budget: Option<u64>) -> Option<image::DynamicImage>;
}
pub(crate) struct System;
impl Backend for System {
    fn avif(
        &self,
        source: Source<'_>,
        workload: Workload,
        budget: Option<u64>,
    ) -> Result<image::DynamicImage, crate::avif_decoder::Error> {
        match source {
            Source::Path(path) => {
                crate::avif_decoder::decode_path_bounded(path, workload, budget.unwrap_or(u64::MAX))
            }
            Source::Bytes { bytes, .. } => crate::avif_decoder::decode_bytes_bounded(
                bytes,
                workload,
                budget.unwrap_or(u64::MAX),
            ),
        }
    }
    fn wic(&self, source: Source<'_>, budget: Option<u64>) -> Option<image::DynamicImage> {
        match (source, budget) {
            (Source::Path(path), None) => crate::wic_decoder::decode_to_dynamic_image(path),
            (Source::Bytes { bytes, .. }, Some(budget)) => {
                crate::wic_decoder::decode_to_dynamic_image_from_bytes_bounded(
                    bytes,
                    source.extension(),
                    budget,
                )
            }
            (Source::Bytes { bytes, .. }, None) => {
                crate::wic_decoder::decode_to_dynamic_image_from_bytes(bytes, source.extension())
            }
            (Source::Path(path), Some(budget)) => std::fs::read(path).ok().and_then(|bytes| {
                crate::wic_decoder::decode_to_dynamic_image_from_bytes_bounded(
                    &bytes,
                    source.extension(),
                    budget,
                )
            }),
        }
    }
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}
fn cancel_error() -> image::ImageError {
    image::ImageError::IoError(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "decode cancelled",
    ))
}
pub(crate) fn primary_with_backend(
    source: Source<'_>,
    workload: Workload,
    budget: Option<u64>,
    cancel: Option<&AtomicBool>,
    primary: impl FnOnce() -> image::ImageResult<image::DynamicImage>,
    backend: &impl Backend,
) -> image::ImageResult<Decoded> {
    if cancelled(cancel) {
        return Err(cancel_error());
    }
    let avif = source.is_avif();
    let primary = if avif {
        backend
            .avif(source, workload, budget)
            .map(|image| Decoded {
                image,
                orientation: Orientation::Applied,
                decoder: DecodeSource::Libavif,
            })
            .map_err(|e| image::ImageError::IoError(std::io::Error::other(e)))
    } else {
        primary().map(|image| Decoded::legacy(image, DecodeSource::Native))
    };
    if cancelled(cancel) {
        return Err(cancel_error());
    }
    let error = match primary {
        Ok(image) => return Ok(image),
        Err(e) => e,
    };
    // Bounded primary raster admission is terminal. Corrupt pixel errors were
    // terminal in now-playing too; only unsupported decoders can delegate.
    if budget.is_some()
        && !avif
        && !matches!(
            &error,
            image::ImageError::Unsupported(_) | image::ImageError::IoError(_)
        )
    {
        return Err(error);
    }
    let started = std::time::Instant::now();
    let wic = backend.wic(source, budget);
    record_backend("WIC", started, wic.is_some());
    if cancelled(cancel) {
        return Err(cancel_error());
    }
    wic.map(|image| Decoded {
        image,
        decoder: DecodeSource::Wic,
        orientation: if budget.is_some() {
            Orientation::Applied
        } else {
            Orientation::NeedsExif
        },
    })
    .ok_or(error)
}

pub fn primary(
    source: Source<'_>,
    workload: Workload,
    budget: Option<u64>,
    cancel: Option<&AtomicBool>,
    primary: impl FnOnce() -> image::ImageResult<image::DynamicImage>,
) -> image::ImageResult<Decoded> {
    primary_with_backend(source, workload, budget, cancel, primary, &System)
}
pub fn susie(
    source: Source<'_>,
    priority: bool,
    cancel: Option<Arc<AtomicBool>>,
) -> std::io::Result<Decoded> {
    let started = std::time::Instant::now();
    let result = match source {
        Source::Path(path) => crate::susie_loader::decode_file(path, priority, cancel),
        Source::Bytes { bytes, hint } => {
            crate::susie_loader::decode_bytes(hint, bytes, priority, cancel)
        }
    };
    record_backend("Susie", started, result.is_ok());
    result.map(|image| Decoded::legacy(image, DecodeSource::Susie))
}
pub(crate) fn record_backend(decoder: &str, started: std::time::Instant, success: bool) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            "decode",
            "backend",
            None,
            0,
            &[
                ("decoder", decoder.into()),
                ("ms", (started.elapsed().as_secs_f64() * 1000.0).into()),
                ("stage", if success { "complete" } else { "decode" }.into()),
                ("success", success.into()),
            ],
        );
    }
}

/// Ordinary full-raster callers; thumbnail JPEG DCT callers retain their fast path.
pub fn decode(
    source: Source<'_>,
    workload: Workload,
    priority: bool,
    cancel: Option<&Arc<AtomicBool>>,
) -> image::ImageResult<Decoded> {
    decode_with_susie_cancel(source, workload, priority, cancel, cancel.cloned())
}

/// The outer request can be cancelled while an admitted Susie IPC must finish.
/// Preserve the caller's existing distinction between those two cancellation scopes.
pub fn decode_with_susie_cancel(
    source: Source<'_>,
    workload: Workload,
    priority: bool,
    cancel: Option<&Arc<AtomicBool>>,
    susie_cancel: Option<Arc<AtomicBool>>,
) -> image::ImageResult<Decoded> {
    primary(
        source,
        workload,
        None,
        cancel.map(Arc::as_ref),
        || match source {
            Source::Path(path) => image::open(path),
            Source::Bytes { bytes, .. } => image::load_from_memory(bytes),
        },
    )
    .or_else(|primary| {
        if cancelled(cancel.map(Arc::as_ref)) {
            return Err(cancel_error());
        }
        let result = susie(source, priority, susie_cancel).map_err(|_| primary);
        if cancelled(cancel.map(Arc::as_ref)) {
            return Err(cancel_error());
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    struct CancelBackend<'a> {
        cancel: &'a AtomicBool,
        avif_calls: AtomicUsize,
        wic_calls: AtomicUsize,
    }
    impl Backend for CancelBackend<'_> {
        fn avif(
            &self,
            _: Source<'_>,
            _: Workload,
            _: Option<u64>,
        ) -> Result<image::DynamicImage, crate::avif_decoder::Error> {
            self.avif_calls.fetch_add(1, Ordering::Relaxed);
            self.cancel.store(true, Ordering::Relaxed);
            Ok(image::DynamicImage::new_rgba8(1, 1))
        }
        fn wic(&self, _: Source<'_>, _: Option<u64>) -> Option<image::DynamicImage> {
            self.wic_calls.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
    #[test]
    fn avif_cancellation_is_checked_before_and_after_the_native_call() {
        for initially_cancelled in [true, false] {
            let cancel = AtomicBool::new(initially_cancelled);
            let backend = CancelBackend {
                cancel: &cancel,
                avif_calls: AtomicUsize::new(0),
                wic_calls: AtomicUsize::new(0),
            };
            let result = primary_with_backend(
                Source::Bytes {
                    bytes: b"",
                    hint: "page.avif",
                },
                Workload::Background,
                None,
                Some(&cancel),
                || panic!("image crate must not handle AVIF"),
                &backend,
            );
            assert!(
                matches!(result, Err(image::ImageError::IoError(error)) if error.kind() == std::io::ErrorKind::Interrupted)
            );
            assert_eq!(
                backend.avif_calls.load(Ordering::Relaxed),
                usize::from(!initially_cancelled)
            );
            assert_eq!(backend.wic_calls.load(Ordering::Relaxed), 0);
        }
    }
}
