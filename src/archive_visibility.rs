//! Worker-local archive projection shared by aggregate producers. No persistent state or UI I/O.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::file_type_visibility::FileTypeVisibility;
use crate::grid_item::GridItem;

struct ArchiveProjection {
    known_members: HashSet<String>,
    visible_members: HashSet<String>,
    raw_tree: crate::zip_tree::ZipTree,
    visible_tree: crate::zip_tree::ZipTree,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn visibility_partial_manifest_with_uppercase_image_is_legacy_and_cancel_is_not() {
        let guard = crate::data_dir::TestDataDirGuard::new();
        let (source, cached) = legacy_conversion_fixture(guard.path());
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&cached).unwrap());
        for member in ["root.jpg", "PAGE.JPG"] {
            archive
                .start_file(member, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"image").unwrap();
        }
        let manifest = crate::archive_origin::ArchiveOriginManifest {
            version: 1,
            images: [("root.jpg".into(), Vec::new())].into_iter().collect(),
        };
        archive
            .start_file(
                crate::archive_origin::MANIFEST_ENTRY,
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        archive.finish().unwrap();
        record_conversion_fixture(&source, &cached);
        let policy = FileTypeVisibility {
            excluded_extensions: ["zip".into()].into_iter().collect(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let normalized_cached = PathBuf::from(crate::path_key::normalize_keep_drive(&cached));
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(!projection.allows_member(&normalized_cached, "PAGE.JPG"));
        assert!(!projection.allows_member(&normalized_cached, "root.jpg"));
        assert_eq!(
            projection.notice(),
            Some(ArchiveVisibilityNotice::LegacyConvertedCache)
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            crate::zip_loader::enumerate_converted_image_entries_detailed_readonly_with_cancel(
                &cached,
                Some(&cancel)
            )
            .unwrap_err()
            .kind(),
            std::io::ErrorKind::Interrupted
        );
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(projection.allows_member(&cached, "PAGE.JPG"));
        assert_eq!(projection.notice(), None);
        cancel.store(false, Ordering::Relaxed);
        regenerate_conversion_fixture(&source, &cached);
        let mut regenerated = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(regenerated.allows_member(&cached, "root.jpg"));
        assert!(!regenerated.allows_member(&cached, "chapter.zip/page.jpg"));
        assert_eq!(regenerated.notice(), None);
        // A DB-confirmed cache outside cache_root also uses actual manifest ancestry.
        let relocated = guard.path().join("relocated.zip");
        std::fs::copy(&cached, &relocated).unwrap();
        record_conversion_fixture(&source, &relocated);
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(!projection.allows_member(&relocated, "chapter.zip/page.jpg"));
        assert!(projection.allows_member(&relocated, "notes.zip/page.jpg"));
        assert_eq!(projection.notice(), None);
    }

    #[test]
    fn visibility_missing_manifest_projection_is_readonly_and_preserves_unknown_cache_refs() {
        let guard = crate::data_dir::TestDataDirGuard::new();
        let (source, cached) = legacy_conversion_fixture(guard.path());
        record_conversion_fixture(&source, &cached);
        let policy = FileTypeVisibility {
            excluded_extensions: ["zip".into()].into_iter().collect(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(!projection.allows_member(&cached, "root.jpg"));
        assert!(projection.notice().is_some());
        let conn = rusqlite::Connection::open(crate::archive_cache::db_path()).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT origin_manifest_version FROM converted_archives",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let unknown = crate::archive_cache::cache_root().join("unregistered.zip");
        std::fs::copy(&cached, &unknown).unwrap();
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(projection.allows_member(&unknown, "root.jpg"));
        assert_eq!(projection.notice(), None);
        std::fs::remove_file(&cached).unwrap();
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(projection.allows_member(&cached, "root.jpg"));
        assert_eq!(projection.notice(), None);
    }

    #[test]
    fn visibility_legacy_projection_hides_only_known_rows_and_restores_after_regeneration() {
        let guard = crate::data_dir::TestDataDirGuard::new();
        let (source, cached) = legacy_conversion_fixture(guard.path());
        let policy = FileTypeVisibility {
            excluded_categories: [crate::file_type_visibility::FileTypeCategory::Zip]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(!projection.allows_member(&cached, "root.jpg"));
        assert!(!projection.allows_member(&source, "chapter.zip/page.jpg"));
        assert!(projection.allows_member(&cached, "missing.jpg"));
        assert_eq!(
            projection.notice(),
            Some(ArchiveVisibilityNotice::LegacyConvertedCache)
        );
        assert_eq!(
            projection.notice().unwrap().message(),
            "古い変換キャッシュのため表示できない項目があります。本を開くか、キャッシュを作り直すと表示されます。"
        );
        let mut leaf_only = FileTypeVisibility::default();
        leaf_only.excluded_extensions.insert("png".into());
        assert!(
            ArchiveVisibilityProjection::new(&leaf_only, &cancel)
                .allows_member(&cached, "root.jpg")
        );
        assert!(
            ArchiveVisibilityProjection::new(&Default::default(), &cancel)
                .allows_member(&cached, "root.jpg")
        );
        regenerate_conversion_fixture(&source, &cached);
        let before = crate::archive_cache::ArchiveCacheDb::open_readonly()
            .unwrap()
            .list_all()
            .unwrap()[0]
            .last_access_at;
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(
            projection.allows_member(&cached, "root.jpg"),
            "outer ZIP format is not nested ancestry"
        );
        assert!(!projection.allows_member(&cached, "chapter.zip/page.jpg"));
        assert!(
            projection.allows_member(&cached, "notes.zip/page.jpg"),
            "literal directory is not a ZIP boundary"
        );
        assert_eq!(projection.notice(), None);
        assert_eq!(
            before,
            crate::archive_cache::ArchiveCacheDb::open_readonly()
                .unwrap()
                .list_all()
                .unwrap()[0]
                .last_access_at
        );
        std::fs::remove_file(&source).unwrap();
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(
            !projection.allows_member(&cached, "chapter.zip/page.jpg"),
            "the existing cache retains its actual ancestry without the original source"
        );
        assert!(projection.allows_member(&cached, "missing.jpg"));
        assert!(projection.allows_member(&source, "chapter.zip/page.jpg"));
        assert_eq!(projection.notice(), None);
    }

    #[test]
    fn visibility_direct_cache_proof_survives_missing_or_changed_source() {
        for manifest in [false, true] {
            for missing_source in [false, true] {
                let guard = crate::data_dir::TestDataDirGuard::new();
                let (source, cached) = legacy_conversion_fixture(guard.path());
                let source_bytes = std::fs::read(&source).unwrap();
                if manifest {
                    regenerate_conversion_fixture(&source, &cached);
                }
                if missing_source {
                    std::fs::remove_file(&source).unwrap();
                } else {
                    std::fs::write(&source, b"changed original archive").unwrap();
                }
                let policy = FileTypeVisibility {
                    excluded_extensions: ["zip".into()].into_iter().collect(),
                    ..Default::default()
                };
                let cancel = AtomicBool::new(false);
                let mut source_reference = ArchiveVisibilityProjection::new(&policy, &cancel);
                assert!(source_reference.allows_member(&source, "chapter.zip/page.jpg"));
                assert_eq!(source_reference.notice(), None);

                let canonical_cached = std::fs::canonicalize(&cached).unwrap();
                for cache_reference in [&cached, &canonical_cached] {
                    let mut direct = ArchiveVisibilityProjection::new(&policy, &cancel);
                    assert!(!direct.allows_member(cache_reference, "chapter.zip/page.jpg"));
                    assert_eq!(direct.allows_member(cache_reference, "root.jpg"), manifest);
                    assert_eq!(
                        direct.allows_member(cache_reference, "notes.zip/page.jpg"),
                        manifest
                    );
                    assert!(direct.allows_member(cache_reference, "missing.jpg"));
                    let directory = |prefix: &str| GridItem::ZipDir {
                        zip_path: cache_reference.clone(),
                        dir_prefix: prefix.into(),
                        is_archive: true,
                        representative: None,
                    };
                    assert!(!direct.allows_item(&directory("chapter.zip/")));
                    assert_eq!(direct.allows_item(&directory("notes.zip/")), manifest);
                    assert!(direct.allows_item(&directory("missing.zip/")));
                    assert_eq!(
                        direct.notice(),
                        (!manifest).then_some(ArchiveVisibilityNotice::LegacyConvertedCache)
                    );
                }

                std::fs::write(&source, &source_bytes).unwrap();
                regenerate_conversion_fixture(&source, &cached);
                let mut regenerated = ArchiveVisibilityProjection::new(&policy, &cancel);
                assert!(!regenerated.allows_member(&cached, "chapter.zip/page.jpg"));
                assert!(regenerated.allows_member(&cached, "root.jpg"));
                assert!(regenerated.allows_member(&cached, "notes.zip/page.jpg"));
                assert_eq!(regenerated.notice(), None);

                std::fs::remove_file(&cached).unwrap();
                let mut missing = ArchiveVisibilityProjection::new(&policy, &cancel);
                assert!(missing.allows_member(&cached, "chapter.zip/page.jpg"));
                assert!(missing.allows_item(&GridItem::ZipDir {
                    zip_path: cached.clone(),
                    dir_prefix: "chapter.zip/".into(),
                    is_archive: true,
                    representative: None,
                }));
                assert_eq!(missing.notice(), None);
            }
        }
    }

    #[test]
    fn visibility_direct_cache_reads_actual_proof_with_stale_database_metadata() {
        for manifest in [false, true] {
            let guard = crate::data_dir::TestDataDirGuard::new();
            let (source, cached) = legacy_conversion_fixture(guard.path());
            if manifest {
                regenerate_conversion_fixture(&source, &cached);
            }
            let conn = rusqlite::Connection::open(crate::archive_cache::db_path()).unwrap();
            conn.execute(
                "UPDATE converted_archives SET origin_manifest_version=0, cached_zip_size=cached_zip_size+1",
                [],
            )
            .unwrap();
            let policy = FileTypeVisibility {
                excluded_extensions: ["zip".into()].into_iter().collect(),
                ..Default::default()
            };
            let cancel = AtomicBool::new(false);
            let mut direct = ArchiveVisibilityProjection::new(&policy, &cancel);
            assert!(!direct.allows_member(&cached, "chapter.zip/page.jpg"));
            assert_eq!(direct.allows_member(&cached, "root.jpg"), manifest);
            assert_eq!(
                direct.allows_member(&cached, "notes.zip/page.jpg"),
                manifest
            );
            assert!(direct.allows_member(&cached, "missing.jpg"));
            assert_eq!(
                direct.notice(),
                (!manifest).then_some(ArchiveVisibilityNotice::LegacyConvertedCache)
            );
            let mut source_reference = ArchiveVisibilityProjection::new(&policy, &cancel);
            assert!(source_reference.allows_member(&source, "chapter.zip/page.jpg"));
            assert_eq!(source_reference.notice(), None);
        }
    }

    #[test]
    fn aggregate_members_share_actual_ancestry_and_preserve_unknown_references() {
        let _guard = crate::data_dir::TestDataDirGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("outer.cbz");
        let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        inner
            .start_file("page.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        inner.write_all(b"nested image").unwrap();
        let bytes = inner.finish().unwrap().into_inner();
        let mut outer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        outer
            .start_file("chapter.zip", zip::write::SimpleFileOptions::default())
            .unwrap();
        outer.write_all(&bytes).unwrap();
        outer
            .start_file(
                "notes.zip/page.png",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        outer.write_all(b"directory image").unwrap();
        outer
            .start_file("root.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        outer.write_all(b"root image").unwrap();
        outer.finish().unwrap();
        let policy = FileTypeVisibility {
            excluded_extensions: ["zip".into(), "png".into()].into_iter().collect(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let mut projection = ArchiveVisibilityProjection::new(&policy, &cancel);
        assert!(!projection.allows_member(&path, "chapter.zip/page.png"));
        assert!(projection.allows_member(&path, "root.jpg"));
        let directory = |prefix: &str| GridItem::ZipDir {
            zip_path: path.clone(),
            dir_prefix: prefix.into(),
            is_archive: true,
            representative: None,
        };
        assert!(!projection.allows_item(&directory("chapter.zip/")));
        assert!(
            projection.allows_item(&directory("notes.zip/")),
            "physical directory remains a passage even after its image is hidden"
        );
        assert_eq!(projection.containers.len(), 1);
        assert!(projection.allows_member(&path, "missing.jpg"));
        assert!(
            projection.allows_member(&tmp.path().join("missing.cbz"), "chapter.zip/missing.jpg")
        );
        assert!(!projection.allows_member(&tmp.path().join("missing.cbz"), "missing.png"));
        assert_eq!(projection.containers.len(), 2);
    }
}

/// A projection warning travels with the worker result, never with persistent registrations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveVisibilityNotice {
    LegacyConvertedCache,
}

impl ArchiveVisibilityNotice {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::LegacyConvertedCache => {
                "古い変換キャッシュのため表示できない項目があります。本を開くか、キャッシュを作り直すと表示されます。"
            }
        }
    }
}

enum ArchiveProjectionStatus {
    Available(ArchiveProjection),
    Legacy {
        known_members: HashSet<String>,
        known_prefixes: HashSet<String>,
    },
    Unknown,
}

/// A source address can borrow only its current conversion, while a direct cache
/// reference owns the existing ZIP independently of the original archive.
enum ConvertedArchiveReference {
    CachedZip(crate::archive_cache::ConvertedArchiveSnapshot),
    Source(crate::archive_cache::ConvertedArchiveSnapshot),
}

impl ConvertedArchiveReference {
    fn snapshot(&self) -> &crate::archive_cache::ConvertedArchiveSnapshot {
        match self {
            Self::CachedZip(snapshot) | Self::Source(snapshot) => snapshot,
        }
    }

    fn source_mapping_is_current(&self) -> bool {
        match self {
            // The direct ZIP's successful enumeration proves its members and origins.
            // Original-source and recorded-cache stamps do not own that reference.
            Self::CachedZip(_) => true,
            Self::Source(snapshot) => snapshot.is_current(),
        }
    }
}

/// One immutable policy and enumeration per distinct container for a single worker build.
pub(crate) struct ArchiveVisibilityProjection<'a> {
    policy: &'a FileTypeVisibility,
    cancel: &'a AtomicBool,
    containers: HashMap<PathBuf, ArchiveProjectionStatus>,
    cache: Option<crate::archive_cache::ArchiveCacheDb>,
    notice: Option<ArchiveVisibilityNotice>,
}

impl<'a> ArchiveVisibilityProjection<'a> {
    pub(crate) fn new(policy: &'a FileTypeVisibility, cancel: &'a AtomicBool) -> Self {
        Self {
            policy,
            cancel,
            containers: HashMap::new(),
            notice: None,
            cache: policy
                .requires_archive_origins()
                .then(|| crate::archive_cache::ArchiveCacheDb::open_readonly().ok())
                .flatten(),
        }
    }

    pub(crate) fn notice(&self) -> Option<ArchiveVisibilityNotice> {
        self.notice
    }

    fn projection(&mut self, path: &Path) -> &ArchiveProjectionStatus {
        if !self.containers.contains_key(path) {
            let status = self.build_projection(path);
            self.containers.insert(path.to_path_buf(), status);
        }
        &self.containers[path]
    }

    fn build_projection(&self, path: &Path) -> ArchiveProjectionStatus {
        let reference = self.cache.as_ref().and_then(|db| {
            db.snapshot_for_cached_zip(path)
                .map(ConvertedArchiveReference::CachedZip)
                .or_else(|| {
                    db.snapshot_for_source(path)
                        .map(ConvertedArchiveReference::Source)
                })
        });
        let path = reference.as_ref().map_or(path, |reference| {
            reference.snapshot().cached_zip_path.as_path()
        });
        if reference
            .as_ref()
            .is_some_and(|reference| !reference.source_mapping_is_current())
        {
            return ArchiveProjectionStatus::Unknown;
        }
        if self.cancel.load(Ordering::Relaxed) {
            return ArchiveProjectionStatus::Unknown;
        }
        // The DB proves conversion identity; the existing cache proves its ancestry.
        // Read the actual manifest even if the DB predates it. Missing/invalid origins
        // only inspect literal metadata and never fabricate container ancestry.
        let enumeration = if reference.is_some() {
            crate::zip_loader::enumerate_converted_image_entries_detailed_readonly_with_cancel(
                path,
                Some(self.cancel),
            )
        } else {
            crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(
                path,
                Some(self.cancel),
            )
        };
        let enumeration = match enumeration {
            Ok(enumeration) => enumeration,
            Err(error)
                if error.kind() == std::io::ErrorKind::InvalidData
                    && !self.cancel.load(Ordering::Relaxed) =>
            {
                return reference
                    .as_ref()
                    .map_or(ArchiveProjectionStatus::Unknown, |reference| {
                        self.legacy_projection(path, reference)
                    });
            }
            Err(_) => return ArchiveProjectionStatus::Unknown,
        };
        if reference
            .as_ref()
            .is_some_and(|reference| !reference.source_mapping_is_current())
        {
            return ArchiveProjectionStatus::Unknown;
        }
        let known_members = enumeration
            .entries
            .iter()
            .map(|entry| entry.entry_name.clone())
            .collect();
        let visible_members = enumeration
            .entries
            .iter()
            .filter(|entry| {
                self.policy
                    .allows_physical_file(Path::new(&entry.entry_name))
                    && entry
                        .container_ancestry
                        .iter()
                        .all(|origin| origin.allows(self.policy))
            })
            .map(|entry| entry.entry_name.clone())
            .collect();
        let raw_tree = crate::zip_tree::ZipTree::build(path.to_path_buf(), enumeration.entries);
        let visible_tree = raw_tree.project(self.policy);
        ArchiveProjectionStatus::Available(ArchiveProjection {
            known_members,
            visible_members,
            raw_tree,
            visible_tree,
        })
    }

    fn legacy_projection(
        &self,
        path: &Path,
        reference: &ConvertedArchiveReference,
    ) -> ArchiveProjectionStatus {
        if self.cancel.load(Ordering::Relaxed) {
            return ArchiveProjectionStatus::Unknown;
        }
        let Ok(file) = std::fs::File::open(path) else {
            return ArchiveProjectionStatus::Unknown;
        };
        let Ok(archive) = zip::ZipArchive::new(file) else {
            return ArchiveProjectionStatus::Unknown;
        };
        let mut known_members = HashSet::new();
        let mut known_prefixes = HashSet::new();
        for member in archive.file_names() {
            if self.cancel.load(Ordering::Relaxed) {
                return ArchiveProjectionStatus::Unknown;
            }
            known_members.insert(member.to_owned());
            for (offset, _) in member.match_indices('/') {
                known_prefixes.insert(member[..=offset].to_owned());
            }
        }
        if self.cancel.load(Ordering::Relaxed) || !reference.source_mapping_is_current() {
            return ArchiveProjectionStatus::Unknown;
        }
        ArchiveProjectionStatus::Legacy {
            known_members,
            known_prefixes,
        }
    }

    pub(crate) fn allows_member(&mut self, path: &Path, member: &str) -> bool {
        if !self.policy.allows_physical_file(Path::new(member)) {
            return false;
        }
        if !self.policy.requires_archive_origins() {
            return true;
        }
        let (allows, legacy) = match self.projection(path) {
            ArchiveProjectionStatus::Available(projection) => (
                !projection.known_members.contains(member)
                    || projection.visible_members.contains(member),
                false,
            ),
            ArchiveProjectionStatus::Legacy { known_members, .. } => (
                !known_members.contains(member),
                known_members.contains(member),
            ),
            ArchiveProjectionStatus::Unknown => (true, false),
        };
        if legacy {
            self.notice = Some(ArchiveVisibilityNotice::LegacyConvertedCache);
        }
        allows
    }

    pub(crate) fn allows_bookmark(
        &mut self,
        bookmark: &crate::book_bookmarks::BookBookmark,
    ) -> bool {
        if !crate::bookmark_browser::book_bookmark_is_visible(self.policy, bookmark) {
            return false;
        }
        use crate::book_bookmarks::{BookContainerKind, PageIdentity};
        match (&bookmark.container_kind, &bookmark.page_identity) {
            (
                BookContainerKind::Zip | BookContainerKind::OtherArchive,
                PageIdentity::ArchiveEntry(member),
            ) => self.allows_member(&bookmark.container_path, member),
            _ => true,
        }
    }

    pub(crate) fn allows_item(&mut self, item: &GridItem) -> bool {
        if !self.policy.allows_grid_item(item) {
            return false;
        }
        match item {
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => self.allows_member(zip_path, entry_name),
            GridItem::ZipDir {
                zip_path,
                dir_prefix,
                ..
            } if self.policy.requires_archive_origins() => {
                let segments = dir_prefix
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let (allows, legacy) = match self.projection(zip_path) {
                    ArchiveProjectionStatus::Available(projection) => (
                        projection.raw_tree.node_at(&segments).is_none()
                            || projection.visible_tree.node_at(&segments).is_some(),
                        false,
                    ),
                    ArchiveProjectionStatus::Legacy { known_prefixes, .. } => (
                        !known_prefixes.contains(dir_prefix),
                        known_prefixes.contains(dir_prefix),
                    ),
                    ArchiveProjectionStatus::Unknown => (true, false),
                };
                if legacy {
                    self.notice = Some(ArchiveVisibilityNotice::LegacyConvertedCache);
                }
                allows
            }
            _ => true,
        }
    }
}

#[cfg(test)]
pub(crate) fn legacy_conversion_fixture(root: &Path) -> (PathBuf, PathBuf) {
    use std::io::Write;
    // Keep the outer book visible when the individual nested ZIP format is excluded.
    let source = root.join("converted-source.cbz");
    let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    inner
        .start_file("page.jpg", zip::write::SimpleFileOptions::default())
        .unwrap();
    inner.write_all(b"nested").unwrap();
    let inner = inner.finish().unwrap().into_inner();
    let mut outer = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
    for (name, bytes) in [
        ("chapter.zip", inner.as_slice()),
        ("notes.zip/page.jpg", b"directory"),
        ("root.jpg", b"root"),
    ] {
        outer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        outer.write_all(bytes).unwrap();
    }
    outer.finish().unwrap();
    let db = crate::archive_cache::ArchiveCacheDb::open().unwrap();
    let cached = db.reserve_cache_zip_path(&source).unwrap();
    let mut legacy = zip::ZipWriter::new(std::fs::File::create(&cached).unwrap());
    for name in ["chapter.zip/page.jpg", "notes.zip/page.jpg", "root.jpg"] {
        legacy
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        legacy.write_all(b"legacy").unwrap();
    }
    legacy.finish().unwrap();
    record_conversion_fixture(&source, &cached);
    rusqlite::Connection::open(crate::archive_cache::db_path())
        .unwrap()
        .execute(
            "UPDATE converted_archives SET origin_manifest_version=0",
            [],
        )
        .unwrap();
    (source, cached)
}

#[cfg(test)]
fn record_conversion_fixture(source: &Path, cached: &Path) {
    let source_meta = std::fs::metadata(source).unwrap();
    crate::archive_cache::ArchiveCacheDb::open()
        .unwrap()
        .record(
            source,
            crate::ui_helpers::mtime_secs(&source_meta),
            source_meta.len() as i64,
            crate::archive_converter::ArchiveFormat::Zip,
            cached,
            std::fs::metadata(cached).unwrap().len() as i64,
            3,
            false,
        )
        .unwrap();
}

#[cfg(test)]
pub(crate) fn regenerate_conversion_fixture(source: &Path, cached: &Path) {
    crate::archive_converter::convert_to_zip(
        source,
        cached,
        crate::archive_converter::ArchiveFormat::Zip,
        &AtomicBool::new(false),
        None,
    )
    .unwrap();
    record_conversion_fixture(source, cached);
}
