use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    CollectionEntryId, CollectionRegistration, CollectionResolvedKind, CollectionSnapshot,
    CollectionSortFacts, CollectionStoreError, effective_collection_order,
};
use crate::settings::{GridDisplayOrder, GridItemDisplayKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollectionPreparedCategory {
    Display(GridItemDisplayKind),
    UnresolvedTail,
}

impl CollectionPreparedCategory {
    fn rank(self, display_order: &GridDisplayOrder) -> u8 {
        match self {
            Self::Display(kind) => display_order.row_for(kind) as u8,
            Self::UnresolvedTail => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CollectionSourcePreparation {
    Available {
        kind: CollectionResolvedKind,
        mtime: i64,
        file_size: Option<i64>,
    },
    Missing,
    Unsupported,
    AccessError(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollectionPreparedRegistration {
    pub(crate) path: PathBuf,
    pub(crate) result: Result<CollectionRegistration, CollectionPrepareError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollectionExportPreparation {
    pub(crate) collection_id: super::CollectionId,
    pub(crate) collection_revision: u64,
    pub(crate) ordered_paths: Arc<[PathBuf]>,
    pub(crate) source_states: Arc<[(CollectionEntryId, CollectionSourcePreparation)]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CollectionPrepareError {
    Cancelled,
    Unsupported(PathBuf),
    Access { path: PathBuf, message: String },
    Store(CollectionStoreError),
    Io(String),
}

impl std::fmt::Display for CollectionPrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("collection operation was cancelled"),
            Self::Unsupported(path) => {
                write!(f, "unsupported collection source: {}", path.display())
            }
            Self::Access { path, message } => {
                write!(
                    f,
                    "cannot inspect collection source {}: {message}",
                    path.display()
                )
            }
            Self::Store(error) => error.fmt(f),
            Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CollectionPrepareError {}

impl From<CollectionStoreError> for CollectionPrepareError {
    fn from(value: CollectionStoreError) -> Self {
        Self::Store(value)
    }
}

/// Confirm 後のworker用。pathごとのI/Oはここだけが行い、UIは結果を受け取る。
pub(crate) fn prepare_collection_registrations(
    paths: &[PathBuf],
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Vec<CollectionPreparedRegistration> {
    let total = paths.len();
    let mut prepared = Vec::with_capacity(total);
    for (index, path) in paths.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let result = match inspect_collection_source(path) {
            CollectionSourcePreparation::Available { kind, .. } => {
                CollectionRegistration::from_trusted_path(path, kind)
                    .map_err(CollectionPrepareError::Store)
            }
            CollectionSourcePreparation::Missing => {
                CollectionRegistration::from_trusted_path(path, CollectionResolvedKind::Unresolved)
                    .map_err(CollectionPrepareError::Store)
            }
            CollectionSourcePreparation::Unsupported => {
                Err(CollectionPrepareError::Unsupported(path.clone()))
            }
            CollectionSourcePreparation::AccessError(message) => {
                Err(CollectionPrepareError::Access {
                    path: path.clone(),
                    message,
                })
            }
        };
        prepared.push(CollectionPreparedRegistration {
            path: path.clone(),
            result,
        });
        progress(index + 1, total);
    }
    prepared
}

/// 最新DB snapshotからStandard/Manualの全件export順をworker内で解決する。
pub(crate) fn prepare_collection_export(
    snapshot: &CollectionSnapshot,
    display_order: &GridDisplayOrder,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Result<CollectionExportPreparation, CollectionPrepareError> {
    let total = snapshot.entries.len();
    let mut facts = Vec::with_capacity(total);
    let mut states = Vec::with_capacity(total);
    for (index, entry) in snapshot.entries.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionPrepareError::Cancelled);
        }
        let state = inspect_collection_source(&entry.source_path);
        let (category, mtime, file_size) = match &state {
            CollectionSourcePreparation::Available {
                kind,
                mtime,
                file_size,
            } => (category_for_kind(*kind), *mtime, *file_size),
            CollectionSourcePreparation::Missing
            | CollectionSourcePreparation::Unsupported
            | CollectionSourcePreparation::AccessError(_) => {
                (category_for_kind(entry.resolved_kind), 0, None)
            }
        };
        facts.push(CollectionSortFacts {
            entry_id: entry.id,
            category_rank: category.rank(display_order),
            name: entry
                .source_path
                .file_name()
                .unwrap_or_else(|| entry.source_path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            mtime,
            file_size,
        });
        states.push((entry.id, state));
        progress(index + 1, total);
    }
    let order = effective_collection_order(snapshot, &facts)?;
    let by_id = snapshot
        .entries
        .iter()
        .map(|entry| (entry.id, entry.source_path.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let ordered_paths = order
        .into_iter()
        .map(|id| {
            by_id
                .get(&id)
                .cloned()
                .ok_or(CollectionStoreError::InvalidOrder)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CollectionExportPreparation {
        collection_id: snapshot.collection_id(),
        collection_revision: snapshot.revision(),
        ordered_paths: Arc::from(ordered_paths),
        source_states: Arc::from(states),
    })
}

pub(crate) fn write_collection_export_atomic(
    destination: &Path,
    contents: &[u8],
    cancel: &AtomicBool,
) -> Result<(), CollectionPrepareError> {
    write_collection_export_atomic_with(destination, contents, cancel, replace_file_atomic)
}

fn write_collection_export_atomic_with(
    destination: &Path,
    contents: &[u8],
    cancel: &AtomicBool,
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), CollectionPrepareError> {
    if cancel.load(Ordering::Acquire) {
        return Err(CollectionPrepareError::Cancelled);
    }
    let parent = destination.parent().ok_or_else(|| {
        CollectionPrepareError::Io("export destination has no parent directory".into())
    })?;
    let file_name = destination
        .file_name()
        .ok_or_else(|| CollectionPrepareError::Io("export destination has no file name".into()))?;
    let temp_name = format!(
        ".{}.miv-collection-{}.tmp",
        file_name.to_string_lossy(),
        uuid::Uuid::new_v4()
    );
    let temp_path = parent.join(temp_name);
    let mut cleanup = TempExportFile(Some(temp_path.clone()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|error| CollectionPrepareError::Io(error.to_string()))?;
    file.write_all(contents)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| CollectionPrepareError::Io(error.to_string()))?;
    drop(file);
    if cancel.load(Ordering::Acquire) {
        return Err(CollectionPrepareError::Cancelled);
    }
    replace(&temp_path, destination)
        .map_err(|error| CollectionPrepareError::Io(error.to_string()))?;
    cleanup.0 = None;
    Ok(())
}

fn inspect_collection_source(path: &Path) -> CollectionSourcePreparation {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CollectionSourcePreparation::Missing;
        }
        Err(error) => return CollectionSourcePreparation::AccessError(error.to_string()),
    };
    let mtime = crate::ui_helpers::mtime_secs(&metadata);
    if metadata.is_dir() {
        return CollectionSourcePreparation::Available {
            kind: CollectionResolvedKind::Folder,
            mtime,
            file_size: None,
        };
    }
    if !metadata.is_file() {
        return CollectionSourcePreparation::Unsupported;
    }
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return CollectionSourcePreparation::Unsupported;
    };
    let extension = extension.to_ascii_lowercase();
    let kind = if crate::folder_tree::is_recognized_image_ext(&extension) {
        CollectionResolvedKind::Image
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        CollectionResolvedKind::Video
    } else if crate::folder_tree::is_audio_ext(&extension) {
        CollectionResolvedKind::Audio
    } else if crate::folder_tree::is_zip_extension(&extension) {
        CollectionResolvedKind::Zip
    } else if crate::folder_tree::is_pdf_extension(&extension) {
        CollectionResolvedKind::Pdf
    } else if crate::archive_converter::ArchiveFormat::from_extension(&extension).is_some() {
        CollectionResolvedKind::ConvertibleArchive
    } else {
        return CollectionSourcePreparation::Unsupported;
    };
    CollectionSourcePreparation::Available {
        kind,
        mtime,
        file_size: Some(metadata.len().min(i64::MAX as u64) as i64),
    }
}

fn category_for_kind(kind: CollectionResolvedKind) -> CollectionPreparedCategory {
    match kind {
        CollectionResolvedKind::Folder => {
            CollectionPreparedCategory::Display(GridItemDisplayKind::Folder)
        }
        CollectionResolvedKind::Zip
        | CollectionResolvedKind::Pdf
        | CollectionResolvedKind::ConvertibleArchive => {
            CollectionPreparedCategory::Display(GridItemDisplayKind::Archive)
        }
        CollectionResolvedKind::Image => {
            CollectionPreparedCategory::Display(GridItemDisplayKind::Image)
        }
        CollectionResolvedKind::Video | CollectionResolvedKind::Audio => {
            CollectionPreparedCategory::Display(GridItemDisplayKind::VideoAudio)
        }
        CollectionResolvedKind::Unresolved => CollectionPreparedCategory::UnresolvedTail,
    }
}

struct TempExportFile(Option<PathBuf>);

impl Drop for TempExportFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(windows)]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;
    let temp: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(temp.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, destination)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::collection_store::{
        CollectionDefinition, CollectionEntry, CollectionId, CollectionOrderMode,
        CollectionResolvedKind, CollectionSourcePath,
    };
    use crate::settings::SortOrder;

    fn entry(path: &Path, kind: CollectionResolvedKind, position: u64) -> CollectionEntry {
        let source = CollectionSourcePath::from_trusted(path).unwrap();
        CollectionEntry {
            id: CollectionEntryId::new(),
            collection_id: CollectionId::new(),
            source_path: source.path().to_path_buf(),
            source_key: source.key().clone(),
            resolved_kind: kind,
            manual_position: position,
        }
    }

    fn snapshot(
        entries: Vec<CollectionEntry>,
        mode: CollectionOrderMode,
        sort: SortOrder,
    ) -> CollectionSnapshot {
        let collection_id = CollectionId::new();
        let entries = entries
            .into_iter()
            .enumerate()
            .map(|(index, mut entry)| {
                entry.collection_id = collection_id;
                entry.manual_position = index as u64;
                entry
            })
            .collect::<Vec<_>>();
        CollectionSnapshot {
            catalog_revision: 4,
            definition: CollectionDefinition {
                id: collection_id,
                name: "Export".into(),
                order_mode: mode,
                standard_sort: sort,
                revision: 7,
            },
            entries: Arc::from(entries),
        }
    }

    #[test]
    fn classifier_preserves_sources_and_distinguishes_missing_and_unsupported() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("image.png");
        let unsupported = temp.path().join("notes.txt");
        let folder = temp.path().join("folder");
        let missing = temp.path().join("missing.jpg");
        std::fs::write(&image, b"source-bytes").unwrap();
        std::fs::write(&unsupported, b"not-media").unwrap();
        std::fs::create_dir(&folder).unwrap();

        let cancel = AtomicBool::new(false);
        let mut progress = Vec::new();
        let prepared = prepare_collection_registrations(
            &[
                image.clone(),
                folder.clone(),
                missing.clone(),
                unsupported.clone(),
            ],
            &cancel,
            |done, total| progress.push((done, total)),
        );

        assert_eq!(progress, vec![(1, 4), (2, 4), (3, 4), (4, 4)]);
        assert_eq!(std::fs::read(&image).unwrap(), b"source-bytes");
        assert_eq!(std::fs::read(&unsupported).unwrap(), b"not-media");
        assert_eq!(
            prepared[0].result.as_ref().unwrap().resolved_kind(),
            CollectionResolvedKind::Image
        );
        assert_eq!(
            prepared[1].result.as_ref().unwrap().resolved_kind(),
            CollectionResolvedKind::Folder
        );
        assert_eq!(
            prepared[2].result.as_ref().unwrap().resolved_kind(),
            CollectionResolvedKind::Unresolved
        );
        assert!(matches!(
            prepared[3].result,
            Err(CollectionPrepareError::Unsupported(ref path)) if path == &unsupported
        ));
    }

    #[test]
    fn classifier_cancel_never_publishes_unvisited_paths() {
        let cancel = AtomicBool::new(true);
        let mut progress_called = false;
        let prepared = prepare_collection_registrations(
            &[PathBuf::from(r"C:\never\visited.jpg")],
            &cancel,
            |_, _| progress_called = true,
        );
        assert!(prepared.is_empty());
        assert!(!progress_called);
    }

    #[test]
    fn standard_export_keeps_all_missing_and_uses_last_known_category_then_unresolved_tail() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        let image = temp.path().join("z-image.png");
        let known_missing = temp.path().join("a-missing.mp4");
        let unresolved = temp.path().join("0-unresolved.bin");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&image, b"image").unwrap();
        let snapshot = snapshot(
            vec![
                entry(&unresolved, CollectionResolvedKind::Unresolved, 0),
                entry(&image, CollectionResolvedKind::Image, 1),
                entry(&known_missing, CollectionResolvedKind::Video, 2),
                entry(&folder, CollectionResolvedKind::Folder, 3),
            ],
            CollectionOrderMode::Standard,
            SortOrder::FileName,
        );

        let prepared = prepare_collection_export(
            &snapshot,
            &GridDisplayOrder::default(),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(prepared.collection_id, snapshot.collection_id());
        assert_eq!(prepared.collection_revision, snapshot.revision());
        assert_eq!(prepared.ordered_paths.len(), 4);
        assert_eq!(prepared.ordered_paths[0], folder);
        assert_eq!(prepared.ordered_paths[1], known_missing);
        assert_eq!(prepared.ordered_paths[2], image);
        assert_eq!(prepared.ordered_paths[3], unresolved);
        assert!(matches!(
            prepared.source_states[0].1,
            CollectionSourcePreparation::Missing
        ));

        let custom_rows = GridDisplayOrder::from_rows([
            vec![GridItemDisplayKind::Image],
            vec![GridItemDisplayKind::VideoAudio],
            vec![GridItemDisplayKind::Folder],
            vec![GridItemDisplayKind::Archive],
        ]);
        let custom =
            prepare_collection_export(&snapshot, &custom_rows, &AtomicBool::new(false), |_, _| {})
                .unwrap();
        assert_eq!(
            custom.ordered_paths.as_ref(),
            [image, known_missing, folder, unresolved]
        );
    }

    #[test]
    fn standard_export_retains_every_entry_for_every_sort_and_manual_keeps_saved_order() {
        let temp = tempfile::tempdir().unwrap();
        let paths = [
            temp.path().join("missing-b.mp4"),
            temp.path().join("missing-a.png"),
            temp.path().join("missing-c.bin"),
        ];
        let base_entries = vec![
            entry(&paths[0], CollectionResolvedKind::Video, 0),
            entry(&paths[1], CollectionResolvedKind::Image, 1),
            entry(&paths[2], CollectionResolvedKind::Unresolved, 2),
        ];
        for &sort in SortOrder::all() {
            let snapshot = snapshot(base_entries.clone(), CollectionOrderMode::Standard, sort);
            let prepared = prepare_collection_export(
                &snapshot,
                &GridDisplayOrder::default(),
                &AtomicBool::new(false),
                |_, _| {},
            )
            .unwrap();
            let actual = prepared
                .ordered_paths
                .iter()
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(actual.len(), paths.len(), "{sort:?}");
            assert!(paths.iter().all(|path| actual.contains(path)), "{sort:?}");
        }

        let snapshot = snapshot(
            base_entries,
            CollectionOrderMode::Manual,
            SortOrder::SizeDesc,
        );
        let prepared = prepare_collection_export(
            &snapshot,
            &GridDisplayOrder::default(),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(prepared.ordered_paths.as_ref(), paths.as_slice());
    }

    #[test]
    fn atomic_export_replaces_only_after_success_and_cancel_preserves_destination() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("collection.txt");
        std::fs::write(&destination, b"old").unwrap();

        let cancel = AtomicBool::new(true);
        assert_eq!(
            write_collection_export_atomic(&destination, b"cancelled", &cancel),
            Err(CollectionPrepareError::Cancelled)
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"old");
        assert!(temp.path().read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp")
        }));

        let failure = write_collection_export_atomic_with(
            &destination,
            b"must-not-replace",
            &AtomicBool::new(false),
            |_, _| Err(std::io::Error::other("injected replace failure")),
        );
        assert!(matches!(failure, Err(CollectionPrepareError::Io(_))));
        assert_eq!(std::fs::read(&destination).unwrap(), b"old");
        assert!(temp.path().read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp")
        }));

        write_collection_export_atomic(&destination, b"new\r\n", &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new\r\n");
    }
}
