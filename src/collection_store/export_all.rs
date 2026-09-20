use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::settings::GridDisplayOrder;

use super::{
    CollectionAllExportSnapshot, CollectionPrepareError, prepare_collection_export,
    serialize_collection_paths,
};

const INDEX_NAME: &str = "collections-index.txt";

#[derive(Debug)]
pub(crate) enum CollectionAllExportFailure {
    Cancelled {
        incomplete: Option<PathBuf>,
    },
    Failed {
        incomplete: Option<PathBuf>,
        message: String,
    },
}

impl CollectionAllExportFailure {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Cancelled { incomplete: None } => "一括書き出しを取り消しました。".into(),
            Self::Cancelled {
                incomplete: Some(path),
            } => format!(
                "一括書き出しを取り消しました。未完成のフォルダ: {}",
                path.display()
            ),
            Self::Failed {
                incomplete: None,
                message,
            } => {
                format!("一括書き出しに失敗しました: {message}")
            }
            Self::Failed {
                incomplete: Some(path),
                message,
            } => format!(
                "一括書き出しに失敗しました: {message}。未完成のフォルダ: {}",
                path.display()
            ),
        }
    }
}

/// `parent` is user-selected. Only a newly created child directory is ever written, and no
/// existing file is replaced. Its index is published last and marks a complete export.
pub(crate) fn write_all_collections_export(
    parent: &Path,
    bundle: &CollectionAllExportSnapshot,
    display_order: &GridDisplayOrder,
    cancel: &AtomicBool,
    progress: &(AtomicUsize, AtomicUsize),
) -> Result<PathBuf, CollectionAllExportFailure> {
    write_all_collections_export_with(parent, bundle, display_order, cancel, progress, |_, _| {
        Ok(())
    })
}

fn write_all_collections_export_with(
    parent: &Path,
    bundle: &CollectionAllExportSnapshot,
    display_order: &GridDisplayOrder,
    cancel: &AtomicBool,
    progress: &(AtomicUsize, AtomicUsize),
    mut before_file: impl FnMut(usize, &Path) -> std::io::Result<()>,
) -> Result<PathBuf, CollectionAllExportFailure> {
    let fail = |message: String, incomplete: Option<PathBuf>| CollectionAllExportFailure::Failed {
        incomplete,
        message,
    };
    if cancel.load(Ordering::Acquire) {
        return Err(CollectionAllExportFailure::Cancelled { incomplete: None });
    }
    if bundle.catalog.definitions.len() != bundle.snapshots.len() {
        return Err(fail("actor snapshot count changed".into(), None));
    }
    let folder =
        create_unique_export_folder(parent).map_err(|error| fail(error.to_string(), None))?;
    let incomplete = || Some(folder.clone());
    progress.1.store(bundle.snapshots.len(), Ordering::Release);
    let mut used_names = HashSet::from([INDEX_NAME.to_lowercase()]);
    let mut index = String::from(
        "# mImageViewer collections export\r\n# position\tuuid\tname\torder_mode\tstandard_sort\tentries\tfile\r\n",
    );
    for (position, snapshot) in bundle.snapshots.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionAllExportFailure::Cancelled {
                incomplete: incomplete(),
            });
        }
        // The actor bundle is internally coherent; a mismatched order would be a programmer
        // error, never a reason to silently omit a collection.
        if bundle.catalog.definitions.get(position) != Some(&snapshot.definition)
            || snapshot.catalog_revision != bundle.catalog.catalog_revision
        {
            return Err(fail("actor snapshot order changed".into(), incomplete()));
        }
        let prepared = prepare_collection_export(snapshot, display_order, cancel, |_, _| {})
            .map_err(|error| match error {
                CollectionPrepareError::Cancelled => CollectionAllExportFailure::Cancelled {
                    incomplete: incomplete(),
                },
                other => fail(other.to_string(), incomplete()),
            })?;
        let filename = unique_text_filename(&snapshot.definition.name, &mut used_names);
        let path = folder.join(&filename);
        before_file(position, &path).map_err(|error| fail(error.to_string(), incomplete()))?;
        let contents =
            serialize_collection_paths(prepared.ordered_paths.iter().map(PathBuf::as_path));
        write_new_file(&path, contents.as_bytes())
            .map_err(|error| fail(error.to_string(), incomplete()))?;
        index.push_str(&format!(
            "{}\t{}\t{}\t{}\t{:?}\t{}\t{}\r\n",
            position + 1,
            snapshot.collection_id(),
            escape_index_field(&snapshot.definition.name),
            snapshot.definition.order_mode.as_str(),
            snapshot.definition.standard_sort,
            prepared.ordered_paths.len(),
            filename,
        ));
        progress.0.store(position + 1, Ordering::Release);
    }
    if cancel.load(Ordering::Acquire) {
        return Err(CollectionAllExportFailure::Cancelled {
            incomplete: incomplete(),
        });
    }
    index.push_str("# complete\r\n");
    // Publish the complete sibling index with no-replace rename on Windows. A crash or failure
    // before this point leaves only an unindexed partial folder; external files are not removed.
    let pending = folder.join(format!("{INDEX_NAME}.pending-{}", uuid::Uuid::new_v4()));
    write_new_file(&pending, index.as_bytes())
        .map_err(|error| fail(error.to_string(), incomplete()))?;
    if cancel.load(Ordering::Acquire) {
        let _ = fs::remove_file(&pending);
        return Err(CollectionAllExportFailure::Cancelled {
            incomplete: incomplete(),
        });
    }
    if let Err(error) =
        publish_index_no_replace(&pending, &folder.join(INDEX_NAME), index.as_bytes())
    {
        let _ = fs::remove_file(&pending);
        return Err(fail(error.to_string(), incomplete()));
    }
    // The published index is complete even if removal of its owned temporary file fails.
    let _ = fs::remove_file(&pending);
    Ok(folder)
}

#[cfg(windows)]
fn publish_index_no_replace(
    pending: &Path,
    final_path: &Path,
    _bytes: &[u8],
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    use windows::core::PCWSTR;

    let source: Vec<u16> = pending.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = final_path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
fn publish_index_no_replace(
    pending: &Path,
    final_path: &Path,
    _bytes: &[u8],
) -> std::io::Result<()> {
    // POSIX link publishes the already-synced inode atomically without replacing an existing
    // name. If this filesystem cannot link, fail with an unindexed partial folder rather than
    // expose a final index whose write/sync might be incomplete.
    fs::hard_link(pending, final_path)
}

fn create_unique_export_folder(parent: &Path) -> std::io::Result<PathBuf> {
    for suffix in 0..10_000 {
        let name = if suffix == 0 {
            "collections-export".to_owned()
        } else {
            format!("collections-export-{suffix}")
        };
        let folder = parent.join(name);
        match fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no unused export folder name",
    ))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn unique_text_filename(name: &str, used: &mut HashSet<String>) -> String {
    let mut stem = sanitized_stem(name);
    if stem.is_empty() {
        stem = "collection".into();
    }
    for suffix in 0..usize::MAX {
        let candidate = if suffix == 0 {
            format!("{stem}.txt")
        } else {
            format!("{stem}-{suffix}.txt")
        };
        if used.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("finite collection catalog cannot exhaust filenames")
}

fn sanitized_stem(name: &str) -> String {
    let mut output = String::new();
    let mut units = 0;
    for ch in name.chars() {
        let safe = if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
            '_'
        } else {
            ch
        };
        let next = safe.len_utf16();
        if units + next > 100 {
            break;
        }
        units += next;
        output.push(safe);
    }
    let output = output.trim_end_matches([' ', '.']).trim().to_owned();
    let base = output
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    if matches!(
        base.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CONIN$"
            | "CONOUT$"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "COM¹"
            | "COM²"
            | "COM³"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
            | "LPT¹"
            | "LPT²"
            | "LPT³"
    ) {
        format!("_{output}")
    } else {
        output
    }
}

fn escape_index_field(input: &str) -> String {
    input
        .chars()
        .flat_map(|ch| match ch {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\n' => "\\n".chars().collect(),
            ch if ch.is_control() => format!("\\u{:04X}", ch as u32).chars().collect(),
            ch => vec![ch],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection_store::{
        CollectionCatalogSnapshot, CollectionDefinition, CollectionEntry, CollectionEntryId,
        CollectionId, CollectionOrderMode, CollectionResolvedKind, CollectionSnapshot,
        CollectionSourcePath,
    };
    use crate::settings::SortOrder;
    use std::sync::Arc;

    fn bundle(names: &[&str]) -> CollectionAllExportSnapshot {
        let mut definitions = Vec::new();
        let mut snapshots = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let definition = CollectionDefinition {
                id: CollectionId::new(),
                name: (*name).into(),
                order_mode: CollectionOrderMode::Manual,
                standard_sort: SortOrder::FileName,
                shuffle_seed: 1,
                revision: 1,
            };
            let entries = if index == 0 {
                let path = PathBuf::from(r"C:\absent\missing image.png");
                let source = CollectionSourcePath::from_trusted(&path).unwrap();
                vec![CollectionEntry {
                    id: CollectionEntryId::new(),
                    collection_id: definition.id,
                    source_path: path,
                    source_key: source.key().clone(),
                    resolved_kind: CollectionResolvedKind::Image,
                    manual_position: 0,
                }]
            } else {
                Vec::new()
            };
            snapshots.push(CollectionSnapshot {
                catalog_revision: 9,
                definition: definition.clone(),
                entries: Arc::from(entries),
            });
            definitions.push(definition);
        }
        CollectionAllExportSnapshot {
            catalog: CollectionCatalogSnapshot {
                catalog_revision: 9,
                definitions: Arc::from(definitions),
            },
            snapshots,
        }
    }

    #[test]
    fn names_are_windows_safe_case_unique_and_index_marks_complete_export() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("collections-export")).unwrap();
        let bundle = bundle(&[
            "CON",
            "same",
            "SAME",
            "collections-index",
            "a/b:*?",
            "空",
            "line\tbreak",
        ]);
        let progress = (AtomicUsize::new(0), AtomicUsize::new(0));
        let folder = write_all_collections_export(
            temp.path(),
            &bundle,
            &GridDisplayOrder::default(),
            &AtomicBool::new(false),
            &progress,
        )
        .unwrap();
        assert_eq!(folder.file_name().unwrap(), "collections-export-1");
        let files = fs::read_dir(&folder)
            .unwrap()
            .map(|row| row.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), bundle.snapshots.len() + 1);
        assert!(files.iter().any(|name| name == "_CON.txt"));
        assert!(files.iter().any(|name| name == "same.txt"));
        assert!(files.iter().any(|name| name == "SAME-1.txt"));
        assert!(files.iter().any(|name| name == "collections-index-1.txt"));
        assert!(files.iter().any(|name| name == INDEX_NAME));
        let index = fs::read_to_string(folder.join(INDEX_NAME)).unwrap();
        assert!(index.contains("line\\tbreak"));
        assert!(index.contains("\tmanual\tFileName\t1\t"));
        assert_eq!(index.lines().count(), bundle.snapshots.len() + 3);
        assert!(index.ends_with("# complete\r\n"));
        assert_eq!(progress.0.load(Ordering::Acquire), bundle.snapshots.len());
        assert!(
            fs::read_to_string(folder.join("_CON.txt"))
                .unwrap()
                .contains("missing image.png")
        );
        assert!(
            fs::read_to_string(folder.join("same.txt"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cancellation_and_failure_leave_explicit_unindexed_partial_folder() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = bundle(&["first", "second"]);
        let progress = (AtomicUsize::new(0), AtomicUsize::new(0));
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            write_all_collections_export(
                temp.path(),
                &bundle,
                &GridDisplayOrder::default(),
                &cancelled,
                &progress
            ),
            Err(CollectionAllExportFailure::Cancelled { incomplete: None })
        ));
        assert!(!temp.path().join("collections-export").exists());

        let cancelled = AtomicBool::new(false);
        let failure = write_all_collections_export_with(
            temp.path(),
            &bundle,
            &GridDisplayOrder::default(),
            &cancelled,
            &progress,
            |index, _| {
                if index == 1 {
                    Err(std::io::Error::other("injected failure"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        let CollectionAllExportFailure::Failed {
            incomplete: Some(folder),
            message,
        } = failure
        else {
            panic!("failure must report partial folder")
        };
        assert!(message.contains("injected failure"));
        assert!(folder.join("first.txt").exists());
        assert!(!folder.join(INDEX_NAME).exists());

        let cancelled = AtomicBool::new(false);
        let failure = write_all_collections_export_with(
            temp.path(),
            &bundle,
            &GridDisplayOrder::default(),
            &cancelled,
            &progress,
            |index, _| {
                if index == 1 {
                    cancelled.store(true, Ordering::Release);
                }
                Ok(())
            },
        )
        .unwrap_err();
        let CollectionAllExportFailure::Cancelled {
            incomplete: Some(folder),
        } = failure
        else {
            panic!("cancel must report partial folder")
        };
        assert!(folder.join("first.txt").exists());
        assert!(!folder.join(INDEX_NAME).exists());
    }

    #[test]
    fn index_publication_never_overwrites_an_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let pending = temp.path().join("pending.txt");
        let final_path = temp.path().join(INDEX_NAME);
        write_new_file(&pending, b"complete").unwrap();
        write_new_file(&final_path, b"external").unwrap();
        assert!(publish_index_no_replace(&pending, &final_path, b"complete").is_err());
        assert_eq!(fs::read(&final_path).unwrap(), b"external");
        assert_eq!(fs::read(&pending).unwrap(), b"complete");
    }

    #[test]
    fn empty_export_and_long_names_have_complete_bounded_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let progress = (AtomicUsize::new(0), AtomicUsize::new(0));
        let empty = bundle(&[]);
        let folder = write_all_collections_export(
            temp.path(),
            &empty,
            &GridDisplayOrder::default(),
            &AtomicBool::new(false),
            &progress,
        )
        .unwrap();
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        assert!(
            fs::read_to_string(folder.join(INDEX_NAME))
                .unwrap()
                .ends_with("# complete\r\n")
        );

        let long = "あ".repeat(200);
        let second = bundle(&[&long]);
        let folder = write_all_collections_export(
            temp.path(),
            &second,
            &GridDisplayOrder::default(),
            &AtomicBool::new(false),
            &progress,
        )
        .unwrap();
        let filename = fs::read_dir(&folder)
            .unwrap()
            .map(|row| row.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name != INDEX_NAME)
            .unwrap();
        assert!(filename.encode_utf16().count() <= 104);
        assert!(folder.join(filename).exists());
        assert_eq!(sanitized_stem("CONIN$"), "_CONIN$");
        assert_eq!(sanitized_stem("COM¹.txt"), "_COM¹.txt");
        assert_eq!(sanitized_stem("CON .foo"), "_CON .foo");
    }
}
