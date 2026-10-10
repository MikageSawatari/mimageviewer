//! Enumeration provenance for real nested archive entries. Navigation names are not evidence.
use crate::file_type_visibility::{FileTypeCategory, FileTypeVisibility};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArchiveEntryLocator {
    pub index: usize,
    pub member_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArchiveContainerOrigin {
    pub locators: Vec<ArchiveEntryLocator>,
    pub category: FileTypeCategory,
    pub navigation_prefix: String,
}

impl ArchiveContainerOrigin {
    pub fn allows(&self, policy: &FileTypeVisibility) -> bool {
        self.locators.last().is_none_or(|locator| {
            policy.allows_path(self.category, std::path::Path::new(&locator.member_name))
        })
    }
}

/// Stored with a converted cache, mapping the published, deduplicated image name to ancestry.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ArchiveOriginManifest {
    pub version: u32,
    pub images: std::collections::BTreeMap<String, Vec<ArchiveContainerOrigin>>,
}

pub const MANIFEST_ENTRY: &str = "__mimageviewer_archive_origin_v1.json";
