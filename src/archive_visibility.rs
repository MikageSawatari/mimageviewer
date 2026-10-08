//! Worker-local archive projection shared by aggregate producers. No persistent state or UI I/O.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

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
    fn aggregate_members_share_actual_ancestry_and_preserve_unknown_references() {
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

/// One immutable policy and enumeration per distinct container for a single worker build.
pub(crate) struct ArchiveVisibilityProjection<'a> {
    policy: &'a FileTypeVisibility,
    cancel: &'a AtomicBool,
    containers: HashMap<PathBuf, Option<ArchiveProjection>>,
}

impl<'a> ArchiveVisibilityProjection<'a> {
    pub(crate) fn new(policy: &'a FileTypeVisibility, cancel: &'a AtomicBool) -> Self {
        Self {
            policy,
            cancel,
            containers: HashMap::new(),
        }
    }

    fn projection(&mut self, path: &Path) -> Option<&ArchiveProjection> {
        let policy = self.policy;
        let cancel = self.cancel;
        self.containers
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let enumeration = crate::zip_loader::enumerate_image_entries_detailed_with_cancel(
                    path,
                    Some(cancel),
                )
                .ok()?;
                let known_members = enumeration
                    .entries
                    .iter()
                    .map(|entry| entry.entry_name.clone())
                    .collect();
                let visible_members = enumeration
                    .entries
                    .iter()
                    .filter(|entry| {
                        policy.allows_physical_file(Path::new(&entry.entry_name))
                            && entry
                                .container_ancestry
                                .iter()
                                .all(|origin| origin.allows(policy))
                    })
                    .map(|entry| entry.entry_name.clone())
                    .collect();
                let raw_tree =
                    crate::zip_tree::ZipTree::build(path.to_path_buf(), enumeration.entries);
                let visible_tree = raw_tree.project(policy);
                Some(ArchiveProjection {
                    known_members,
                    visible_members,
                    raw_tree,
                    visible_tree,
                })
            })
            .as_ref()
    }

    pub(crate) fn allows_member(&mut self, path: &Path, member: &str) -> bool {
        if !self.policy.allows_physical_file(Path::new(member)) {
            return false;
        }
        if !self.policy.requires_archive_origins() {
            return true;
        }
        // Missing/unreadable references keep their existing placeholder/delete route. Only
        // known enumeration ancestry proves a nested archive exclusion; suffixes never do.
        self.projection(path).is_none_or(|projection| {
            !projection.known_members.contains(member)
                || projection.visible_members.contains(member)
        })
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
                self.projection(zip_path).is_none_or(|projection| {
                    projection.raw_tree.node_at(&segments).is_none()
                        || projection.visible_tree.node_at(&segments).is_some()
                })
            }
            _ => true,
        }
    }
}
