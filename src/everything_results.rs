//! Completed Everything result projection, called only by the common Search prepare worker.
//!
//! The reply is an indexed name snapshot, not proof that a path still names a readable file.
//! Recognize and apply the submitted policy before stat, then keep the successful stamps with
//! the immutable dataset for sort/facet preparation and return from a physical child. This
//! module does not decode, inspect archives, collapse names, discover sidecars, or mutate a DB.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::archive_converter::ArchiveFormat;
use crate::everything_ipc::{CancelHandle, EverythingReply, MAX_RESULTS};
use crate::file_type_visibility::{FileTypeCategory, FileTypeVisibility};
use crate::grid_item::GridItem;
use crate::io_semaphore::{GlobalIoSemaphore, IoPriority};
use crate::settings::{ListingSortMetadata, Settings};

/// Submitted recognition/visibility snapshot. Construction belongs on the prepare worker:
/// Susie pool initialization can wait, and a nonblocking pool lookup would lose startup formats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EverythingPolicy {
    pub(crate) visibility: FileTypeVisibility,
    pub(crate) include_epub: bool,
    pub(crate) include_convertible: bool,
    image_extensions: HashSet<String>,
}

impl EverythingPolicy {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        let image_extensions = crate::folder_tree::SUPPORTED_EXTENSIONS
            .iter()
            .map(|extension| (*extension).to_owned())
            .chain(crate::susie_loader::get_pool().extensions())
            .collect();
        Self {
            visibility: settings.file_type_visibility.clone(),
            include_epub: !settings.epub_file_handling_ignores_epub(),
            include_convertible: !settings.archive_file_handling_ignores_convertible(),
            image_extensions,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EverythingPreparedRow {
    /// Exact physical spelling key, retaining the drive; never a filename/stem key.
    pub(crate) physical_key: String,
    pub(crate) item: GridItem,
    /// Always Some for adopted rows. A failed stat never becomes a fabricated zero stamp.
    pub(crate) image_meta: Option<(i64, i64)>,
    pub(crate) sort_meta: ListingSortMetadata,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EverythingStats {
    /// Everything's total syntax hits, including unsupported files and folders.
    pub(crate) total_items: u32,
    pub(crate) raw_returned: usize,
    /// Folder/non-file, unsafe path, unrecognized format, or ignored handling setting.
    pub(crate) unsupported: usize,
    /// Recognized files excluded by the submitted file-type visibility policy.
    pub(crate) excluded: usize,
    pub(crate) unreadable: usize,
    pub(crate) duplicates: usize,
    /// True only when the raw cap omitted syntax hits, not for exactly 10,000 total hits.
    pub(crate) limit_reached: bool,
}

#[derive(Debug)]
pub(crate) struct EverythingDataset {
    pub(crate) rows: Vec<EverythingPreparedRow>,
    pub(crate) stats: EverythingStats,
}

/// Classify/stamp a complete, validated reply on the existing prepare worker. Cancellation
/// returns no partial dataset. The prepare owner must cancel `prepare_cancel` on request cancel
/// as well, so its existing cancellable I/O wait observes both retirement paths.
pub(crate) fn project_reply(
    reply: EverythingReply,
    policy: &EverythingPolicy,
    io: &GlobalIoSemaphore,
    request_cancel: &CancelHandle,
    prepare_cancel: &AtomicBool,
) -> Option<Arc<EverythingDataset>> {
    let raw_returned = reply.rows.len();
    debug_assert!(raw_returned <= MAX_RESULTS as usize);
    project_with(
        reply.total_items,
        raw_returned,
        reply.rows.into_iter().map(|row| {
            let is_file = row.is_file();
            (row.full_path, is_file)
        }),
        policy,
        || request_cancel.is_cancelled() || prepare_cancel.load(Ordering::Relaxed),
        |path| {
            let _permit = io.acquire_cancellable(IoPriority::Normal, prepare_cancel)?;
            if request_cancel.is_cancelled() || prepare_cancel.load(Ordering::Relaxed) {
                return None;
            }
            Some(stat_file(path))
        },
    )
    .map(Arc::new)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    mtime: i64,
    size: i64,
}

fn stat_file(path: &Path) -> Result<FileStamp, ()> {
    let metadata = std::fs::metadata(path).map_err(|_| ())?;
    if !metadata.is_file() {
        return Err(());
    }
    Ok(FileStamp {
        mtime: crate::ui_helpers::mtime_secs(&metadata),
        size: i64::try_from(metadata.len()).map_err(|_| ())?,
    })
}

/// Injectable stat keeps coverage independent of files, Everything installation, and IPC.
/// `None` means canceled acquisition; `Some(Err)` means an unreadable/non-file candidate.
fn project_with(
    total_items: u32,
    raw_returned: usize,
    candidates: impl IntoIterator<Item = (String, bool)>,
    policy: &EverythingPolicy,
    canceled: impl Fn() -> bool,
    mut stat: impl FnMut(&Path) -> Option<Result<FileStamp, ()>>,
) -> Option<EverythingDataset> {
    let mut dataset = EverythingDataset {
        rows: Vec::with_capacity(raw_returned),
        stats: EverythingStats {
            total_items,
            raw_returned,
            limit_reached: raw_returned >= MAX_RESULTS as usize
                && u64::from(total_items) > raw_returned as u64,
            ..EverythingStats::default()
        },
    };
    let mut seen = HashSet::with_capacity(raw_returned);
    for (full_path, is_file) in candidates {
        if canceled() {
            return None;
        }
        let Some(physical_key) = is_file.then(|| physical_file_key(&full_path)).flatten() else {
            dataset.stats.unsupported += 1;
            continue;
        };
        if !seen.insert(physical_key.clone()) {
            dataset.stats.duplicates += 1;
            continue;
        }
        let path = crate::path_key::normalize_input_path(Path::new(&full_path));
        let Some((item, category)) = classify(&path, policy) else {
            dataset.stats.unsupported += 1;
            continue;
        };
        if !policy.visibility.allows_path(category, &path) {
            dataset.stats.excluded += 1;
            continue;
        }
        let stamp = stat(&path)?;
        if canceled() {
            return None;
        }
        let Ok(stamp) = stamp else {
            dataset.stats.unreadable += 1;
            continue;
        };
        dataset.rows.push(EverythingPreparedRow {
            physical_key,
            item,
            image_meta: Some((stamp.mtime, stamp.size)),
            sort_meta: ListingSortMetadata::new(stamp.mtime, Some(stamp.size)),
        });
    }
    (!canceled()).then_some(dataset)
}

/// Same recognition precedence as folder_scan, with a frozen Susie extension snapshot.
/// Handling settings filter EPUB/convertible archives; sibling/stem skip settings do not apply
/// to explicit Everything filename hits.
fn classify(path: &Path, policy: &EverythingPolicy) -> Option<(GridItem, FileTypeCategory)> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let path = path.to_path_buf();
    if policy.image_extensions.contains(&extension) {
        let category = if crate::raw_format::is_raw_ext(&extension) {
            FileTypeCategory::Raw
        } else {
            FileTypeCategory::Image
        };
        Some((GridItem::Image(path), category))
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        Some((GridItem::Video(path), FileTypeCategory::Video))
    } else if crate::folder_tree::is_audio_ext(&extension) {
        Some((GridItem::Audio(path), FileTypeCategory::Audio))
    } else if crate::folder_tree::is_zip_extension(&extension) {
        Some((GridItem::ZipFile(path), FileTypeCategory::Zip))
    } else if crate::folder_tree::is_pdf_extension(&extension) {
        Some((GridItem::PdfFile(path), FileTypeCategory::Pdf))
    } else if extension == "epub" {
        policy
            .include_epub
            .then_some((GridItem::PdfFile(path), FileTypeCategory::Epub))
    } else {
        let format = ArchiveFormat::from_extension(&extension)?;
        policy.include_convertible.then_some((
            GridItem::ConvertibleArchive { path, format },
            crate::file_type_visibility::category_for_archive_format(format),
        ))
    }
}

/// Reject virtual identities and device/relative paths before the existing lexical key helper.
/// Accept ordinary drive absolute, UNC, verbatim drive absolute, and verbatim UNC spellings.
fn physical_file_key(raw: &str) -> Option<String> {
    if raw.chars().any(|character| character.is_control()) {
        return None;
    }
    let spelling = raw.replace('\\', "/");
    let ordinary = if let Some(rest) = spelling.strip_prefix("//?/") {
        if rest
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("unc/"))
        {
            format!("//{}", &rest[4..])
        } else {
            rest.to_owned()
        }
    } else {
        spelling
    };
    if ordinary.contains(['|', '<', '>', '"', '*', '?']) {
        return None;
    }
    let bytes = ordinary.as_bytes();
    let remainder = if bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'/'
    {
        &ordinary[3..]
    } else if let Some(unc) = ordinary.strip_prefix("//") {
        let (server, rest) = unc.split_once('/')?;
        let (share, tail) = rest.split_once('/')?;
        if server.is_empty()
            || share.is_empty()
            || server.contains(':')
            || share.contains(':')
            || [server, share]
                .iter()
                .any(|part| matches!(*part, "." | ".."))
        {
            return None;
        }
        tail
    } else {
        return None;
    };
    // A colon inside a file path is an alternate data stream / virtual-entry separator.
    if remainder.is_empty() || remainder.ends_with('/') || remainder.contains(':') {
        return None;
    }
    crate::path_key::physical_path_spelling(&PathBuf::from(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn policy() -> EverythingPolicy {
        EverythingPolicy {
            visibility: FileTypeVisibility::default(),
            include_epub: true,
            include_convertible: true,
            image_extensions: crate::folder_tree::SUPPORTED_EXTENSIONS
                .iter()
                .copied()
                .chain(["mag", "pi"])
                .map(str::to_owned)
                .collect(),
        }
    }

    fn project(paths: &[&str], policy: &EverythingPolicy) -> EverythingDataset {
        project_with(
            paths.len() as u32,
            paths.len(),
            paths.iter().map(|path| ((*path).to_owned(), true)),
            policy,
            || false,
            |_| Some(Ok(FileStamp { mtime: 7, size: 9 })),
        )
        .unwrap()
    }

    #[test]
    fn all_supported_formats_use_regular_grid_kinds_and_successful_stamps() {
        let policy = policy();
        let mut extensions: Vec<&str> = crate::folder_tree::SUPPORTED_EXTENSIONS.to_vec();
        extensions.extend_from_slice(crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS);
        extensions.extend_from_slice(crate::folder_tree::SUPPORTED_AUDIO_EXTENSIONS);
        extensions.extend([
            "mag", "pi", "zip", "cbz", "pdf", "epub", "rar", "cbr", "7z", "cb7", "lzh", "lha",
        ]);
        let paths: Vec<String> = extensions
            .iter()
            .map(|extension| format!(r"C:\Results\file.{extension}"))
            .collect();
        let dataset = project(
            &paths.iter().map(String::as_str).collect::<Vec<_>>(),
            &policy,
        );
        assert_eq!(dataset.rows.len(), extensions.len());
        for (row, extension) in dataset.rows.iter().zip(extensions) {
            assert_eq!(row.image_meta, Some((7, 9)));
            assert_eq!(row.sort_meta, ListingSortMetadata::new(7, Some(9)));
            match extension {
                "zip" | "cbz" => assert!(matches!(row.item, GridItem::ZipFile(_))),
                "pdf" | "epub" => assert!(matches!(row.item, GridItem::PdfFile(_))),
                "rar" | "cbr" | "7z" | "cb7" | "lzh" | "lha" => {
                    assert!(matches!(row.item, GridItem::ConvertibleArchive { .. }));
                }
                extension
                    if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension) =>
                {
                    assert!(matches!(row.item, GridItem::Video(_)));
                }
                extension if crate::folder_tree::is_audio_ext(extension) => {
                    assert!(matches!(row.item, GridItem::Audio(_)));
                }
                _ => assert!(matches!(row.item, GridItem::Image(_))),
            }
        }
        let epub = dataset
            .rows
            .iter()
            .find(|row| row.physical_key.ends_with(".epub"))
            .unwrap();
        assert_eq!(
            epub.item,
            GridItem::PdfFile(PathBuf::from(r"C:\Results\file.epub"))
        );
    }

    #[test]
    fn classify_and_policy_exclusions_happen_before_any_stat() {
        let mut policy = policy();
        policy.include_epub = false;
        policy.include_convertible = false;
        policy
            .visibility
            .excluded_categories
            .insert(FileTypeCategory::Raw);
        policy
            .visibility
            .excluded_extensions
            .insert("jpg".to_owned());
        let paths = [
            r"C:\a.jpg",
            r"C:\a.cr3",
            r"C:\a.epub",
            r"C:\a.rar",
            r"C:\a.txt",
            r"C:\folder.png",
        ];
        let dataset = project_with(
            paths.len() as u32,
            paths.len(),
            paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| (path.to_owned(), index != 5)),
            &policy,
            || false,
            |_| panic!("unsupported, excluded and folder rows must not be statted"),
        )
        .unwrap();
        assert!(dataset.rows.is_empty());
        assert_eq!(dataset.stats.unsupported, 4);
        assert_eq!(dataset.stats.excluded, 2);
    }

    #[test]
    fn same_stems_and_sidecar_hits_are_independent_rows() {
        let dataset = project(
            &[
                r"C:\movie.mp4",
                r"C:\movie.jpg",
                r"C:\movie.png",
                r"C:\movie.miv.mkv",
                r"C:\movie.zip",
                r"C:\movie.rar",
            ],
            &policy(),
        );
        assert_eq!(dataset.rows.len(), 6);
    }

    #[test]
    fn physical_spelling_dedup_keeps_drive_identity_and_unc() {
        let dataset = project(
            &[
                r"C:\Images\a.JPG",
                r"\\?\C:\Images\a.jpg",
                r"c:/images/./a.jpg",
                r"D:\Images\a.jpg",
                r"\\Server\Share\a.jpg",
                r"\\?\UNC\server\share\a.jpg",
            ],
            &policy(),
        );
        assert_eq!(dataset.rows.len(), 3);
        assert_eq!(dataset.stats.duplicates, 3);
        assert_eq!(dataset.rows[0].physical_key, "c:/images/a.jpg");
        assert_eq!(dataset.rows[1].physical_key, "d:/images/a.jpg");
        assert_eq!(dataset.rows[2].physical_key, "//server/share/a.jpg");
    }

    #[test]
    fn rejects_relative_url_device_control_and_virtual_entry_paths() {
        for path in [
            "",
            "a.jpg",
            r"C:a.jpg",
            r"\a.jpg",
            "/a.jpg",
            "https://host/a.jpg",
            r"\\.\C:\a.jpg",
            r"\\?\GLOBALROOT\Device\a.jpg",
            r"\\?\Volume{a}\a.jpg",
            r"\\Server\Share",
            r"\\Server:stream\Share\a.jpg",
            r"\\Server\Share:stream\a.jpg",
            r"C:\",
            r"C:\..\a.jpg",
            r"C:\a.zip|page.jpg",
            r"C:\a.zip::page.jpg",
            "C:\\a\nb.jpg",
            "C:\\a\u{1f}b.jpg",
        ] {
            assert!(
                physical_file_key(path).is_none(),
                "unexpected accepted path {path:?}"
            );
        }
    }

    #[test]
    fn failed_stat_is_omitted_and_real_zero_byte_stamp_is_preserved() {
        let dataset = project_with(
            3,
            3,
            [r"C:\empty.jpg", r"C:\missing.jpg", r"C:\now_folder.jpg"]
                .map(|path| (path.to_owned(), true)),
            &policy(),
            || false,
            |path| {
                Some(if path.ends_with("empty.jpg") {
                    Ok(FileStamp { mtime: 0, size: 0 })
                } else {
                    Err(())
                })
            },
        )
        .unwrap();
        assert_eq!(dataset.rows.len(), 1);
        assert_eq!(dataset.rows[0].image_meta, Some((0, 0)));
        assert_eq!(dataset.rows[0].sort_meta.file_size, Some(0));
        assert_eq!(dataset.stats.unreadable, 2);
    }

    #[test]
    fn cap_counts_raw_hits_even_when_none_are_displayable() {
        for (count, total, capped) in [
            (9_999, 9_999, false),
            (10_000, 10_000, false),
            (10_000, 52_340, true),
        ] {
            let dataset = project_with(
                total,
                count,
                (0..count).map(|index| (format!(r"C:\file{index}.txt"), true)),
                &policy(),
                || false,
                |_| panic!("unsupported hits do not require stat"),
            )
            .unwrap();
            assert_eq!(dataset.stats.raw_returned, count);
            assert_eq!(dataset.stats.total_items, total);
            assert_eq!(dataset.stats.limit_reached, capped);
            assert_eq!(dataset.stats.unsupported, count);
            assert!(dataset.rows.is_empty());
        }
    }

    #[test]
    fn cancel_during_stat_does_not_publish_partial_rows() {
        let canceled = Cell::new(false);
        let dataset = project_with(
            2,
            2,
            [r"C:\a.jpg", r"C:\b.jpg"].map(|path| (path.to_owned(), true)),
            &policy(),
            || canceled.get(),
            |_| {
                canceled.set(true);
                Some(Ok(FileStamp { mtime: 7, size: 9 }))
            },
        );
        assert!(dataset.is_none());
    }

    #[test]
    fn canceled_permit_acquisition_does_not_become_unreadable() {
        assert!(
            project_with(
                1,
                1,
                [(r"C:\a.jpg".to_owned(), true)],
                &policy(),
                || false,
                |_| None,
            )
            .is_none()
        );
    }
}
