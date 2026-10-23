//! Resolve a rating target from the accepted recipe of the mounted viewer, without I/O.
use super::*;
use crate::rating_db::{RatingItemKind, RatingMeta};

#[derive(Clone, Debug)]
pub(crate) struct ContainerRatingTarget {
    pub(crate) key: String,
    pub(crate) source: PathBuf,
    pub(crate) meta: RatingMeta,
}

pub(crate) struct PhysicalContainerKey {
    pub(crate) key: String,
    pub(crate) source: PathBuf,
    pub(crate) dir_prefix: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum ContainerRatingToast {
    Container,
    Book,
}

#[derive(Debug)]
pub(crate) enum BookPreferredRatingTarget {
    Book(ContainerRatingTarget),
    ViewerItem,
    Unavailable(BookPreferredUnavailable),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookPreferredUnavailable {
    NoPhysicalTarget,
    ArchiveRootMismatch,
    ArchiveLevelMismatch,
    DocumentMismatch,
    FolderMismatch,
}

impl App {
    /// The existing container key derivation, independent of search UI and metadata I/O.
    pub(crate) fn physical_container_rating_key(&self) -> Option<PhysicalContainerKey> {
        if let Some(nav) = self.zip_nav.as_ref() {
            let root = zip_rating_root_path(
                &nav.tree.zip_path,
                self.archive_source_override.as_deref(),
                self.current_folder.as_deref(),
            );
            let (path, dir_prefix) = if nav.at_root() {
                (root.to_path_buf(), None)
            } else {
                let prefix = nav.current_parent_zipdir_prefix()?;
                let path = book_container_key_from_segs(root, &prefix);
                let prefix = prefix.join("/");
                let prefix = if prefix.is_empty() {
                    prefix
                } else {
                    format!("{prefix}/")
                };
                (path, Some(prefix))
            };
            return Some(PhysicalContainerKey {
                key: crate::adjustment_db::normalize_path(&path),
                source: root.to_path_buf(),
                dir_prefix,
            });
        }
        let folder = self.current_folder.as_ref()?;
        if is_synthetic_view_path(folder) {
            return None;
        }
        Some(PhysicalContainerKey {
            key: crate::adjustment_db::normalize_path(folder),
            source: folder.clone(),
            dir_prefix: None,
        })
    }

    pub(crate) fn resolve_book_preferred_viewer_target(&self) -> BookPreferredRatingTarget {
        use BookPreferredRatingTarget::{Book, Unavailable, ViewerItem};
        use BookPreferredUnavailable::*;
        use book_materialization::{BookScope, ProjectionRecipe};
        let ProjectionRecipe::Book(scope) = &self.projection_recipe else {
            return ViewerItem;
        };
        let Some(physical) = self.physical_container_rating_key() else {
            return Unavailable(NoPhysicalTarget);
        };
        let kind = match scope {
            BookScope::Archive {
                logical_root,
                prefix,
            } => {
                let Some(nav) = self.zip_nav.as_ref() else {
                    return Unavailable(ArchiveRootMismatch);
                };
                if !crate::folder_tree::path_eq(&physical.source, logical_root) {
                    return Unavailable(ArchiveRootMismatch);
                }
                if nav.current() != prefix {
                    return Unavailable(ArchiveLevelMismatch);
                }
                if !nav.at_root() {
                    RatingItemKind::ZipDir
                } else if physical
                    .source
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| {
                        crate::folder_tree::is_zip_extension(&ext.to_ascii_lowercase())
                    })
                {
                    RatingItemKind::ZipFile
                } else if archive_format_for_rating_path(&physical.source).is_some() {
                    RatingItemKind::ConvertibleArchive
                } else {
                    RatingItemKind::ZipFile
                }
            }
            BookScope::Document(logical) => {
                if self.zip_nav.is_some() || !crate::folder_tree::path_eq(&physical.source, logical)
                {
                    return Unavailable(DocumentMismatch);
                }
                RatingItemKind::PdfFile
            }
            BookScope::ImageFolder(path) | BookScope::CompiledFolder(path) => {
                if self.zip_nav.is_some()
                    || !self.current_folder.as_ref().is_some_and(|folder| {
                        !is_synthetic_view_path(folder) && crate::folder_tree::path_eq(folder, path)
                    })
                {
                    return Unavailable(FolderMismatch);
                }
                RatingItemKind::Folder
            }
        };
        let meta = container_rating_meta(&physical, kind);
        Book(ContainerRatingTarget {
            key: physical.key,
            source: physical.source,
            meta,
        })
    }

    pub(crate) fn report_book_preferred_unavailable(&mut self, reason: BookPreferredUnavailable) {
        crate::logger::log(format!(
            "[rating] book_preferred unavailable reason={reason:?}"
        ));
        self.show_feedback_toast("[この本は評価できません]".to_string());
    }
}

pub(super) fn container_rating_meta(
    physical: &PhysicalContainerKey,
    kind: RatingItemKind,
) -> RatingMeta {
    let mut meta = RatingMeta::new(kind).with_source_path(&physical.source);
    if let Some(prefix) = &physical.dir_prefix {
        meta.dir_prefix = Some(prefix.clone());
        meta.zipdir_is_archive = Some(zipdir_prefix_is_archive(prefix));
    }
    if kind == RatingItemKind::ConvertibleArchive
        && let Some(format) = archive_format_for_rating_path(&physical.source)
    {
        meta.archive_format = Some(archive_format_to_rating_str(format).to_string());
    }
    meta
}
