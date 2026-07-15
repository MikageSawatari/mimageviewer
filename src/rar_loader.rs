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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RarDirectReadDecision {
    Direct,
    Solid,
    NestedArchive,
    Encrypted,
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

static DECISION_CACHE: LazyLock<Mutex<Vec<(DecisionCacheKey, RarInspection)>>> =
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

fn open_listing_from_volume(
    path: &Path,
) -> io::Result<(
    unrar::OpenArchive<unrar::List, unrar::CursorBeforeHeader>,
    RarVolumeKind,
    PathBuf,
)> {
    let archive = unrar::Archive::new(path);
    let first_part = archive.first_part();
    let opened = archive.open_for_listing().map_err(unrar_io)?;
    let volume_kind = RarVolumeKind::from(opened.volume_info());
    if volume_kind != RarVolumeKind::Subsequent {
        return Ok((opened, volume_kind, path.to_path_buf()));
    }
    drop(opened);
    let opened = unrar::Archive::new(&first_part)
        .open_for_listing()
        .map_err(unrar_io)?;
    Ok((opened, volume_kind, first_part))
}

fn resolved_volume_path(path: &Path) -> io::Result<(PathBuf, RarVolumeKind)> {
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
    Ok((resolved, volume_kind))
}

/// Header-backed decision for worker-side folder navigation. A malformed or unreadable RAR is
/// reported as an error so callers can conservatively keep it visible.
pub fn is_subsequent_volume(path: &Path) -> io::Result<bool> {
    resolved_volume_path(path).map(|(_, kind)| kind == RarVolumeKind::Subsequent)
}

/// Inspect a RAR once per `(path, mtime, size)` identity.
pub fn inspect_for_direct_read(path: &Path) -> io::Result<RarInspection> {
    let key = cache_key(path)?;
    if let Ok(cache) = DECISION_CACHE.lock()
        && let Some((_, inspection)) = cache.iter().find(|(cached, _)| cached == &key)
    {
        return Ok(inspection.clone());
    }
    let (mut archive, volume_kind, resolved_path) = open_listing_from_volume(path)?;
    let is_solid = archive.is_solid();
    let mut has_encrypted = archive.has_encrypted_headers();
    let mut image_count = 0u32;
    let mut total_uncompressed_bytes = 0u64;
    let mut nested_archive_count = 0u32;
    for entry in archive.by_ref() {
        let entry = entry.map_err(unrar_io)?;
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
            total_uncompressed_bytes = total_uncompressed_bytes.saturating_add(entry.unpacked_size);
        } else if nested_archive_kind(&name).is_some() {
            nested_archive_count = nested_archive_count.saturating_add(1);
        }
    }
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
        use base64::Engine;

        // Tiny non-solid RAR with two distinct PNG payloads whose stored names are both page.png.
        // The fixture is embedded so the test does not require rar.exe at runtime.
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                "UmFyIRoHAQAzkrXlCgEFBgAFAQGAgABdyPuwJAIDC6UBBKUBIL8JEzuAAAAIcGFnZS5wbmcKAwKX0YuaNf3cAYlQTkcNChoKAAAADUlIRFIAAABAAAAAMAgCAAAALinrSAAAAGxJREFUeJztz7EJgDAAAEEMYiGW7j9mCgtxjCP48APcb+8z9+tetzGOc+28oAEtaEALGtCCBrSgAS1oQAsa0IIGtKABLWhACxrQgga0oAEtaEALGtCCBrSgAS1oQAsa0IIGtKABLWhAC34+8AHOE9+qokthcQAAAABJRU5ErkJggo4TMhYkAgMLvAEEvAEgRG2V/YAAAAhwYWdlLnBuZwoDAgoki5o1/dwBiVBORw0KGgoAAAANSUhEUgAAAEAAAAAwCAIAAAAuKetIAAAAC3RFWHRwYXJhbWV0ZXJzAAmqaREAAABsSURBVHic7c+xCYAwAABBDGIhli6YeR3I1jGO4MMPcL89892ve93GOM6184IGtKABLWhACxrQgga0oAEtaEALGtCCBrSgAS1oQAsa0IIGtKABLWhACxrQgga0oAEtaEALGtCCBrSgAS34+cAHG8He3/HxQZMAAAAASUVORK5CYIIdd1ZRAwUEAA==",
            )
            .unwrap();
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
}
