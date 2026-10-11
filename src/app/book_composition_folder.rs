//! Physical image-folder adapter for the common book projection.
//! All enumeration/materialization runs on the existing preparation workers.
//! Eligibility comes from the production listing; raw identities never promote a list to Book.
use std::{collections::HashMap, path::Path};

use super::book_materialization::{
    MaterializedBookRows, PageRows, ProjectionRecipe, finish_book_rows,
    materialize_local_folder_listing,
};
use crate::{book_composition::*, grid_item::GridItem, settings::Settings};

impl super::App {
    /// Preserve the initial book-open intent across row adoption. Choosing an
    /// index in the raw folder first would turn "from start" into an explicit
    /// page identity and reopen that page at its later composed position.
    pub(super) fn start_folder_composition_first_display(&mut self) -> bool {
        if self.book_composition_service.is_none()
            || !matches!(
                self.projection_recipe,
                ProjectionRecipe::Book(super::book_materialization::BookScope::ImageFolder(_))
            )
        {
            return false;
        }
        self.complete_deferred_book_composition_open(super::DeferredFsReopen {
            history_trigger: super::HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: super::DeferredFsTarget::None,
            resume_to_last_page: self.settings.book_open_resume.resumes(),
            from_explicit_open: true,
            preserve_after_password_prompt: true,
        });
        true
    }
}

pub(crate) struct PreparedFolderSource {
    pub(crate) raw_pages: Vec<CanonicalRawPage>,
    pub(crate) manifest: Vec<ManifestPage>,
    pub(crate) rows: PageRows,
    pub(crate) recipe: ProjectionRecipe,
}

/// Physical direct-image identity, shared with the editor and bookmark consumers.
pub(crate) fn page_ref(root: &Path, path: &Path) -> Result<CompositionPageRef, String> {
    let path = crate::book_scope::relative_reference_from_root(root, &path.to_string_lossy())?;
    let page = CompositionPageRef::RelativePath { path };
    PageDomain::ImageFolder
        .validate_page(&page)
        .map_err(|error| error.to_string())?;
    Ok(page)
}

/// Raw source enumeration deliberately ignores visibility, facets and same-name winners.
/// A reference to an existing excluded/filtered image stays distinct from a missing file.
fn raw_folder_pages(root: &Path) -> Result<Vec<CanonicalRawPage>, String> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if !crate::fs_entry::classify_dir_entry(&entry, &file_type).is_file()
            || crate::fs_entry::is_internal_app_entry_name(&entry.file_name())
            || crate::folder_tree::is_apple_double(&entry.path())
        {
            continue;
        }
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        if crate::folder_tree::is_recognized_image_ext(&extension.to_ascii_lowercase()) {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let key = super::BOOK_READING_PAGE_ORDER.name_key(&name);
            paths.push((path, key));
        }
    }
    paths.sort_by(|(_, a), (_, b)| super::BOOK_READING_PAGE_ORDER.compare_name_keys(a, 0, b, 0));
    paths
        .into_iter()
        .enumerate()
        .map(|(raw_index, (path, _))| {
            Ok(CanonicalRawPage {
                page: page_ref(root, &path)?,
                raw_index,
            })
        })
        .collect()
}

pub(crate) fn folder_manifest(
    root: &Path,
    raw_pages: &[CanonicalRawPage],
    items: &[GridItem],
    policy: &crate::file_type_visibility::FileTypeVisibility,
) -> Vec<ManifestPage> {
    let present: HashMap<_, _> = items
        .iter()
        .enumerate()
        .filter_map(|(item_index, item)| match item {
            GridItem::Image(path) => page_ref(root, path).ok().map(|page| (page, item_index)),
            _ => None,
        })
        .collect();
    raw_pages
        .iter()
        .map(|raw| {
            let availability = if let Some(item_index) = present.get(&raw.page) {
                ManifestAvailability::Present {
                    item_index: *item_index,
                }
            } else {
                let CompositionPageRef::RelativePath { path } = &raw.page else {
                    unreachable!("folder adapter only creates direct relative references")
                };
                if policy.allows_physical_file(&root.join(path)) {
                    ManifestAvailability::FilteredOut
                } else {
                    ManifestAvailability::ExcludedByPolicy
                }
            };
            ManifestPage {
                page: raw.page.clone(),
                availability,
            }
        })
        .collect()
}

/// Worker-only source preparation preserves released filtering, order and classification.
/// OFF/mixed/child-only listings are returned as OrdinaryList and never consume a record.
pub(crate) fn prepare_folder_source(
    root: &Path,
    settings: &Settings,
) -> Result<PreparedFolderSource, String> {
    let scan = super::folder_scan::scan_directory_with_settings(root, settings)
        .map_err(|error| error.to_string())?;
    let listing = materialize_local_folder_listing(root, scan, settings);
    let recipe = ProjectionRecipe::for_physical(settings, root, root, &listing.items);
    let raw_pages = raw_folder_pages(root)?;
    let manifest = folder_manifest(
        root,
        &raw_pages,
        &listing.items,
        &settings.file_type_visibility,
    );
    Ok(PreparedFolderSource {
        raw_pages,
        manifest,
        rows: (listing.items, listing.metas),
        recipe,
    })
}

pub(crate) fn project_folder_composition(
    root: &Path,
    raw_pages: &[CanonicalRawPage],
    rows: PageRows,
    policy: crate::file_type_visibility::FileTypeVisibility,
    token: CompositionToken,
    record: Option<&BookComposition>,
) -> CompositionResult<MaterializedBookRows> {
    let scope = crate::book_scope::BookScopeKey::folder(root).map_err(CompositionError::Json)?;
    let manifest = folder_manifest(root, raw_pages, &rows.0, &policy);
    let prepared = project_composition(
        scope,
        &PageDomain::ImageFolder,
        token,
        record,
        raw_pages,
        &manifest,
        rows.0.len(),
    )?;
    finish_book_rows(root, rows, policy, prepared)
}

#[cfg(test)]
#[path = "book_composition_folder_tests.rs"]
mod tests;
