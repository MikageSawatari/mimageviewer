//! Bounded dimension-only sampling for a representative jacket leaf.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::rotation_db::{Rotation, RotationDb};

pub(crate) const MAX_SAMPLE_PAGES: usize = 8;
pub(crate) const MAX_SAMPLE_PREFIX_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_RAR_SAMPLE_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_RAR_SAMPLE_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SampleAbort {
    Cancelled,
    ReadFailed(String),
}

impl std::fmt::Display for SampleAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::ReadFailed(reason) => write!(f, "{reason}"),
        }
    }
}

/// Preserve transient read failures until the caller decides whether to save.
#[derive(Debug)]
pub(crate) enum SampleRead {
    Read(Vec<u8>),
    Unreadable,
    Cancelled,
    ReadFailed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SampleFailureKind {
    Data,
    ReadFailed,
}

#[derive(Clone, Copy)]
pub(crate) enum SampleIoOrigin {
    Direct,
    ZipDecoder,
}

/// Preserve source provenance when a reader synthesizes an error without an
/// OS code. Real OS errors retain their original raw_os_error and payload.
#[derive(Debug)]
struct SourceReadError(std::io::Error);

impl std::fmt::Display for SourceReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for SourceReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub(crate) fn source_read_error(error: std::io::Error) -> std::io::Error {
    if error.raw_os_error().is_some() {
        error
    } else {
        std::io::Error::new(error.kind(), SourceReadError(error))
    }
}

pub(crate) fn classify_io_error_at(
    error: &std::io::Error,
    origin: SampleIoOrigin,
) -> SampleFailureKind {
    use std::io::ErrorKind;
    if error.raw_os_error().is_some()
        || error
            .get_ref()
            .is_some_and(|error| error.is::<SourceReadError>())
    {
        return SampleFailureKind::ReadFailed;
    }
    // ZIP exposes decoder/CRC/structure errors as io::Error too. In
    // particular flate2 uses InvalidInput for malformed Deflate streams.
    // Only the underlying reader's errors above denote source I/O failure.
    if matches!(origin, SampleIoOrigin::ZipDecoder) {
        return SampleFailureKind::Data;
    }
    match error.kind() {
        ErrorKind::InvalidData
        | ErrorKind::InvalidInput
        | ErrorKind::UnexpectedEof
        | ErrorKind::Unsupported => SampleFailureKind::Data,
        _ => SampleFailureKind::ReadFailed,
    }
}

pub(crate) fn classify_io_error(error: &std::io::Error) -> SampleFailureKind {
    classify_io_error_at(error, SampleIoOrigin::Direct)
}

pub(crate) fn classify_unrar_error(code: unrar::error::Code) -> SampleFailureKind {
    match code {
        unrar::error::Code::BadData
        | unrar::error::Code::BadArchive
        | unrar::error::Code::UnknownFormat
        | unrar::error::Code::BadPassword => SampleFailureKind::Data,
        _ => SampleFailureKind::ReadFailed,
    }
}

pub(crate) fn check_cancel(cancel: Option<&AtomicBool>) -> Result<(), SampleAbort> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        Err(SampleAbort::Cancelled)
    } else {
        Ok(())
    }
}

impl SampleRead {
    pub(crate) fn from_io(result: std::io::Result<Vec<u8>>, cancel: Option<&AtomicBool>) -> Self {
        Self::from_io_at(result, cancel, SampleIoOrigin::Direct)
    }

    pub(crate) fn from_io_at(
        result: std::io::Result<Vec<u8>>,
        cancel: Option<&AtomicBool>,
        origin: SampleIoOrigin,
    ) -> Self {
        if check_cancel(cancel).is_err() {
            return Self::Cancelled;
        }
        match result {
            Ok(bytes) => Self::Read(bytes),
            Err(error) => match classify_io_error_at(&error, origin) {
                SampleFailureKind::Data => Self::Unreadable,
                SampleFailureKind::ReadFailed => Self::ReadFailed(error.to_string()),
            },
        }
    }

    pub(crate) fn into_slot(self) -> Result<Option<Vec<u8>>, SampleAbort> {
        match self {
            Self::Read(bytes) => Ok(Some(bytes)),
            Self::Unreadable => Ok(None),
            Self::Cancelled => Err(SampleAbort::Cancelled),
            Self::ReadFailed(reason) => Err(SampleAbort::ReadFailed(reason)),
        }
    }
}

pub(crate) type SampleAspect = Result<Result<f64, PageAspectDecline>, SampleAbort>;

#[cfg(test)]
thread_local! {
    static CONTAINER_SAMPLE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn container_sample_calls_for_test() -> usize {
    CONTAINER_SAMPLE_CALLS.with(std::cell::Cell::get)
}

/// The list is metadata only. Selection is shared by ZIP and direct RAR and
/// does not change the released ZIP name ordering, scope, or attempt budget.
pub(crate) fn select_archive_samples(
    entries: impl IntoIterator<Item = (String, u64)>,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Vec<(String, u64)> {
    let prefix = prefix.map(|p| {
        let p = p.replace('\\', "/");
        if p.is_empty() {
            p
        } else {
            format!("{}/", p.trim_end_matches('/'))
        }
    });
    let mut names: Vec<_> = entries
        .into_iter()
        .filter(|(name, _)| {
            // Match ZIP's released lowercase_ext rule, including a basename
            // such as book/.png (Path::extension treats that name differently).
            let image = name.rfind('.').is_some_and(|dot| {
                dot >= name.rfind('/').map_or(0, |slash| slash + 1)
                    && crate::folder_tree::is_recognized_image_ext(
                        &name[dot + 1..].to_ascii_lowercase(),
                    )
            });
            !name.contains("__MACOSX/")
                && !name.starts_with('.')
                && !prefix
                    .as_ref()
                    .is_some_and(|prefix| !name.starts_with(prefix))
                && crate::zip_loader::split_nested_zip_path(name).len() == 1
                && image
        })
        .map(|(name, size)| {
            (
                crate::filename_sort::FileNameSortKey::new(&name),
                name,
                size,
            )
        })
        .collect();
    names.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let leaf = leaf_entry.replace('\\', "/");
    let Some(index) = names.iter().position(|(_, name, _)| name == &leaf) else {
        return Vec::new();
    };
    sample_indices(names.len(), index)
        .into_iter()
        .map(|index| (names[index].1.clone(), names[index].2))
        .collect()
}

#[derive(Default)]
pub(crate) struct SampleMetrics {
    pub(crate) selected: usize,
    pub(crate) read: usize,
    pub(crate) skipped_limit: usize,
    pub(crate) bytes: usize,
    pub(crate) list_ms: f64,
    pub(crate) read_ms: f64,
}

impl SampleMetrics {
    pub(crate) fn emit<T>(&self, path: &Path, format: &str, result: &Result<T, SampleAbort>) {
        if !crate::perf::is_enabled() {
            return;
        }
        let outcome = match result {
            Ok(_) => "judged",
            Err(SampleAbort::Cancelled) => "cancelled",
            Err(SampleAbort::ReadFailed(_)) => "read_failed",
        };
        crate::perf::event(
            "thumb",
            "auto_cover_sample",
            Some(&crate::path_key::normalize_keep_drive(path)),
            0,
            &[
                ("format", format.into()),
                ("selected", self.selected.into()),
                ("read", self.read.into()),
                ("skipped_limit", self.skipped_limit.into()),
                ("bytes", self.bytes.into()),
                ("list_ms", self.list_ms.into()),
                ("read_ms", self.read_ms.into()),
                ("outcome", outcome.into()),
            ],
        );
    }
}

/// PageSizes is IPC work: even an InvalidData response is a worker failure,
/// never evidence about a page's aspect. Do not discard a harvested success.
pub(crate) fn page_sizes_read_result(
    result: std::io::Result<Vec<(f32, f32)>>,
) -> Result<Vec<(f32, f32)>, SampleAbort> {
    result.map_err(|error| SampleAbort::ReadFailed(error.to_string()))
}

/// Ephemeral, read-only rotation lookup. Construct and query this handle only
/// on a sampling worker: opening the DB and reading a key both perform I/O.
pub(crate) struct SampleRotations(Option<RotationDb>);

impl SampleRotations {
    pub(crate) fn for_worker() -> Self {
        Self::from_path_worker(Some(&RotationDb::db_path()))
    }

    /// `None` explicitly disables lookup (for example in isolated evaluation).
    /// A missing DB is normal and must never be created by sampling.
    pub(crate) fn from_path_worker(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self(None);
        };
        match std::fs::metadata(path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self(None),
            Err(error) => {
                log::warn!("Cover page sampling could not inspect rotation DB: {error}");
                return Self(None);
            }
        }
        match RotationDb::open_readonly_at(path) {
            Ok(db) => Self(Some(db)),
            Err(error) => {
                log::warn!("Cover page sampling could not open rotation DB: {error}");
                Self(None)
            }
        }
    }

    pub(crate) fn get(&self, key: &str) -> Rotation {
        let Some(db) = &self.0 else {
            return Rotation::None;
        };
        match db.get_key_checked(key) {
            Ok(rotation) => rotation.unwrap_or_default(),
            Err(error) => {
                log::warn!("Cover page sampling could not read saved rotation: {error}");
                Rotation::None
            }
        }
    }
}

fn aspect_after_rotation(width: f64, height: f64, rotation: Rotation) -> f64 {
    match rotation {
        Rotation::Cw90 | Rotation::Cw270 => height / width,
        Rotation::None | Rotation::Cw180 => width / height,
    }
}

/// The representative never counts as a sample. Only a final representative
/// samples backwards; other representatives sample the remaining next pages.
pub(crate) fn sample_indices(len: usize, leaf: usize) -> Vec<usize> {
    if leaf >= len {
        return Vec::new();
    }
    if leaf + 1 == len {
        (0..leaf).rev().take(MAX_SAMPLE_PAGES).collect()
    } else {
        (leaf + 1..len).take(MAX_SAMPLE_PAGES).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageAspectDecline {
    TooFew,
    Landscape,
    Inconsistent,
}

impl PageAspectDecline {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TooFew => "too-few",
            Self::Landscape => "landscape",
            Self::Inconsistent => "inconsistent",
        }
    }
}

fn aspect_consensus(ratios: impl Iterator<Item = f64>) -> Result<f64, PageAspectDecline> {
    let ratios: Vec<_> = ratios.filter(|r| r.is_finite() && *r > 0.0).collect();
    if ratios.len() < 3 {
        return Err(PageAspectDecline::TooFew);
    }
    let mut portrait: Vec<_> = ratios.iter().copied().filter(|&r| r < 0.9).collect();
    if portrait.len() * 10 < ratios.len() * 7 {
        return Err(PageAspectDecline::Landscape);
    }
    portrait.sort_unstable_by(f64::total_cmp);
    let count = portrait.len();
    let median = (portrait[(count - 1) / 2] + portrait[count / 2]) * 0.5;
    let consistent = portrait
        .iter()
        .filter(|&&r| (r - median).abs() <= median * 0.06)
        .count();
    if consistent * 10 >= count * 7 {
        Ok(median)
    } else {
        Err(PageAspectDecline::Inconsistent)
    }
}

/// Keep unknown slots until after the leading run is removed: an unknown
/// page stops the run, rather than exposing a later landscape as a new first.
fn aspect_from_slots(
    slots: impl IntoIterator<Item = Option<f64>>,
) -> Result<f64, PageAspectDecline> {
    let slots: Vec<_> = slots
        .into_iter()
        .map(|r| r.filter(|r| r.is_finite() && *r > 0.0))
        .collect();
    let skip = slots
        .iter()
        .take(3)
        .take_while(|r| r.is_some_and(|r| r >= 0.9))
        .count();
    aspect_consensus(slots.into_iter().skip(skip).flatten())
}

pub(crate) fn page_aspect_from_sizes(sizes: &[(f32, f32)], leaf: usize) -> Option<f64> {
    page_aspect_from_sizes_report(sizes, leaf).ok()
}

pub(crate) fn page_aspect_from_sizes_report(
    sizes: &[(f32, f32)],
    leaf: usize,
) -> Result<f64, PageAspectDecline> {
    page_aspect_from_sizes_with_rotations_report(sizes, leaf, Path::new(""), |_| Rotation::None)
}

/// `sizes` uses the PDF worker's zero-based page order. `logical_pdf_path`
/// is the viewer's page-key container, which may differ from the read target.
pub(crate) fn page_aspect_from_sizes_with_rotations_report(
    sizes: &[(f32, f32)],
    leaf: usize,
    logical_pdf_path: &Path,
    mut lookup: impl FnMut(&str) -> Rotation,
) -> Result<f64, PageAspectDecline> {
    aspect_from_slots(sample_indices(sizes.len(), leaf).into_iter().map(|index| {
        let (width, height) = sizes[index];
        (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0).then(|| {
            let key = crate::edit_source::page_key_for_pdf(logical_pdf_path, index as u32);
            aspect_after_rotation(f64::from(width), f64::from(height), lookup(&key))
        })
    }))
}

/// Compatibility helper for callers which only need a best-effort ratio.
pub(crate) fn sample_zip_page_aspect(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Option<f64> {
    sample_zip_page_aspect_report(path, leaf_entry, prefix).ok()
}

pub(crate) fn sample_zip_page_aspect_report(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Result<f64, PageAspectDecline> {
    sample_zip_pages(path, leaf_entry, prefix)
}

pub(crate) fn sample_zip_pages(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Result<f64, PageAspectDecline> {
    sample_zip_pages_with_rotations(path, leaf_entry, prefix, path, |_| Rotation::None)
}

/// Keep physical reads on `path` while looking up saved rotations using the
/// viewer's container identity. This also covers evaluation copies of a ZIP.
pub(crate) fn sample_zip_pages_with_rotations(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
    logical_zip_path: &Path,
    lookup: impl FnMut(&str) -> Rotation,
) -> Result<f64, PageAspectDecline> {
    sample_container_pages_with_rotations_cancellable(
        path,
        leaf_entry,
        prefix,
        logical_zip_path,
        lookup,
        None,
    )
    .unwrap_or(Err(PageAspectDecline::TooFew))
}

/// Physical input and viewer page-key identity deliberately remain separate.
/// ZIP reads bounded prefixes; direct RAR reads a pre-budgeted set in one pass.
pub(crate) fn sample_container_pages_with_rotations_cancellable(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
    logical_zip_path: &Path,
    mut lookup: impl FnMut(&str) -> Rotation,
    cancel: Option<&AtomicBool>,
) -> SampleAspect {
    #[cfg(test)]
    CONTAINER_SAMPLE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let started = Instant::now();
    let mut metrics = SampleMetrics::default();
    let result = (|| {
        check_cancel(cancel)?;
        let prefixes = if crate::rar_loader::is_rar_path(path) {
            crate::rar_loader::read_cover_auto_page_samples(
                path,
                leaf_entry,
                prefix,
                cancel,
                MAX_RAR_SAMPLE_ENTRY_BYTES,
                MAX_RAR_SAMPLE_TOTAL_BYTES,
                &mut metrics,
            )?
        } else {
            crate::zip_loader::read_cover_auto_page_samples(
                path,
                leaf_entry,
                prefix,
                cancel,
                &mut metrics,
            )?
        };
        let mut slots = Vec::with_capacity(prefixes.len());
        for (name, read) in prefixes {
            check_cancel(cancel)?;
            let bytes = read.into_slot()?;
            let dimensions = bytes
                .as_deref()
                .and_then(|bytes| dimensions_from_prefix(bytes, &name));
            slots.push(dimensions.map(|(width, height)| {
                let key = crate::adjustment_db::zip_entry_key(logical_zip_path, &name);
                aspect_after_rotation(width, height, lookup(&key))
            }));
        }
        Ok(aspect_from_slots(slots))
    })();
    if metrics.read_ms == 0.0 {
        metrics.read_ms = (started.elapsed().as_secs_f64() * 1000.0 - metrics.list_ms).max(0.0);
    }
    metrics.emit(
        path,
        if crate::rar_loader::is_rar_path(path) {
            "rar"
        } else {
            "zip"
        },
        &result,
    );
    result
}

fn dimensions_from_prefix(bytes: &[u8], name: &str) -> Option<(f64, f64)> {
    let (width, height) = crate::app::probe_image_dims_from_bytes(bytes, name)?;
    (width > 0 && height > 0).then_some((f64::from(width), f64::from(height)))
}

fn read_folder_prefix_typed(path: &Path, cancel: Option<&AtomicBool>) -> SampleRead {
    if check_cancel(cancel).is_err() {
        return SampleRead::Cancelled;
    }
    let opened = std::fs::File::open(path);
    if check_cancel(cancel).is_err() {
        return SampleRead::Cancelled;
    }
    let file = match opened {
        Ok(file) => file,
        // Opening a source always failing is transient, even if an OS happens
        // to report an error kind also used for malformed stream data.
        Err(error) => return SampleRead::ReadFailed(error.to_string()),
    };
    let mut bytes = Vec::new();
    SampleRead::from_io(
        file.take(MAX_SAMPLE_PREFIX_BYTES)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(source_read_error),
        cancel,
    )
}

#[cfg(test)]
fn read_folder_prefix(path: &Path) -> Option<Vec<u8>> {
    read_folder_prefix_typed(path, None)
        .into_slot()
        .ok()
        .flatten()
}

/// The caller supplies the already enumerated image paths in FileNameSortKey
/// order. Do not enumerate or sort again: this is the representative worker's
/// same direct-child snapshot, and only eight bounded reads are allowed. On the
/// worker only, a missing rexif orientation may query WIC once per sampled file.
pub(crate) fn sample_folder_pages(
    paths: &[PathBuf],
    cover: &Path,
) -> Result<f64, PageAspectDecline> {
    sample_folder_pages_with_rotations(paths, cover, |_| Rotation::None)
}

pub(crate) fn sample_folder_pages_with_rotations(
    paths: &[PathBuf],
    cover: &Path,
    lookup: impl FnMut(&str) -> Rotation,
) -> Result<f64, PageAspectDecline> {
    sample_folder_pages_with_rotations_cancellable(paths, cover, lookup, None)
        .unwrap_or(Err(PageAspectDecline::TooFew))
}

pub(crate) fn sample_folder_pages_with_rotations_cancellable(
    paths: &[PathBuf],
    cover: &Path,
    lookup: impl FnMut(&str) -> Rotation,
    cancel: Option<&AtomicBool>,
) -> SampleAspect {
    sample_folder_pages_typed(
        paths,
        cover,
        lookup,
        |path| crate::thumb_loader::file_orientation_or_wic(None, path),
        cancel,
    )
}

#[cfg(test)]
fn sample_folder_pages_with_wic(
    paths: &[PathBuf],
    cover: &Path,
    wic_orientation: impl FnMut(&Path) -> u16,
) -> Result<f64, PageAspectDecline> {
    sample_folder_pages_with_rotations_and_wic(paths, cover, |_| Rotation::None, wic_orientation)
}

fn sample_folder_pages_with_rotations_and_wic(
    paths: &[PathBuf],
    cover: &Path,
    lookup: impl FnMut(&str) -> Rotation,
    wic_orientation: impl FnMut(&Path) -> u16,
) -> Result<f64, PageAspectDecline> {
    sample_folder_pages_typed(paths, cover, lookup, wic_orientation, None)
        .unwrap_or(Err(PageAspectDecline::TooFew))
}

fn sample_folder_pages_typed(
    paths: &[PathBuf],
    cover: &Path,
    mut lookup: impl FnMut(&str) -> Rotation,
    mut wic_orientation: impl FnMut(&Path) -> u16,
    cancel: Option<&AtomicBool>,
) -> SampleAspect {
    let mut metrics = SampleMetrics::default();
    let started = Instant::now();
    let result = (|| {
        check_cancel(cancel)?;
        let leaf = paths
            .iter()
            .position(|path| crate::path_key::eq_keep_drive(path, cover));
        let Some(leaf) = leaf else {
            return Ok(Err(PageAspectDecline::TooFew));
        };
        let indices = sample_indices(paths.len(), leaf);
        metrics.selected = indices.len();
        metrics.list_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut slots = Vec::with_capacity(indices.len());
        for index in indices {
            let path = &paths[index];
            let bytes = read_folder_prefix_typed(path, cancel).into_slot()?;
            if let Some(bytes) = &bytes {
                metrics.read += 1;
                metrics.bytes += bytes.len();
            }
            slots.push(
                bytes
                    .as_deref()
                    .and_then(|bytes| {
                        folder_dimensions_from_prefix(bytes, path, &mut wic_orientation)
                    })
                    .map(|(width, height)| {
                        let key = crate::adjustment_db::normalize_path(path);
                        aspect_after_rotation(width, height, lookup(&key))
                    }),
            );
        }
        check_cancel(cancel)?;
        Ok(aspect_from_slots(slots))
    })();
    metrics.read_ms = (started.elapsed().as_secs_f64() * 1000.0 - metrics.list_ms).max(0.0);
    metrics.emit(cover, "folder", &result);
    result
}

fn folder_dimensions_from_prefix(
    bytes: &[u8],
    path: &Path,
    wic_orientation: &mut impl FnMut(&Path) -> u16,
) -> Option<(f64, f64)> {
    // This probe has already applied rexif, or native RAW/AVIF orientation.
    // Never apply that transform twice. Reuse the owned prefix for the
    // presence check; no second file read is needed for rexif.
    let (mut width, mut height) = dimensions_from_prefix(bytes, &path.to_string_lossy())?;
    if crate::raw_format::is_raw_path(path)
        || path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("avif"))
        || crate::thumb_loader::read_exif_orientation_from_bytes_if_present(bytes).is_some()
    {
        return Some((width, height));
    }
    if (5..=8).contains(&wic_orientation(path)) {
        std::mem::swap(&mut width, &mut height);
    }
    Some((width, height))
}

#[cfg(test)]
#[path = "cover_auto_sample/saved_rotation_tests.rs"]
mod saved_rotation_tests;

#[cfg(test)]
#[path = "cover_auto_sample/rar_tests.rs"]
pub(crate) mod rar_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    pub(super) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    pub(super) fn oriented_image(
        extension: &str,
        width: u32,
        height: u32,
        orientation: u16,
    ) -> Vec<u8> {
        use image::ImageEncoder;
        let mut exif = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend(orientation.to_le_bytes());
        exif.extend([0; 6]);
        let pixels = image::RgbImage::from_pixel(width, height, image::Rgb([30, 80, 140]));
        let mut bytes = Vec::new();
        match extension {
            "webp" => {
                let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut bytes);
                encoder.set_exif_metadata(exif).unwrap();
                encoder
                    .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
                    .unwrap();
            }
            "png" => {
                let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
                encoder.set_exif_metadata(exif).unwrap();
                encoder
                    .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
                    .unwrap();
            }
            "jpg" => {
                image::DynamicImage::ImageRgb8(pixels)
                    .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
                    .unwrap();
                let mut app1 = vec![0xff, 0xe1];
                app1.extend(((exif.len() + 8) as u16).to_be_bytes());
                app1.extend(b"Exif\0\0");
                app1.extend(exif);
                bytes.splice(2..2, app1);
            }
            _ => unreachable!(),
        }
        bytes
    }

    #[test]
    fn folder_webp_orientation_matches_cover_file_decode_and_zip_stays_bytes_only() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = oriented_image("webp", 900, 600, 6);
        let paths: Vec<_> = (0..11)
            .map(|index| {
                let path = dir.path().join(format!("{index:03}.webp"));
                std::fs::write(&path, &bytes).unwrap();
                path
            })
            .collect();
        assert_eq!(
            crate::app::probe_image_dims_from_bytes(&bytes, "page.webp"),
            Some((900, 600))
        );
        let cover = crate::thumb_loader::decode_cover_auto_analysis_image(
            crate::image_decode::Source::Path(&paths[0]),
            None,
            None,
        )
        .unwrap();
        // This Windows WIC decoder exposes WebP's orientation, unlike rexif.
        #[cfg(windows)]
        assert_eq!(cover.native_dims, (600, 900));
        let sample = sample_folder_pages(&paths, &paths[0]);
        assert_eq!(
            folder_dimensions_from_prefix(&bytes, &paths[1], &mut |path| {
                crate::thumb_loader::file_orientation_or_wic(None, path)
            }),
            Some((
                f64::from(cover.native_dims.0),
                f64::from(cover.native_dims.1)
            ))
        );
        assert_eq!(
            sample,
            aspect_from_slots(
                [Some(f64::from(cover.native_dims.0) / f64::from(cover.native_dims.1)); 8]
            )
        );
        #[cfg(windows)]
        assert_eq!(sample, Ok(2.0 / 3.0));
        let archive = zip((0..11)
            .map(|i| (format!("{i:03}.webp"), bytes.clone()))
            .collect());
        let zip_sample = sample_zip_pages(archive.path(), "000.webp", None);
        assert_eq!(
            dimensions_from_prefix(&bytes, "001.webp"),
            Some((900.0, 600.0))
        );
        assert_eq!(zip_sample, Err(PageAspectDecline::Landscape));
    }

    #[test]
    fn folder_png_and_jpeg_orientation6_keep_released_dimensions() {
        for (extension, stored) in [("png", (70, 100)), ("jpg", (100, 70))] {
            let dir = tempfile::tempdir().unwrap();
            let bytes = oriented_image(extension, stored.0, stored.1, 6);
            let paths: Vec<_> = (0..4)
                .map(|index| {
                    let path = dir.path().join(format!("{index:03}.{extension}"));
                    std::fs::write(&path, &bytes).unwrap();
                    path
                })
                .collect();
            let cover = crate::thumb_loader::decode_cover_auto_analysis_image(
                crate::image_decode::Source::Path(&paths[0]),
                None,
                None,
            )
            .unwrap();
            assert_eq!(cover.native_dims, (70, 100));
            let sample = sample_folder_pages(&paths, &paths[0]);
            assert_eq!(
                folder_dimensions_from_prefix(&bytes, &paths[1], &mut |path| {
                    crate::thumb_loader::file_orientation_or_wic(None, path)
                }),
                Some((70.0, 100.0))
            );
            assert_eq!(sample, Ok(0.7));
        }
    }

    #[test]
    fn folder_wic_fallback_only_for_missing_rexif_and_never_double_rotates() {
        for orientation in [1, 6] {
            let bytes = oriented_image("jpg", 100, 70, orientation);
            let result = folder_dimensions_from_prefix(&bytes, Path::new("page.jpg"), &mut |_| {
                panic!("valid rexif must not open WIC")
            });
            assert_eq!(
                result,
                Some(if orientation == 6 {
                    (70.0, 100.0)
                } else {
                    (100.0, 70.0)
                })
            );
        }
        assert_eq!(
            folder_dimensions_from_prefix(
                b"invalid header",
                Path::new("bad.png"),
                &mut |_| panic!("no dimensions must not open WIC")
            ),
            None
        );
        let mut calls = 0;
        let mut fallback = |_: &Path| {
            calls += 1;
            6
        };
        assert_eq!(
            folder_dimensions_from_prefix(&png(100, 70), Path::new("plain.png"), &mut fallback),
            Some((70.0, 100.0))
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn folder_wic_budget_keeps_eight_attempts_and_256k_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = png(100, 70);
        bytes.resize(300 * 1024, 0);
        let paths: Vec<_> = (0..12)
            .map(|i| {
                let path = dir.path().join(format!("{i:03}.png"));
                std::fs::write(&path, &bytes).unwrap();
                path
            })
            .collect();
        assert_eq!(
            read_folder_prefix(&paths[1]).unwrap().len(),
            MAX_SAMPLE_PREFIX_BYTES as usize
        );
        let mut opened = Vec::new();
        let sample = sample_folder_pages_with_wic(&paths, &paths[0], |path| {
            opened.push(path.to_owned());
            6
        });
        assert_eq!(opened, paths[1..=8]);
        assert_eq!(
            folder_dimensions_from_prefix(&bytes, &paths[1], &mut |_| 6),
            Some((70.0, 100.0))
        );
        assert_eq!(sample, Ok(0.7));
    }

    pub(super) fn zip(entries: Vec<(String, Vec<u8>)>) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    #[test]
    fn pdf_sample_preserves_unknown_slots_until_leading_trim() {
        let sizes = [
            (300.0, 100.0),
            (140.0, 100.0),
            (f32::NAN, 100.0),
            (140.0, 100.0),
            (140.0, 100.0),
            (140.0, 100.0),
            (70.0, 100.0),
            (70.0, 100.0),
            (70.0, 100.0),
        ];
        assert_eq!(
            page_aspect_from_sizes_report(&sizes, 0),
            Err(PageAspectDecline::Landscape)
        );
    }

    #[test]
    fn folder_uses_supplied_name_order_and_eight_attempts_without_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in [10, 0, 9, 2, 3, 1, 4, 5, 6, 7, 8, 11] {
            let path = dir.path().join(format!("{i:02}.png"));
            let bytes = if i == 1 {
                b"unknown".to_vec()
            } else {
                png(60 + i, 100)
            };
            std::fs::write(&path, bytes).unwrap();
            paths.push(path);
        }
        paths.sort_by_cached_key(|path| {
            crate::filename_sort::FileNameSortKey::new(&path.file_name().unwrap().to_string_lossy())
        });
        assert_eq!(paths[2].file_name().unwrap(), "02.png");
        assert_eq!(paths[10].file_name().unwrap(), "10.png");
        // This direct child is deliberately absent from the supplied snapshot.
        std::fs::write(dir.path().join("00a.png"), png(200, 100)).unwrap();
        let mut opened = Vec::new();
        let sample = sample_folder_pages_with_wic(&paths, &paths[0], |path| {
            opened.push(path.to_owned());
            1
        });
        assert_eq!(sample, Ok(0.65));
        // The unknown first slot consumes an attempt without querying WIC.
        assert_eq!(opened, paths[2..=8]);
        opened.clear();
        let backwards = sample_folder_pages_with_wic(&paths, &paths[11], |path| {
            opened.push(path.to_owned());
            1
        });
        assert_eq!(backwards, Ok((0.66 + 0.67) * 0.5));
        assert_eq!(
            opened,
            paths[3..=10].iter().rev().cloned().collect::<Vec<_>>()
        );
        assert_eq!(
            sample_folder_pages(&paths, &dir.path().join("missing.png")),
            Err(PageAspectDecline::TooFew)
        );
    }

    #[test]
    fn folder_prefix_read_is_bounded_and_os_failure_aborts() {
        let dir = tempfile::tempdir().unwrap();
        let cover = dir.path().join("0.png");
        let large = dir.path().join("1.png");
        let missing = dir.path().join("2.png");
        std::fs::write(&large, vec![0; 300 * 1024]).unwrap();
        assert_eq!(
            read_folder_prefix(&large).unwrap().len(),
            MAX_SAMPLE_PREFIX_BYTES as usize
        );
        let mut paths = vec![cover.clone(), large, missing];
        for i in 3..9 {
            let path = dir.path().join(format!("{i}.png"));
            std::fs::write(&path, png(if i < 6 { 140 } else { 70 }, 100)).unwrap();
            paths.push(path);
        }
        assert!(matches!(
            sample_folder_pages_with_rotations_cancellable(
                &paths,
                &cover,
                |_| Rotation::None,
                None,
            ),
            Err(SampleAbort::ReadFailed(_))
        ));
        // A data-only failure retains its slot and cannot expose a later
        // landscape run to leading-slot trimming.
        std::fs::write(&paths[2], b"unknown header").unwrap();
        assert_eq!(
            sample_folder_pages(&paths, &cover),
            Err(PageAspectDecline::Landscape)
        );
    }

    #[test]
    fn zip_sample_preserves_unknown_slots_until_leading_trim() {
        let file = zip(vec![
            ("0.png".into(), png(300, 100)),
            ("1.png".into(), png(140, 100)),
            ("2.png".into(), b"unknown".to_vec()),
            ("3.png".into(), png(140, 100)),
            ("4.png".into(), png(140, 100)),
            ("5.png".into(), png(140, 100)),
            ("6.png".into(), png(70, 100)),
            ("7.png".into(), png(70, 100)),
            ("8.png".into(), png(70, 100)),
        ]);
        let sample = sample_zip_pages(file.path(), "0.png", None);
        assert_eq!(sample, Err(PageAspectDecline::Landscape));
        assert_eq!(
            sample_zip_page_aspect_report(file.path(), "0.png", None),
            sample
        );
    }

    #[test]
    fn next_pages_last_page_and_consensus_boundaries() {
        assert_eq!(sample_indices(12, 0), (1..9).collect::<Vec<_>>());
        assert_eq!(sample_indices(12, 11), (3..11).rev().collect::<Vec<_>>());
        assert_eq!(sample_indices(12, 10), vec![11]);
        assert!(sample_indices(12, 12).is_empty());
        let mut sizes = vec![(70.0, 100.0); 11];
        sizes[0] = (400.0, 100.0);
        assert_eq!(page_aspect_from_sizes(&sizes, 0), Some(0.7));
        assert_eq!(page_aspect_from_sizes(&sizes, 10), Some(0.7));
        assert_eq!(page_aspect_from_sizes(&sizes, 9), None);
        sizes[1] = (200.0, 100.0);
        sizes[2] = (200.0, 100.0);
        assert_eq!(page_aspect_from_sizes(&sizes, 0), Some(0.7)); // 6/8 portrait
        sizes[3] = (200.0, 100.0);
        assert_eq!(page_aspect_from_sizes(&sizes, 0), Some(0.7)); // leading three omitted
        assert_eq!(
            aspect_consensus([0.5, 0.6, 0.7, 0.8].into_iter()),
            Err(PageAspectDecline::Inconsistent)
        );
        assert_eq!(
            aspect_consensus([0.7, 0.7].into_iter()),
            Err(PageAspectDecline::TooFew)
        );
        assert_eq!(
            aspect_consensus([0.9, 0.9, 0.9].into_iter()),
            Err(PageAspectDecline::Landscape)
        );
    }

    #[test]
    fn leading_landscape_slots_pdf_zip_forward_backward_and_typed_reasons() {
        use PageAspectDecline::*;
        let mut cases = Vec::new();
        for count in 1..=3 {
            cases.push((
                vec![Some(1.4); count]
                    .into_iter()
                    .chain(vec![Some(0.7); 8 - count])
                    .collect::<Vec<_>>(),
                Ok(0.7),
            ));
        }
        cases.extend([
            (
                vec![
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Ok(0.7),
            ),
            (
                vec![
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (
                vec![
                    Some(0.7),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (vec![Some(1.4); 8], Err(Landscape)),
            (
                vec![
                    None,
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (
                vec![
                    Some(1.4),
                    None,
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (vec![Some(0.7); 2], Err(TooFew)),
            (
                vec![Some(0.5), Some(0.6), Some(0.7), Some(0.8)],
                Err(Inconsistent),
            ),
        ]);
        for (slots, expected) in cases {
            assert_eq!(aspect_from_slots(slots.clone()), expected, "{slots:?}");
            for backwards in [false, true] {
                let mut values = vec![Some(3.0)];
                values.extend(slots.iter().copied());
                if backwards {
                    values.reverse();
                }
                let leaf = if backwards { values.len() - 1 } else { 0 };
                let sizes: Vec<_> = values
                    .iter()
                    .map(|r| r.map_or((0.0, 0.0), |r| ((r * 100.0) as f32, 100.0)))
                    .collect();
                assert_eq!(
                    page_aspect_from_sizes_report(&sizes, leaf),
                    expected,
                    "PDF {slots:?}/{backwards}"
                );
                let entries = values
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        (
                            format!("{i:03}.png"),
                            r.map_or_else(|| b"unknown".to_vec(), |r| png((r * 100.0) as u32, 100)),
                        )
                    })
                    .collect();
                let file = zip(entries);
                let prefixes = crate::zip_loader::read_cover_auto_page_prefixes(
                    file.path(),
                    &format!("{leaf:03}.png"),
                    None,
                )
                .unwrap();
                assert_eq!(prefixes.len(), slots.len());
                assert_eq!(
                    sample_zip_page_aspect_report(file.path(), &format!("{leaf:03}.png"), None),
                    expected,
                    "ZIP {slots:?}/{backwards}"
                );
            }
        }
    }

    #[test]
    fn zip_prefix_budget_is_eight_attempts_not_eight_successes() {
        let entries = (0..12)
            .map(|i| (format!("{i:03}.png"), vec![0; 300 * 1024]))
            .collect();
        let file = zip(entries);
        let prefixes =
            crate::zip_loader::read_cover_auto_page_prefixes(file.path(), "000.png", None).unwrap();
        assert_eq!(prefixes.len(), 8);
        assert!(
            prefixes
                .iter()
                .all(|(_, bytes)| bytes.as_ref().unwrap().len() == MAX_SAMPLE_PREFIX_BYTES as usize)
        );
        assert_eq!(prefixes.first().unwrap().0, "001.png");
        assert_eq!(prefixes.last().unwrap().0, "008.png");
        assert_eq!(
            prefixes
                .iter()
                .map(|(_, bytes)| bytes.as_ref().unwrap().len())
                .sum::<usize>(),
            8 * 256 * 1024
        );
        assert_eq!(sample_zip_page_aspect(file.path(), "000.png", None), None);
    }

    #[test]
    fn zip_reads_only_selected_prefix_and_skips_nested_and_ignored_entries() {
        let mut entries = vec![
            ("book/000.png".into(), png(300, 100)),
            ("book/001.png".into(), b"invalid header".to_vec()),
            ("book/003.txt".into(), b"text".to_vec()),
            ("book/__MACOSX/004.png".into(), png(200, 100)),
            (
                "book/nested.zip".into(),
                b"not a zip; must not be expanded".to_vec(),
            ),
            ("book/nested.zip/005.png".into(), png(200, 100)),
            ("other/006.png".into(), png(200, 100)),
        ];
        for i in [2, 4, 6, 7] {
            entries.push((format!("book/{i:03}.png"), png(70, 100)));
        }
        let file = zip(entries);
        assert_eq!(
            sample_zip_page_aspect(file.path(), "book/000.png", Some("book/")),
            Some(0.7)
        );
        assert_eq!(
            sample_zip_page_aspect(file.path(), "book/007.png", Some("book")),
            Some(0.7)
        );
        assert_eq!(
            sample_zip_page_aspect(file.path(), "book/nested.zip/005.png", Some("book/")),
            None
        );
        assert_eq!(
            sample_zip_page_aspect(file.path(), "absent.png", None),
            None
        );
    }

    #[test]
    fn zip_probe_applies_exif_orientation_before_consensus() {
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(100, 70))
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        // APP1 Exif, little-endian TIFF, one SHORT orientation=6 entry.
        let exif: &[u8] = b"Exif\0\0II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00";
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        bytes.extend_from_slice(exif);
        bytes.extend_from_slice(&jpeg.into_inner()[2..]);
        let file = zip((0..4)
            .map(|i| (format!("{i:03}.jpg"), bytes.clone()))
            .collect());
        assert_eq!(
            sample_zip_page_aspect(file.path(), "000.jpg", None),
            Some(0.7)
        );
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..4)
            .map(|i| {
                let path = dir.path().join(format!("{i:03}.jpg"));
                std::fs::write(&path, &bytes).unwrap();
                path
            })
            .collect();
        let sample = sample_folder_pages(&paths, &paths[0]);
        assert_eq!(sample, Ok(0.7));
        assert_eq!(
            dimensions_from_prefix(&bytes, "001.jpg"),
            Some((70.0, 100.0))
        );
    }
}
