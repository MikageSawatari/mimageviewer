//! Flat, non-solid RAR/CBR access used by the virtual-folder read boundary.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::archive_converter::{
    ArchiveImageSummary, dedup_entry_name, is_image_entry, nested_archive_kind, should_ignore_entry,
};
use crate::zip_loader::{ZipEnumeration, ZipImageEntry};

const MAX_DIRECT_ENTRY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DECISION_CACHE_CAPACITY: usize = 32;
// Volume resolution entries contain only two paths and a small enum. Keep more of them than the
// full inspection cache so folder navigation and thumbnail workers can reuse probes across a
// moderately large archive folder without retaining archive contents or handles.
const VOLUME_RESOLUTION_CACHE_CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RarDirectReadDecision {
    Direct,
    Solid,
    NestedArchive,
    Encrypted,
}

impl RarDirectReadDecision {
    fn diagnostic_label(self) -> &'static str {
        match self {
            Self::Direct => "Direct",
            Self::Solid => "Solid",
            Self::NestedArchive => "Nested",
            Self::Encrypted => "Encrypted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RarInspectionOrigin {
    FolderDecisionWorker,
    ExplicitOpen,
}

impl RarInspectionOrigin {
    fn diagnostic_label(self) -> &'static str {
        match self {
            Self::FolderDecisionWorker => "folder_decision_worker",
            Self::ExplicitOpen => "explicit_open",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectionCacheOutcome {
    Unknown,
    Hit,
    Miss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RarVolumeKind {
    Single,
    First,
    Subsequent,
}

impl From<unrar::VolumeInfo> for RarVolumeKind {
    fn from(value: unrar::VolumeInfo) -> Self {
        match value {
            unrar::VolumeInfo::None => Self::Single,
            unrar::VolumeInfo::First => Self::First,
            unrar::VolumeInfo::Subsequent => Self::Subsequent,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RarInspection {
    pub decision: RarDirectReadDecision,
    pub summary: ArchiveImageSummary,
    /// Header truth for the path that the caller originally supplied.
    pub volume_kind: RarVolumeKind,
    /// The path that must actually be listed/read. For a subsequent volume this is the
    /// first volume; otherwise it is the original path.
    pub resolved_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DecisionCacheKey {
    path: PathBuf,
    len: u64,
    mtime: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RarVolumeResolution {
    resolved_path: PathBuf,
    volume_kind: RarVolumeKind,
}

static DECISION_CACHE: LazyLock<Mutex<Vec<(DecisionCacheKey, RarInspection)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static VOLUME_RESOLUTION_CACHE: LazyLock<Mutex<Vec<(DecisionCacheKey, RarVolumeResolution)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

#[cfg(test)]
static VOLUME_RESOLUTION_PROBE_COUNTS: LazyLock<Mutex<Vec<(PathBuf, usize)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

pub fn is_rar_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rar") || ext.eq_ignore_ascii_case("cbr"))
}

fn cache_key(path: &Path) -> io::Result<DecisionCacheKey> {
    let meta = std::fs::metadata(path)?;
    Ok(DecisionCacheKey {
        path: path.to_path_buf(),
        len: meta.len(),
        mtime: meta.modified().ok(),
    })
}

fn cached_volume_resolution(key: &DecisionCacheKey) -> Option<RarVolumeResolution> {
    let cache = VOLUME_RESOLUTION_CACHE.lock().ok()?;
    cache
        .iter()
        .rev()
        .find(|(cached, _)| cached == key)
        .map(|(_, resolution)| resolution.clone())
}

fn remember_volume_resolution(key: &DecisionCacheKey, resolution: RarVolumeResolution) {
    let Ok(mut cache) = VOLUME_RESOLUTION_CACHE.lock() else {
        return;
    };
    // A changed `(len, mtime)` for the same path supersedes the previous identity.
    cache.retain(|(cached, _)| cached.path != key.path);
    if cache.len() >= VOLUME_RESOLUTION_CACHE_CAPACITY {
        cache.remove(0);
    }
    cache.push((key.clone(), resolution));
}

#[cfg(test)]
fn record_volume_resolution_probe(path: &Path) {
    let Ok(mut counts) = VOLUME_RESOLUTION_PROBE_COUNTS.lock() else {
        return;
    };
    if let Some((_, count)) = counts.iter_mut().find(|(cached, _)| cached == path) {
        *count += 1;
    } else {
        counts.push((path.to_path_buf(), 1));
    }
}

fn normalized_entry_name(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn deduplicated_visible_image_name(name: &str, seen: &mut HashSet<String>) -> Option<String> {
    if should_ignore_entry(name) || !is_image_entry(name) {
        return None;
    }
    Some(dedup_entry_name(name.to_string(), seen))
}

fn unrar_io(err: unrar::error::UnrarError) -> io::Error {
    let kind = match err.code {
        unrar::error::Code::MissingPassword => io::ErrorKind::PermissionDenied,
        unrar::error::Code::BadPassword => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, format!("RAR: {err}"))
}

pub(crate) fn classify_direct_read(
    is_solid: bool,
    has_nested: bool,
    has_encrypted: bool,
) -> RarDirectReadDecision {
    if is_solid {
        RarDirectReadDecision::Solid
    } else if has_nested {
        RarDirectReadDecision::NestedArchive
    } else if has_encrypted {
        RarDirectReadDecision::Encrypted
    } else {
        RarDirectReadDecision::Direct
    }
}

fn open_listing_from_volume_with_key(
    path: &Path,
    key: Option<&DecisionCacheKey>,
) -> io::Result<(
    unrar::OpenArchive<unrar::List, unrar::CursorBeforeHeader>,
    RarVolumeKind,
    PathBuf,
)> {
    if let Some(resolution) = key.and_then(cached_volume_resolution) {
        let opened = unrar::Archive::new(&resolution.resolved_path)
            .open_for_listing()
            .map_err(unrar_io)?;
        return Ok((opened, resolution.volume_kind, resolution.resolved_path));
    }

    #[cfg(test)]
    record_volume_resolution_probe(path);
    let archive = unrar::Archive::new(path);
    let first_part = archive.first_part();
    let opened = archive.open_for_listing().map_err(unrar_io)?;
    let volume_kind = RarVolumeKind::from(opened.volume_info());
    let resolved_path = if volume_kind == RarVolumeKind::Subsequent {
        first_part
    } else {
        path.to_path_buf()
    };
    if let Some(key) = key {
        remember_volume_resolution(
            key,
            RarVolumeResolution {
                resolved_path: resolved_path.clone(),
                volume_kind,
            },
        );
    }
    if volume_kind != RarVolumeKind::Subsequent {
        return Ok((opened, volume_kind, resolved_path));
    }
    drop(opened);
    let opened = unrar::Archive::new(&resolved_path)
        .open_for_listing()
        .map_err(unrar_io)?;
    Ok((opened, volume_kind, resolved_path))
}

fn open_listing_from_volume(
    path: &Path,
) -> io::Result<(
    unrar::OpenArchive<unrar::List, unrar::CursorBeforeHeader>,
    RarVolumeKind,
    PathBuf,
)> {
    let key = cache_key(path).ok();
    open_listing_from_volume_with_key(path, key.as_ref())
}

fn resolved_volume_path(path: &Path) -> io::Result<(PathBuf, RarVolumeKind)> {
    let key = cache_key(path).ok();
    if let Some(resolution) = key.as_ref().and_then(cached_volume_resolution) {
        return Ok((resolution.resolved_path, resolution.volume_kind));
    }

    #[cfg(test)]
    record_volume_resolution_probe(path);
    let archive = unrar::Archive::new(path);
    let first_part = archive.first_part();
    let opened = archive.open_for_listing().map_err(unrar_io)?;
    let volume_kind = RarVolumeKind::from(opened.volume_info());
    drop(opened);
    let resolved = if volume_kind == RarVolumeKind::Subsequent {
        first_part
    } else {
        path.to_path_buf()
    };
    if let Some(key) = key.as_ref() {
        remember_volume_resolution(
            key,
            RarVolumeResolution {
                resolved_path: resolved.clone(),
                volume_kind,
            },
        );
    }
    Ok((resolved, volume_kind))
}

/// Header-backed decision for worker-side folder navigation. A malformed or unreadable RAR is
/// reported as an error so callers can conservatively keep it visible.
pub fn is_subsequent_volume(path: &Path) -> io::Result<bool> {
    resolved_volume_path(path).map(|(_, kind)| kind == RarVolumeKind::Subsequent)
}

/// Inspect a RAR once per `(path, mtime, size)` identity.
pub fn inspect_for_direct_read(path: &Path) -> io::Result<RarInspection> {
    let cancel = AtomicBool::new(false);
    inspect_for_direct_read_cancelable(path, &cancel)
}

/// Cancellable direct-read inspection for archive-open requests. Header open itself is delegated
/// to unrar, then every entry boundary observes the request's shared cancellation token.
pub fn inspect_for_direct_read_cancelable(
    path: &Path,
    cancel: &AtomicBool,
) -> io::Result<RarInspection> {
    inspect_for_direct_read_run(path, cancel).0
}

/// Diagnostic wrapper for the two RAR decision boundaries relevant to an explicit grid open.
/// It emits one bounded summary per inspection; individual archive entries are never logged.
pub(crate) fn inspect_for_direct_read_cancelable_traced(
    path: &Path,
    cancel: &AtomicBool,
    origin: RarInspectionOrigin,
    input_seq: u64,
) -> io::Result<RarInspection> {
    if !crate::perf::is_enabled() {
        return inspect_for_direct_read_cancelable(path, cancel);
    }
    let started = std::time::Instant::now();
    let archive_key = crate::path_key::normalize_keep_drive(path);
    crate::perf::event(
        "rar",
        "inspection_begin",
        Some(&archive_key),
        input_seq,
        &[("origin", serde_json::Value::from(origin.diagnostic_label()))],
    );
    let (result, cache_outcome, scanned_entries) = inspect_for_direct_read_run(path, cancel);
    match cache_outcome {
        InspectionCacheOutcome::Hit | InspectionCacheOutcome::Miss => crate::perf::event(
            "rar",
            if cache_outcome == InspectionCacheOutcome::Hit {
                "decision_cache_hit"
            } else {
                "decision_cache_miss"
            },
            Some(&archive_key),
            input_seq,
            &[("origin", serde_json::Value::from(origin.diagnostic_label()))],
        ),
        InspectionCacheOutcome::Unknown => {}
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    match &result {
        Ok(inspection) => crate::perf::event(
            "rar",
            "inspection_end",
            Some(&archive_key),
            input_seq,
            &[
                ("origin", serde_json::Value::from(origin.diagnostic_label())),
                ("ms", serde_json::Value::from(elapsed_ms)),
                ("scanned_entries", serde_json::Value::from(scanned_entries)),
                (
                    "decision",
                    serde_json::Value::from(inspection.decision.diagnostic_label()),
                ),
            ],
        ),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => crate::perf::event(
            "rar",
            "inspection_cancel",
            Some(&archive_key),
            input_seq,
            &[
                ("origin", serde_json::Value::from(origin.diagnostic_label())),
                ("ms", serde_json::Value::from(elapsed_ms)),
                ("scanned_entries", serde_json::Value::from(scanned_entries)),
            ],
        ),
        Err(error) => crate::perf::event(
            "rar",
            "inspection_error",
            Some(&archive_key),
            input_seq,
            &[
                ("origin", serde_json::Value::from(origin.diagnostic_label())),
                ("ms", serde_json::Value::from(elapsed_ms)),
                ("scanned_entries", serde_json::Value::from(scanned_entries)),
                (
                    "error_kind",
                    serde_json::Value::from(format!("{:?}", error.kind())),
                ),
            ],
        ),
    }
    result
}

fn inspect_for_direct_read_run(
    path: &Path,
    cancel: &AtomicBool,
) -> (io::Result<RarInspection>, InspectionCacheOutcome, u64) {
    let mut cache_outcome = InspectionCacheOutcome::Unknown;
    let mut scanned_entries = 0u64;
    let result = (|| {
        check_inspection_cancel(cancel)?;
        let key = cache_key(path)?;
        if let Ok(cache) = DECISION_CACHE.lock()
            && let Some((_, inspection)) = cache.iter().find(|(cached, _)| cached == &key)
        {
            cache_outcome = InspectionCacheOutcome::Hit;
            check_inspection_cancel(cancel)?;
            return Ok(inspection.clone());
        }
        cache_outcome = InspectionCacheOutcome::Miss;
        check_inspection_cancel(cancel)?;
        let (mut archive, volume_kind, resolved_path) =
            open_listing_from_volume_with_key(path, Some(&key))?;
        let is_solid = archive.is_solid();
        let mut has_encrypted = archive.has_encrypted_headers();
        let mut image_count = 0u32;
        let mut total_uncompressed_bytes = 0u64;
        let mut nested_archive_count = 0u32;
        loop {
            check_inspection_cancel(cancel)?;
            let Some(entry) = archive.next() else {
                break;
            };
            let entry = entry.map_err(unrar_io)?;
            scanned_entries = scanned_entries.saturating_add(1);
            // Encryption is an archive eligibility property, including entries hidden from the UI.
            has_encrypted |= entry.is_encrypted();
            if !entry.is_file() {
                continue;
            }
            let name = normalized_entry_name(&entry.filename);
            if should_ignore_entry(&name) {
                continue;
            }
            if is_image_entry(&name) {
                image_count = image_count.saturating_add(1);
                total_uncompressed_bytes =
                    total_uncompressed_bytes.saturating_add(entry.unpacked_size);
            } else if nested_archive_kind(&name).is_some() {
                nested_archive_count = nested_archive_count.saturating_add(1);
            }
        }
        check_inspection_cancel(cancel)?;
        let inspection = RarInspection {
            decision: classify_direct_read(is_solid, nested_archive_count > 0, has_encrypted),
            summary: ArchiveImageSummary {
                image_count,
                total_uncompressed_bytes,
                nested_archive_count,
            },
            volume_kind,
            resolved_path,
        };
        if let Ok(mut cache) = DECISION_CACHE.lock() {
            cache.retain(|(cached, _)| cached.path != key.path);
            if cache.len() >= DECISION_CACHE_CAPACITY {
                cache.remove(0);
            }
            cache.push((key, inspection.clone()));
        }
        Ok(inspection)
    })();
    (result, cache_outcome, scanned_entries)
}

#[cfg(test)]
pub(crate) fn direct_read_test_fixture_bytes() -> Vec<u8> {
    use base64::Engine as _;

    // Tiny non-solid RAR with two distinct PNG payloads whose stored names are both page.png.
    // Keep the fixture in one place so executor tests exercise the same header-backed path.
    base64::engine::general_purpose::STANDARD
        .decode(
            "UmFyIRoHAQAzkrXlCgEFBgAFAQGAgABdyPuwJAIDC6UBBKUBIL8JEzuAAAAIcGFnZS5wbmcKAwKX0YuaNf3cAYlQTkcNChoKAAAADUlIRFIAAABAAAAAMAgCAAAALinrSAAAAGxJREFUeJztz7EJgDAAAEEMYiGW7j9mCgtxjCP48APcb+8z9+tetzGOc+28oAEtaEALGtCCBrSgAS1oQAsa0IIGtKABLWhACxrQgga0oAEtaEALGtCCBrSgAS1oQAsa0IIGtKABLWhAC34+8AHOE9+qokthcQAAAABJRU5ErkJggo4TMhYkAgMLvAEEvAEgRG2V/YAAAAhwYWdlLnBuZwoDAgoki5o1/dwBiVBORw0KGgoAAAANSUhEUgAAAEAAAAAwCAIAAAAuKetIAAAAC3RFWHRwYXJhbWV0ZXJzAAmqaREAAABsSURBVHic7c+xCYAwAABBDGIhli6YeR3I1jGO4MMPcL89892ve93GOM6184IGtKABLWhACxrQgga0oAEtaEALGtCCBrSgAS1oQAsa0IIGtKABLWhACxrQgga0oAEtaEALGtCCBrSgAS34+cAHG8He3/HxQZMAAAAASUVORK5CYIIdd1ZRAwUEAA==",
        )
        .expect("embedded direct-read RAR fixture")
}

fn check_inspection_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "RAR inspection cancelled",
        ))
    } else {
        Ok(())
    }
}

pub fn enumerate_image_entries_detailed(path: &Path) -> io::Result<ZipEnumeration> {
    let (mut archive, _, resolved_path) = open_listing_from_volume(path)?;
    let mtime = std::fs::metadata(&resolved_path)
        .ok()
        .map_or(0, |m| crate::ui_helpers::mtime_secs(&m));
    let mut entries = Vec::new();
    let mut seen_names = HashSet::new();
    for entry in archive.by_ref() {
        let entry = entry.map_err(unrar_io)?;
        if !entry.is_file() {
            continue;
        }
        let name = normalized_entry_name(&entry.filename);
        let Some(entry_name) = deduplicated_visible_image_name(&name, &mut seen_names) else {
            continue;
        };
        entries.push(ZipImageEntry {
            entry_name,
            uncompressed_size: entry.unpacked_size,
            mtime,
        });
    }
    Ok(ZipEnumeration {
        entries,
        has_foreign_archives: false,
        legacy_renames: Vec::new(),
    })
}

pub(crate) fn enumerate_image_entries_detailed_traced(
    path: &Path,
    input_seq: u64,
) -> io::Result<ZipEnumeration> {
    if !crate::perf::is_enabled() {
        return enumerate_image_entries_detailed(path);
    }
    let started = std::time::Instant::now();
    let archive_key = crate::path_key::normalize_keep_drive(path);
    crate::perf::event(
        "rar",
        "image_enumeration_begin",
        Some(&archive_key),
        input_seq,
        &[],
    );
    let result = enumerate_image_entries_detailed(path);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    match &result {
        Ok(enumeration) => crate::perf::event(
            "rar",
            "image_enumeration_end",
            Some(&archive_key),
            input_seq,
            &[
                ("ms", serde_json::Value::from(elapsed_ms)),
                (
                    "image_entries",
                    serde_json::Value::from(enumeration.entries.len()),
                ),
            ],
        ),
        Err(error) => crate::perf::event(
            "rar",
            "image_enumeration_error",
            Some(&archive_key),
            input_seq,
            &[
                ("ms", serde_json::Value::from(elapsed_ms)),
                (
                    "error_kind",
                    serde_json::Value::from(format!("{:?}", error.kind())),
                ),
            ],
        ),
    }
    result
}

pub fn first_image_entry(path: &Path, cancel: Option<&AtomicBool>) -> Option<String> {
    let (mut archive, _, _) = open_listing_from_volume(path).ok()?;
    for entry in archive.by_ref() {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return None;
        }
        let entry = entry.ok()?;
        let name = normalized_entry_name(&entry.filename);
        if entry.is_file() && !should_ignore_entry(&name) && is_image_entry(&name) {
            return Some(name);
        }
    }
    None
}

pub fn read_first_image_bytes(path: &Path) -> Option<(String, Vec<u8>)> {
    let (resolved_path, _) = resolved_volume_path(path).ok()?;
    let mut archive = unrar::Archive::new(&resolved_path)
        .open_for_processing()
        .ok()?;
    loop {
        let header = archive.read_header().ok()??;
        let entry = header.entry();
        let name = normalized_entry_name(&entry.filename);
        let should_read = entry.is_file()
            && !should_ignore_entry(&name)
            && is_image_entry(&name)
            && entry.unpacked_size <= MAX_DIRECT_ENTRY_BYTES;
        if should_read {
            let (bytes, _next) = header.read().ok()?;
            return Some((name, bytes));
        }
        archive = header.skip().ok()?;
    }
}

pub fn read_entry_bytes(path: &Path, wanted: &str) -> io::Result<Vec<u8>> {
    let wanted = wanted.replace('\\', "/");
    let (resolved_path, _) = resolved_volume_path(path)?;
    let mut archive = unrar::Archive::new(&resolved_path)
        .open_for_processing()
        .map_err(unrar_io)?;
    let mut seen_names = HashSet::new();
    loop {
        let Some(header) = archive.read_header().map_err(unrar_io)? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("RAR entry not found: {wanted}"),
            ));
        };
        let entry = header.entry();
        let name = normalized_entry_name(&entry.filename);
        let resolved_name = entry
            .is_file()
            .then(|| deduplicated_visible_image_name(&name, &mut seen_names))
            .flatten();
        if resolved_name.as_deref() == Some(wanted.as_str()) {
            if entry.unpacked_size > MAX_DIRECT_ENTRY_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "RAR entry exceeds the direct-read size limit",
                ));
            }
            return header.read().map(|(bytes, _)| bytes).map_err(unrar_io);
        }
        archive = header.skip().map_err(unrar_io)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multipart_filename_fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/archives/rar-multipart-filename-regression")
    }

    #[test]
    fn direct_read_classifier_accepts_only_flat_non_solid_unencrypted_rar() {
        assert_eq!(
            classify_direct_read(false, false, false),
            RarDirectReadDecision::Direct
        );
        assert_eq!(
            classify_direct_read(true, false, false),
            RarDirectReadDecision::Solid
        );
        assert_eq!(
            classify_direct_read(false, true, false),
            RarDirectReadDecision::NestedArchive
        );
        assert_eq!(
            classify_direct_read(false, false, true),
            RarDirectReadDecision::Encrypted
        );
    }

    #[test]
    fn duplicate_rar_entry_names_are_unique_and_read_the_matching_header() {
        let bytes = direct_read_test_fixture_bytes();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("duplicate-names.rar");
        std::fs::write(&path, bytes).unwrap();

        let enumeration = enumerate_image_entries_detailed(&path).unwrap();
        let names: Vec<&str> = enumeration
            .entries
            .iter()
            .map(|entry| entry.entry_name.as_str())
            .collect();
        assert_eq!(names, ["page.png", "page (2).png"]);

        let first = read_entry_bytes(&path, "page.png").unwrap();
        let second = read_entry_bytes(&path, "page (2).png").unwrap();
        assert_eq!(first.len(), 165);
        assert_eq!(second.len(), 188);
        assert_ne!(first, second);
    }

    #[test]
    fn ambiguous_numeric_names_are_single_archives_by_header() {
        let root = multipart_filename_fixture_root();
        for name in [
            "○×△□ Vol.1.rar",
            "○×△□ Vol.2.rar",
            "○×△□ Vol.2a.rar",
            "○×△□ Vol.10.rar",
            "○×△□ Vol.10a.rar",
            "○×△□ Vol.123.rar",
            "○×△□ Vol.１.rar",
        ] {
            let path = root.join(name);
            let inspection = inspect_for_direct_read(&path)
                .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()));
            assert_eq!(inspection.volume_kind, RarVolumeKind::Single, "{name}");
            assert_eq!(inspection.resolved_path, path, "{name}");
            let entries = enumerate_image_entries_detailed(&path).unwrap();
            assert_eq!(entries.entries.len(), 1, "{name}");
            let bytes = read_entry_bytes(&path, &entries.entries[0].entry_name).unwrap();
            assert!(!bytes.is_empty(), "{name}");
        }
    }

    #[test]
    fn volume_resolution_probe_is_reused_by_listing_read_and_dfs_checks() {
        // Use a unique path so this assertion is independent of the process-wide caches and of
        // other RAR tests running in parallel. The filename deliberately looks like a later
        // volume while the header says this is a standalone archive.
        let source = multipart_filename_fixture_root().join("○×△□ Vol.2.rar");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("volume-cache Vol.2.rar");
        std::fs::copy(source, &path).unwrap();

        let inspection = inspect_for_direct_read(&path).unwrap();
        assert_eq!(inspection.volume_kind, RarVolumeKind::Single);

        let entries = enumerate_image_entries_detailed(&path).unwrap();
        assert_eq!(entries.entries.len(), 1);
        let bytes = read_entry_bytes(&path, &entries.entries[0].entry_name).unwrap();
        assert!(!bytes.is_empty());
        assert!(!is_subsequent_volume(&path).unwrap());

        let probe_count = VOLUME_RESOLUTION_PROBE_COUNTS
            .lock()
            .unwrap()
            .iter()
            .find(|(probed, _)| probed == &path)
            .map_or(0, |(_, count)| *count);
        assert_eq!(
            probe_count, 1,
            "inspection, entry reads, and DFS checks must share one volume-header probe"
        );
    }

    #[test]
    fn real_subsequent_volume_resolves_to_first_part_by_header() {
        let root = multipart_filename_fixture_root().join("real-split-control");
        let first = root.join("real-split-control.part1.rar");
        let second = root.join("real-split-control.part2.rar");

        let first_inspection = inspect_for_direct_read(&first).unwrap();
        assert_eq!(first_inspection.volume_kind, RarVolumeKind::First);
        assert_eq!(first_inspection.resolved_path, first);

        let second_inspection = inspect_for_direct_read(&second).unwrap();
        assert_eq!(second_inspection.volume_kind, RarVolumeKind::Subsequent);
        assert_eq!(second_inspection.resolved_path, first);
        assert!(is_subsequent_volume(&second).unwrap());

        let from_first = enumerate_image_entries_detailed(&first).unwrap();
        let from_second = enumerate_image_entries_detailed(&second).unwrap();
        let first_entries: Vec<_> = from_first
            .entries
            .iter()
            .map(|entry| (&entry.entry_name, entry.uncompressed_size))
            .collect();
        let second_entries: Vec<_> = from_second
            .entries
            .iter()
            .map(|entry| (&entry.entry_name, entry.uncompressed_size))
            .collect();
        assert_eq!(second_entries, first_entries);
        assert!(!from_first.entries.is_empty());
    }

    #[test]
    fn real_split_volume_can_be_read_concurrently_from_both_visible_parts() {
        use std::sync::{Arc, Barrier};

        let root = multipart_filename_fixture_root().join("real-split-control");
        let first = root.join("real-split-control.part1.rar");
        let second = root.join("real-split-control.part2.rar");
        let expected = read_first_image_bytes(&first).expect("fixture must contain an image");
        for _ in 0..8 {
            let paths = [first.clone(), first.clone(), second.clone(), second.clone()];
            let barrier = Arc::new(Barrier::new(paths.len()));
            let handles: Vec<_> = paths
                .into_iter()
                .map(|path| {
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        read_first_image_bytes(&path)
                    })
                })
                .collect();

            for handle in handles {
                assert_eq!(handle.join().unwrap(), Some(expected.clone()));
            }
        }
    }
}
